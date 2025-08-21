use cdk::{amount::Amount, nuts::{CurrencyUnit, KeySet, PublicKey, Keys}};
use core::array;
use std::{collections::BTreeMap, convert::{TryFrom, TryInto}};
pub use std::error::Error;
use tracing::warn;

#[cfg(not(feature = "with_serde"))]
pub use binary_sv2::binary_codec_sv2::{self, Decodable as Deserialize, Encodable as Serialize, *};
#[cfg(not(feature = "with_serde"))]
pub use derive_codec_sv2::{Decodable as Deserialize, Encodable as Serialize};


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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sv2SigningKey<'decoder> {
    pub amount: u64,
    pub parity_bit: bool,
    pub pubkey: PubKey<'decoder>,
}

impl<'decoder> Default for Sv2SigningKey<'decoder> {
    fn default() -> Self {
        Self { 
            amount: Default::default(),
            parity_bit: Default::default(),
            pubkey: PubKey::from(<[u8; 32]>::from([0_u8; 32])),
        }
    }
}

// Wire type for inter-role communication
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sv2KeySetWire<'decoder> {
    pub id: u64,
    pub keys: B064K<'decoder>,
}

// Domain type for in-role usage
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sv2KeySet<'a> {
    pub id: u64,
    pub keys: [Sv2SigningKey<'a>; 64],
}

impl<'a> Sv2KeySet<'a> {
    pub const KEY_SIZE: usize = 41;
    pub const NUM_KEYS: usize = 64;
}

impl<'a> TryFrom<Sv2KeySetWire<'a>> for [Sv2SigningKey<'a>; 64] {
    type Error = binary_sv2::Error;

    fn try_from(wire: Sv2KeySetWire<'a>) -> Result<Self, Self::Error> {
        let raw = wire.keys.inner_as_ref();
        if raw.len() != Sv2KeySet::KEY_SIZE * Sv2KeySet::NUM_KEYS {
            return Err(binary_sv2::Error::DecodableConversionError);
        }

        let mut keys = array::from_fn(|_| Sv2SigningKey::default());
        for (i, chunk) in raw.chunks(Sv2KeySet::KEY_SIZE).enumerate() {
            let mut buffer = [0u8; Sv2KeySet::KEY_SIZE];
            buffer.copy_from_slice(chunk);
            keys[i] = Sv2SigningKey::from_bytes(&mut buffer)
                .map_err(|_| binary_sv2::Error::DecodableConversionError)?
                .into_static();
        }
        Ok(keys)
    }
}

impl<'a> TryFrom<&[Sv2SigningKey<'a>; 64]> for Sv2KeySetWire<'a> {
    type Error = binary_sv2::Error;

    fn try_from(keys: &[Sv2SigningKey<'a>; 64]) -> Result<Self, Self::Error> {
        let mut buffer = [0u8; Sv2KeySet::KEY_SIZE * Sv2KeySet::NUM_KEYS];
        for (i, key) in keys.iter().enumerate() {
            let start = i * Sv2KeySet::KEY_SIZE;
            let end = start + Sv2KeySet::KEY_SIZE;
            key.clone()
                .to_bytes(&mut buffer[start..end])
                .map_err(|_| binary_sv2::Error::DecodableConversionError)?;
        }
        let encoded_keys = B064K::try_from(buffer.to_vec())
            .map_err(|_| binary_sv2::Error::DecodableConversionError)?;

        Ok(Sv2KeySetWire {
            id: calculate_keyset_id(keys),
            keys: encoded_keys,
        })
    }
}

impl<'a> From<Sv2KeySet<'a>> for Sv2KeySetWire<'a> {
    fn from(domain: Sv2KeySet<'a>) -> Self {
        (&domain.keys).try_into()
            .expect("Encoding keys to Sv2KeySetWire should not fail")
    }
}

impl<'a> TryFrom<Sv2KeySetWire<'a>> for Sv2KeySet<'a> {
    type Error = binary_sv2::Error;

    fn try_from(wire: Sv2KeySetWire<'a>) -> Result<Self, Self::Error> {
        let keys: [Sv2SigningKey<'a>; 64] = wire.clone().try_into()?;
        Ok(Sv2KeySet {
            id: wire.id,
            keys,
        })
    }
}

impl<'a> Default for Sv2KeySet<'a> {
    fn default() -> Self {
        let default_key = Sv2SigningKey::default();
        let keys = array::from_fn(|_| default_key.clone());
        Sv2KeySet { id: 0, keys }
    }
}

impl<'a> TryFrom<KeySet> for Sv2KeySet<'a> {
    type Error = Box<dyn Error>;

