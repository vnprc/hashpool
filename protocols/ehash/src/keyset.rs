use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};

use binary_sv2::{PubKey, Deserialize};
use cdk::{amount::Amount, nuts::{CurrencyUnit, KeySet, Keys, PublicKey}};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum KeysetConversionError {
    #[error("invalid keyset id: {0:?}")]
    InvalidKeysetId(cdk::nuts::nut02::Error),
    #[error("expected 64 signing keys, found {0}")]
    InvalidKeyCount(usize),
    #[error("failed to parse public key: {0}")]
    InvalidPublicKey(String),
}

#[derive(Debug, Clone)]
pub struct KeysetId(pub cdk::nuts::nut02::Id);

impl From<KeysetId> for u64 {
    fn from(id: KeysetId) -> Self {
        let bytes = id.0.to_bytes();
        let mut array = [0u8; 8];
        array[..bytes.len()].copy_from_slice(&bytes);
        u64::from_be_bytes(array)
    }
}

impl TryFrom<u64> for KeysetId {
    type Error = cdk::nuts::nut02::Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        let bytes = value.to_be_bytes();
        cdk::nuts::nut02::Id::from_bytes(&bytes).map(KeysetId)
    }
}

impl std::ops::Deref for KeysetId {
    type Target = cdk::nuts::nut02::Id;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct SigningKey {
    pub amount: u64,
    pub parity_bit: bool,
    pub pubkey: PubKey<'static>,
}

impl SigningKey {
    pub const BYTES: usize = 41;
}

pub fn signing_keys_from_cdk(keyset: &KeySet) -> Result<[SigningKey; 64], KeysetConversionError> {
    let mut sv2_keys = Vec::with_capacity(64);
    for (amount_str, public_key) in keyset.keys.keys().iter() {
        let mut pubkey_bytes = public_key.to_bytes();
        let (parity_byte, pubkey_data) = pubkey_bytes.split_at_mut(1);
        let parity_bit = parity_byte[0] == 0x03;

        let pubkey = PubKey::from_bytes(pubkey_data)
            .map_err(|_| KeysetConversionError::InvalidPublicKey("Sv2 public key parse".into()))?
            .into_static();

        sv2_keys.push(SigningKey {
            amount: (*amount_str.as_ref()).into(),
            parity_bit,
            pubkey,
        });
    }

    if sv2_keys.len() != 64 {
        return Err(KeysetConversionError::InvalidKeyCount(sv2_keys.len()));
    }

    sv2_keys
        .try_into()
        .map_err(|_| KeysetConversionError::InvalidKeyCount(0))
}

pub fn signing_keys_to_cdk(keys: &[SigningKey]) -> Result<Keys, KeysetConversionError> {
    let mut map = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if k.parity_bit { 0x03 } else { 0x02 };
        pubkey_bytes[1..].copy_from_slice(k.pubkey.inner_as_ref());

        let pubkey = PublicKey::from_slice(&pubkey_bytes)
            .map_err(|e| KeysetConversionError::InvalidPublicKey(format!("key {i}: {e:?}")))?;

        map.insert(Amount::from(k.amount), pubkey);
    }
    Ok(Keys::new(map))
}

pub fn calculate_keyset_id(keys: &[SigningKey]) -> u64 {
    match signing_keys_to_cdk(keys) {
        Ok(keys_map) => {
            let id = cdk::nuts::nut02::Id::v1_from_keys(&keys_map);
            let id_bytes = id.to_bytes();
            let mut padded = [0u8; 8];
            padded[..id_bytes.len()].copy_from_slice(&id_bytes);
            u64::from_be_bytes(padded)
        }
        Err(_) => 0,
    }
}

pub fn build_cdk_keyset(
    keyset_id: u64,
    signing_keys: &[SigningKey; 64],
) -> Result<KeySet, KeysetConversionError> {
    let id = *KeysetId::try_from(keyset_id).map_err(KeysetConversionError::InvalidKeysetId)?;
    let keys = signing_keys_to_cdk(signing_keys)?;
    Ok(KeySet {
        id,
        unit: CurrencyUnit::Hash,
        keys,
        final_expiry: None,
    })
}

pub fn keyset_from_sv2_bytes(bytes: &[u8]) -> Result<cdk::nuts::nut02::Id, cdk::nuts::nut02::Error> {
    if bytes.is_empty() {
        return cdk::nuts::nut02::Id::from_bytes(&[0u8; 8]);
    }

    let has_real_data = bytes.iter().any(|&x| x != 0);
    if !has_real_data {
        return cdk::nuts::nut02::Id::from_bytes(&[0u8; 8]);
    }

    match bytes.len() {
        8 | 33 => cdk::nuts::nut02::Id::from_bytes(bytes),
        len if len >= 8 => {
            let mut temp = [0u8; 8];
            temp.copy_from_slice(&bytes[len - 8..]);
            cdk::nuts::nut02::Id::from_bytes(&temp)
        }
        _ => {
            let mut temp = [0u8; 8];
            temp[..bytes.len()].copy_from_slice(bytes);
            cdk::nuts::nut02::Id::from_bytes(&temp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, RngCore};
    use secp256k1::{Secp256k1, SecretKey, PublicKey as SecpPublicKey};

    fn fresh_secret_key(rng: &mut impl RngCore) -> SecretKey {
        loop {
            let mut bytes = [0u8; 32];
            rng.fill_bytes(&mut bytes);
            if let Ok(sk) = SecretKey::from_byte_array(bytes) {
                return sk;
            }
        }
    }

    fn test_signing_keys() -> [SigningKey; 64] {
        let secp = Secp256k1::new();
        let mut rng = rand::thread_rng();
        core::array::from_fn(|i| {
            let sk = fresh_secret_key(&mut rng);
            let pk = SecpPublicKey::from_secret_key(&secp, &sk);
            let serialized = pk.serialize();
            let parity_bit = serialized[0] == 0x03;
            let mut inner = [0u8; 32];
            inner.copy_from_slice(&serialized[1..]);

            SigningKey {
                amount: 1u64 << i,
                parity_bit,
                pubkey: PubKey::from_bytes(&mut inner).unwrap().into_static(),
            }
        })
    }

    #[test]
    fn roundtrip_keyset() {
        let keys = test_signing_keys();
        let cdk_keys = signing_keys_to_cdk(&keys).unwrap();
        assert_eq!(cdk_keys.len(), keys.len());
    }

    #[test]
    fn calculate_id_nonzero() {
        let keys = test_signing_keys();
        assert_ne!(calculate_keyset_id(&keys), 0);
    }

    #[test]
    fn build_and_parse_cdk_keyset() {
        let keys = test_signing_keys();
        let keyset = build_cdk_keyset(42, &keys).unwrap();
        let parsed = signing_keys_from_cdk(&keyset).unwrap();
        for (lhs, rhs) in keys.iter().zip(parsed.iter()) {
            assert_eq!(lhs.amount, rhs.amount);
            assert_eq!(lhs.parity_bit, rhs.parity_bit);
            assert_eq!(lhs.pubkey.inner_as_ref(), rhs.pubkey.inner_as_ref());
        }
    }

    #[test]
    fn sv2_keyset_bytes_roundtrip() {
        let keyset = keyset_from_sv2_bytes(&[0u8; 8]).unwrap();
        assert_eq!(keyset.to_bytes(), [0u8; 8]);

        let mut padded = [0u8; 12];
        padded[8..].copy_from_slice(&[1,2,3,4]);
        let id = keyset_from_sv2_bytes(&padded).unwrap();
        assert_eq!(&id.to_bytes(), &[0,0,0,0,1,2,3,4]);
    }
}
