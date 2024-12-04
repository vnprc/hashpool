use cdk::{amount::{Amount, AmountStr}, nuts::{BlindSignature, BlindedMessage, CurrencyUnit, KeySet, PublicKey}};
use std::{collections::BTreeMap, convert::{TryFrom, TryInto}};
pub use std::error::Error;

#[cfg(not(feature = "with_serde"))]
pub use binary_sv2::binary_codec_sv2::{self, Decodable as Deserialize, Encodable as Serialize, *};
#[cfg(not(feature = "with_serde"))]
pub use derive_codec_sv2::{Decodable as Deserialize, Encodable as Serialize};


// TODO find a better place for these errors
#[derive(Debug)]
pub enum CashuError {
    SeqExceedsMaxSize(usize, usize),
    ReadError(usize, usize),
}

impl std::fmt::Display for CashuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CashuError::SeqExceedsMaxSize(actual, max) => {
                write!(f, "Sequence exceeds max size: got {}, max is {}", actual, max)
            }
            CashuError::ReadError(actual, expected) => {
                write!(f, "Read error: got {}, expected at least {}", actual, expected)
            }
        }
    }
}

impl std::error::Error for CashuError {}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sv2BlindedMessage<'decoder> {
    pub amount: u64,
    pub keyset_id: u64,
    pub parity_bit: bool,
    pub blinded_secret: PubKey<'decoder>,
    // optional field, skip for now
    // pub witness: Option<Witness>,
}

impl From<BlindedMessage> for Sv2BlindedMessage<'_> {
    fn from(msg: BlindedMessage) -> Self {
        let blinded_secret_bytes = msg.blinded_secret.to_bytes();
        Self {
            amount: msg.amount.into(),
            keyset_id: KeysetId(msg.keyset_id).into(),
            parity_bit: blinded_secret_bytes[0] == 0x03,
            // unwrap is safe because blinded_secret is guaranteed to be 33 bytes
            blinded_secret: PubKey::from(<[u8; 32]>::try_from(&blinded_secret_bytes[1..]).unwrap()),
        }
    }
}

impl From<Sv2BlindedMessage<'_>> for BlindedMessage {
    fn from(msg: Sv2BlindedMessage) -> Self {
        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if msg.parity_bit { 0x03 } else { 0x02 };
        // copy_from_slice is safe because blinded_secret is guaranteed to be 32 bytes
        pubkey_bytes[1..].copy_from_slice(&msg.blinded_secret.inner_as_ref());

        BlindedMessage {
            amount: msg.amount.into(),
            keyset_id: *KeysetId::try_from(msg.keyset_id).unwrap(),
            blinded_secret: cdk::nuts::PublicKey::from_slice(&pubkey_bytes).unwrap(),
            witness: None,
        }
    }
}

// used for initialization
impl<'decoder> Default for Sv2BlindedMessage<'decoder> {
    fn default() -> Self {
        Self {
            amount: 0,
            keyset_id: 0,
            parity_bit: false,
            blinded_secret: PubKey::from([0u8; 32]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sv2BlindSignature<'decoder> {
    pub amount: u64,
    pub keyset_id: u64,
    pub parity_bit: bool,
    pub blind_signature: PubKey<'decoder>,
    // optional field, skip for now
    // pub dleq: Option<BlindSignatureDleq>,
}

impl From<BlindSignature> for Sv2BlindSignature<'_> {
    fn from(msg: BlindSignature) -> Self {
        let blind_sig_bytes = msg.c.to_bytes();
        Self {
            amount: msg.amount.into(),
            keyset_id: KeysetId(msg.keyset_id).into(),
            parity_bit: blind_sig_bytes[0] == 0x03,
            // unwrap is safe because blind_sig_bytes is guaranteed to be 33 bytes
            blind_signature: PubKey::from(<[u8; 32]>::try_from(&blind_sig_bytes[1..]).unwrap()),
        }
    }
}

impl From<Sv2BlindSignature<'_>> for BlindSignature {
    fn from(msg: Sv2BlindSignature) -> Self {
        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if msg.parity_bit { 0x03 } else { 0x02 };
        // copy_from_slice is safe because blinded_secret is guaranteed to be 32 bytes
        pubkey_bytes[1..].copy_from_slice(&msg.blind_signature.inner_as_ref());

        BlindSignature {
            amount: msg.amount.into(),
            keyset_id: *KeysetId::try_from(msg.keyset_id).unwrap(),
            c: cdk::nuts::PublicKey::from_slice(&pubkey_bytes).unwrap(),
            dleq: None,
        }
    }
}

// placeholder for now
impl<'decoder> Default for Sv2BlindSignature<'decoder> {
    fn default() -> Self {
        Self {
            amount: 0,
            keyset_id: 0,
            parity_bit: false,
            blind_signature: PubKey::from([0u8; 32]),
        }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sv2KeySet<'decoder> {
    pub id: u64,
    // just one key for now
    // TODO figure out how to do multiple keys
    pub key: Sv2SigningKey<'decoder>,
}

impl<'a> Default for Sv2KeySet<'a> {
    fn default() -> Self {
        Self {
            id: 0,
            key: Sv2SigningKey::default(),
        }
    }
}

impl<'a> TryFrom<KeySet> for Sv2KeySet<'a> {
    type Error = Box<dyn Error>;

