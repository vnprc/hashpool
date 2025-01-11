use cdk::{amount::{Amount, AmountStr}, nuts::{BlindSignature, BlindedMessage, CurrencyUnit, KeySet, PublicKey}};
use core::array;
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
    pub keys: B064K<'decoder>,
}

impl<'decoder> Sv2KeySet<'decoder> {
    const KEY_SIZE: usize = 41;
    const NUM_KEYS: usize = 64;

    /// Attempts to read 64 signing keys from the `keys` field.
    pub fn get_signing_keys(&self) -> Result<[Sv2SigningKey<'static>; 64], binary_sv2::Error> {
        let raw = self.keys.inner_as_ref();

        if raw.len() != Self::KEY_SIZE * Self::NUM_KEYS {
            return Err(binary_sv2::Error::DecodableConversionError);
        }

        let mut output = array::from_fn(|_| Sv2SigningKey::default());

        // Decode each 41-byte chunk into Sv2SigningKey
        for (i, chunk) in raw.chunks(Self::KEY_SIZE).enumerate() {
            let mut buffer = [0u8; Self::KEY_SIZE];
            buffer.copy_from_slice(chunk);
            
            let key = Sv2SigningKey::from_bytes(&mut buffer)?;

            output[i] = Sv2SigningKey {
                amount: key.amount,
                parity_bit: key.parity_bit,
                pubkey: key.pubkey.into_static(),
            };
        }

        Ok(output)
    }

    /// Takes an array of 64 keys, encodes them, and packs them into the `keys` field (`B064K`).
    pub fn set_signing_keys(&mut self, keys: &[Sv2SigningKey<'_>]) -> Result<(), binary_sv2::Error> {
        if keys.len() != Self::NUM_KEYS {
            return Err(binary_sv2::Error::DecodableConversionError);
        }

        let mut buffer = [0u8; Self::KEY_SIZE * Self::NUM_KEYS];

        for (i, key) in keys.iter().enumerate() {
            let start = i * Self::KEY_SIZE;
            let end = start + Self::KEY_SIZE;

            let key_buf = &mut buffer[start..end];
            let written = key.clone().to_bytes(key_buf)?;

            // sanity check
            if written != Self::KEY_SIZE {
                return Err(binary_sv2::Error::DecodableConversionError);
            }
        }

        self.keys = B064K::try_from(buffer.to_vec())?;
        Ok(())
    }
}

impl<'a> Default for Sv2KeySet<'a> {
    fn default() -> Self {
        const KEY_SIZE: usize = 41;
        const NUM_KEYS: usize = 64;

        let mut buffer = Vec::with_capacity(KEY_SIZE * NUM_KEYS);

        let default_key = Sv2SigningKey::default();
        let mut temp_buf = [0u8; KEY_SIZE];
        default_key
            .to_bytes(&mut temp_buf[..])
            .expect("Failed to serialize default Sv2SigningKey");

        for _ in 0..NUM_KEYS {
            buffer.extend_from_slice(&temp_buf);
        }

        let b064k = B064K::try_from(buffer)
            .expect("Failed to create B064K with default signing keys");

        Self {
            id: 0,
            keys: b064k,
        }
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
                amount: amount_str.inner().into(),
                parity_bit,
                pubkey,
            };
            sv2_keys.push(signing_key);
        }

        // sanity check
        if sv2_keys.len() != 64 {
            return Err("Expected KeySet to have exactly 64 keys".into());
        }

        let mut this = Sv2KeySet {
            id,
            keys: B064K::try_from(vec![0u8; 0])
                .map_err(|e| format!("binary_sv2::Error: {:?}", e))?,
        };
        this.set_signing_keys(&sv2_keys)
            .map_err(|e| format!("binary_sv2::Error: {:?}", e))?;

        Ok(this)
    }
}

impl<'a> TryFrom<Sv2KeySet<'a>> for KeySet {
    type Error = Box<dyn Error>;

    fn try_from(value: Sv2KeySet) -> Result<Self, Self::Error> {
        let id = *KeysetId::try_from(value.id)?;

        let signing_keys = value.get_signing_keys()
            .map_err(|e| format!("binary_sv2::Error: {:?}", e))?;

        let mut keys_map: BTreeMap<AmountStr, PublicKey> = BTreeMap::new();
        for signing_key in signing_keys.iter() {
            let amount_str = AmountStr::from(Amount::from(signing_key.amount));

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

    fn get_random_signing_key() -> Sv2SigningKey<'static> {
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

    fn get_random_keyset() -> Sv2KeySet<'static> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
    
        let mut signing_keys: [Sv2SigningKey<'static>; 64] = array::from_fn(|_| get_random_signing_key());
        for i in 0..64 {
            signing_keys[i] = get_random_signing_key();
        }

        let mut keyset = Sv2KeySet::default();
        // TODO this is an invalid keyset_id, does it matter?
        keyset.id = rng.gen::<u64>();
        keyset.set_signing_keys(&signing_keys).unwrap();

        keyset
    }

    #[test]
    fn test_sv2_signing_key_encode_decode() {
        let original_key = get_random_signing_key();

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
        let required_size = 8 + 2 + 64 * 41; // keyset_id + B064K length prefix + ( 64 * signing keys )
        let mut buffer = vec![0u8; required_size];


        let encoded_size = original_keyset.clone().to_bytes(&mut buffer).unwrap();
        assert_eq!(encoded_size, required_size);

        // decode it
        let decoded_keyset = Sv2KeySet::from_bytes(&mut buffer).unwrap();
        assert_eq!(original_keyset.id, decoded_keyset.id);

        // Check that all 64 keys match
        let original_keys = original_keyset.get_signing_keys().unwrap();
        let decoded_keys = decoded_keyset.get_signing_keys().unwrap();
        assert_eq!(original_keys, decoded_keys);
    }
}