    fn try_from(value: KeySet) -> Result<Self, Self::Error> {
        let id: u64 = KeysetId(value.id).into();

        let mut sv2_keys = Vec::with_capacity(64);
        for (amount_str, public_key) in value.keys.keys().iter() {
            let mut pubkey_bytes = public_key.to_bytes();
            let (parity_byte, pubkey_data) = pubkey_bytes.split_at_mut(1);
            let parity_bit = parity_byte[0] == 0x03;

            let pubkey = PubKey::from_bytes(pubkey_data)
                .map_err(|_| "Failed to parse public key")?
                .into_static();

            let signing_key = Sv2SigningKey {
                amount: (*amount_str.as_ref()).into(),
                parity_bit,
                pubkey,
            };
            sv2_keys.push(signing_key);
        }

        // sanity check
        if sv2_keys.len() != 64 {
            return Err(format!("Expected KeySet to have exactly 64 keys. Keys found: {}", sv2_keys.len()).into());
        }

        let keys: [Sv2SigningKey<'a>; 64] = sv2_keys
            .try_into()
            .map_err(|_| "Failed to convert Vec<Sv2SigningKey> into array")?;

        Ok(Sv2KeySet { id, keys })
    }
}

impl<'a> TryFrom<Sv2KeySet<'a>> for KeySet {
    type Error = Box<dyn Error>;

    fn try_from(value: Sv2KeySet) -> Result<Self, Self::Error> {
        let id = *KeysetId::try_from(value.id)?;

        let mut keys_map: BTreeMap<Amount, PublicKey> = BTreeMap::new();
        for signing_key in value.keys.iter() {
            let amount_str = Amount::from(signing_key.amount);

            let mut pubkey_bytes = [0u8; 33];
            pubkey_bytes[0] = if signing_key.parity_bit { 0x03 } else { 0x02 };
            pubkey_bytes[1..].copy_from_slice(&signing_key.pubkey.inner_as_ref());
            
            let public_key = PublicKey::from_slice(&pubkey_bytes)?;
    
            keys_map.insert(amount_str, public_key);
        }

        Ok(KeySet {
            id,
            unit: CurrencyUnit::Custom("HASH".to_string()),
            keys: cdk::nuts::Keys::new(keys_map),
            final_expiry: None,
        })
    }
}

// TODO find a better place for this
pub fn calculate_work(hash: [u8; 32]) -> u64 {
    let mut work = 0u64;

    for byte in hash {
        if byte == 0 {
            work += 8; // Each zero byte adds 8 bits of work
        } else {
            // Count the leading zeros in the current byte
            work += byte.leading_zeros() as u64;
            break; // Stop counting after the first non-zero byte
        }
    }

    work
}

// SRI encodings are totally fucked. Just do it manually.
// TODO delete this function. Probably use serde after upgrading to SRI 1.3
use cdk::nuts::nutXX::MintQuoteMiningShareRequest;

pub fn format_quote_event_json(req: &MintQuoteMiningShareRequest) -> String {
    use std::fmt::Write;
    use cdk::nuts::CurrencyUnit;
    use cdk::util::hex;

    let mut out = String::new();
    out.push('{');

    match &req.unit {
        CurrencyUnit::Custom(s) => write!(out, "\"unit\":\"{}\",", s).unwrap(),
        currency_unit => write!(out, "\"unit\":\"{}\",", currency_unit).unwrap(),
    }

    write!(
        out,
        "\"amount\":{},\"header_hash\":\"{}\",",
        req.amount.to_string(),
        req.header_hash
    ).unwrap();

    match &req.description {
        Some(d) => write!(out, "\"description\":\"{}\",", d).unwrap(),
        None => write!(out, "\"description\":null,").unwrap(),
    }

    match &req.pubkey {
        Some(pk) => write!(out, "\"pubkey\":\"{}\",", hex::encode(pk.to_bytes())).unwrap(),
        None => write!(out, "\"pubkey\":null,").unwrap(),
    }

    write!(out, "\"keyset_id\":\"{}\"", hex::encode(req.keyset_id.to_bytes())).unwrap();

    out.push('}');
    out
}

fn sv2_signing_keys_to_keys(keys: &[Sv2SigningKey]) -> Result<Keys, String> {
    let mut map = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if k.parity_bit { 0x03 } else { 0x02 };
        pubkey_bytes[1..].copy_from_slice(k.pubkey.inner_as_ref());

        let pubkey = PublicKey::from_slice(&pubkey_bytes)
            .map_err(|e| format!("Failed to parse public key for key {}: {:?}", i, e))?;

        map.insert(
            Amount::from(k.amount),
            pubkey,
        );
    }
    Ok(Keys::new(map))
}