    fn try_from(value: KeySet) -> Result<Self, Self::Error> {
        let id: u64 = KeysetId(value.id).into();

        if let Some((amount_str, public_key)) = value.keys.keys().iter().next() {
            let amount: u64 = amount_str.inner().into();
            let mut pubkey_bytes = public_key.to_bytes();
            let (parity_byte, pubkey_data) = pubkey_bytes.split_at_mut(1);
            let parity_bit = parity_byte[0] == 0x03;

            let pubkey = PubKey::from_bytes(pubkey_data)
                .map_err(|_| "Failed to parse public key")?
                .into_static();

            let keys = Sv2SigningKey {
                amount,
                parity_bit,
                pubkey,
            };
            Ok(Sv2KeySet { id, key: keys })
        } else {
            Err("KeySet contains no keys".into())
        }
    }
}

impl<'a> TryFrom<Sv2KeySet<'a>> for KeySet {
    type Error = Box<dyn Error>;

    fn try_from(value: Sv2KeySet) -> Result<Self, Self::Error> {
        let id = *KeysetId::try_from(value.id)?;
        let mut keys_map: BTreeMap<AmountStr, PublicKey> = BTreeMap::new();
        let amount_str = AmountStr::from(Amount::from(value.key.amount));

        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if value.key.parity_bit { 0x03 } else { 0x02 };
        pubkey_bytes[1..].copy_from_slice(&value.key.pubkey.inner_as_ref());

        let public_key = PublicKey::from_slice(&pubkey_bytes)?;

        keys_map.insert(amount_str, public_key);

        Ok(KeySet {
            id,
            unit: CurrencyUnit::Custom("HASH".to_string()),
            keys: cdk::nuts::Keys::new(keys_map),
        })
    }
}

// Define a trait for the conversion
pub trait IntoB032<'a> {
    fn into_b032(self) -> B032<'a>;
}

// Implement the trait for `[u8; 32]`
impl<'a> IntoB032<'a> for [u8; 32] {
    fn into_b032(self) -> B032<'a> {
        let inner = self.to_vec();
        inner.try_into().unwrap() // Safe because we know the sizes match
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn get_random_pubkey<'a>() -> Sv2SigningKey<'a> {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let mut pubkey_bytes = [0u8; 32];
        rng.fill(&mut pubkey_bytes[..]);

        Sv2SigningKey {
            amount: rng.gen::<u64>(),
            pubkey: PubKey::from_bytes(&mut pubkey_bytes).unwrap().into_static(),
            parity_bit: rng.gen(),
        }
    }

    fn get_random_keyset<'a>() -> Sv2KeySet<'a> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
    
        Sv2KeySet {
            id: rng.gen::<u64>(),
            key: get_random_pubkey(),
        }
    }

    #[test]
    fn test_sv2_signing_key_encode_decode() {
        let original_key = get_random_pubkey();

        // encode it
        let mut buffer = [0u8; 41]; // 8 byte amount + 33 byte pubkey
        let encoded_size = original_key.clone().to_bytes(&mut buffer).unwrap();
        assert_eq!(encoded_size, 41);

        // decode it
        let decoded_key = Sv2SigningKey::from_bytes(&mut buffer).unwrap();
        assert_eq!(original_key.amount, decoded_key.amount);
        assert_eq!(original_key.pubkey, decoded_key.pubkey);
    }

    #[test]
    fn test_sv2_keyset_encode_decode() {
        let original_keyset = get_random_keyset();
        let original_key = original_keyset.clone().key;
        let required_size = 8 + 8 + 1 + 32; // id + amount + parity_bit + pubkey

        // encode it
        let mut buffer = vec![0u8; required_size];
        let encoded_size = original_keyset.clone().to_bytes(&mut buffer).unwrap();
        println!("buffer {:?}", buffer);
        assert_eq!(encoded_size, required_size);

        // decode it
        let decoded_keyset = Sv2KeySet::from_bytes(&mut buffer).unwrap();
        assert_eq!(original_keyset.id, decoded_keyset.id);
        assert_eq!(original_key.amount, decoded_keyset.key.amount);
        assert_eq!(original_key.pubkey, decoded_keyset.key.pubkey);
    }
}