fn calculate_keyset_id(keys: &[Sv2SigningKey]) -> u64 {
    match sv2_signing_keys_to_keys(keys) {
        Ok(keys_map) => {
            let id = cdk::nuts::nut02::Id::v1_from_keys(&keys_map);
            let id_bytes = id.to_bytes();

            let mut padded = [0u8; 8];
            padded[..id_bytes.len()].copy_from_slice(&id_bytes);

            u64::from_be_bytes(padded)
        }
        Err(e) => {
            warn!("Failed to generate Keys, defaulting keyset ID to 0: {}", e);
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin_hashes::sha256;
    use rand::{Rng, RngCore};
    use secp256k1::{PublicKey as SecpPublicKey, Secp256k1, SecretKey};
    use std::collections::BTreeMap;

    // ------------------------------------------------------------------------------------------------
    // Helper functions (available only when compiling tests)
    // ------------------------------------------------------------------------------------------------

    fn fresh_secret_key(rng: &mut impl RngCore) -> SecretKey {
        loop {
            let mut bytes = [0u8; 32];
            rng.fill_bytes(&mut bytes);
            if let Ok(sk) = SecretKey::from_byte_array(bytes) {
                return sk;
            }
        }
    }

    fn make_pubkey() -> cdk::nuts::PublicKey {
        let secp = Secp256k1::new();
        let mut rng = rand::thread_rng();
        let sk = fresh_secret_key(&mut rng);
        let pk: SecpPublicKey = SecpPublicKey::from_secret_key(&secp, &sk);
        cdk::nuts::PublicKey::from_slice(&pk.serialize()).unwrap()
    }

    fn test_sv2_keyset() -> Sv2KeySet<'static> {
        let secp = Secp256k1::new();
        let mut rng = rand::thread_rng();

        let keys = core::array::from_fn(|i| {
            let sk = fresh_secret_key(&mut rng);
            let pk = SecpPublicKey::from_secret_key(&secp, &sk);
            let bytes = pk.serialize();

            let parity_bit = bytes[0] == 0x03;
            let mut inner = [0u8; 32];
            inner.copy_from_slice(&bytes[1..]);

            Sv2SigningKey {
                amount: 1u64 << i,
                parity_bit,
                pubkey: PubKey::from_bytes(&mut inner).unwrap().into_static(),
            }
        });

        Sv2KeySet {
            id: rng.gen(),
            keys,
        }
    }

    // ------------------------------------------------------------------------------------------------
    //                                          Tests
    // ------------------------------------------------------------------------------------------------

    #[test]
    fn test_sv2_keyset_roundtrip() {
        // Build deterministic Keys map
        let mut map = BTreeMap::new();
        for i in 0..64 {
            map.insert(Amount::from(1u64 << i), make_pubkey());
        }
        let keys = Keys::new(map);
        let id = cdk::nuts::nut02::Id::v1_from_keys(&keys);

        let keyset = KeySet {
            id,
            unit: CurrencyUnit::Custom("HASH".into()),
            keys,
            final_expiry: None,
        };

        let sv2: Sv2KeySet = keyset.clone().try_into().unwrap();
        let roundtrip: KeySet = sv2.try_into().unwrap();

        assert_eq!(keyset.id.to_bytes(), roundtrip.id.to_bytes());
        assert_eq!(
            keyset.keys.iter().collect::<BTreeMap<_, _>>(),
            roundtrip.keys.iter().collect::<BTreeMap<_, _>>()
        );
    }

    #[test]
    fn test_sv2_signing_keys_to_keys_valid() {
        let sv2_keyset = test_sv2_keyset();
        let keys = sv2_signing_keys_to_keys(&sv2_keyset.keys).unwrap();
        assert_eq!(keys.len(), sv2_keyset.keys.len());

        for k in sv2_keyset.keys.iter() {
            assert!(keys.contains_key(&Amount::from(k.amount)));
        }
    }

    #[test]
    fn test_calculate_keyset_id_nonzero() {
        let sv2_keyset = test_sv2_keyset();
        let id = calculate_keyset_id(&sv2_keyset.keys);
        assert_ne!(id, 0);
    }

    #[test]
    fn test_format_quote_event_json_contains_fields() {
        let hash = sha256::Hash::hash(b"test");

        let req = cdk::nuts::nutXX::MintQuoteMiningShareRequest {
            amount: Amount::from(1000u64),
            unit: CurrencyUnit::Custom("HASH".into()),
            header_hash: cdk::secp256k1::hashes::Hash::from_slice(&hash.to_byte_array()).unwrap(),
            description: Some("test quote".into()),
            pubkey: Some(make_pubkey()),
            keyset_id: cdk::nuts::nut02::Id::v1_from_keys(&cdk::nuts::Keys::new(BTreeMap::new())),
        };

        let out = format_quote_event_json(&req);

        assert!(out.contains("test quote"));
        assert!(out.contains("HASH"));
    }


    #[test]
    fn test_calculate_work_expected_values() {
        assert_eq!(calculate_work([0u8; 32]), 256);
        let mut one = [0u8; 32];
        one[31] = 1;
        assert_eq!(calculate_work(one), 255);

        let mut sixteen = [0u8; 32];
        sixteen[31] = 0x10;
        assert_eq!(calculate_work(sixteen), 251);
    }


    #[test]
    fn test_sv2_keyset_wire_roundtrip() {
        let sv2 = test_sv2_keyset();
        let wire: Sv2KeySetWire = (&sv2.keys).try_into().unwrap();
        let domain: Sv2KeySet = wire.clone().try_into().unwrap();
        let wire2: Sv2KeySetWire = (&domain.keys).try_into().unwrap();
        assert_eq!(wire, wire2);
    }
}