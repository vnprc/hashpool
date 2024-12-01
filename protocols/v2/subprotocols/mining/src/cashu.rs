use cdk::{amount::{Amount, AmountStr}, nuts::{BlindSignature, BlindedMessage, CurrencyUnit, KeySet, PublicKey}};
use decodable::FieldMarker;
use encodable::EncodablePrimitive;
use core::default;
use std::{collections::BTreeMap, convert::{TryFrom, TryInto}};
pub use std::error::Error;

#[cfg(not(feature = "with_serde"))]
pub use binary_codec_sv2::{self, Decodable as Deserialize, Encodable as Serialize, *};
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

/// just like Seq064K without the lifetime
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CashuSeq64K<T> {
    inner: Vec<T>,
}

impl<T> CashuSeq64K<T> {
    const HEADERSIZE: usize = 2;

    pub fn new(inner: Vec<T>) -> Result<Self, Box<dyn std::error::Error>> {
        if inner.len() <= 65535 {
            Ok(Self { inner })
        } else {
            Err(Box::new(CashuError::SeqExceedsMaxSize(inner.len(), 65535)))
        }
    }

    pub fn into_inner(self) -> Vec<T> {
        self.inner
    }

    fn expected_len(data: &[u8]) -> Result<usize, Box<dyn std::error::Error>> {
        if data.len() >= Self::HEADERSIZE {
            Ok(u16::from_le_bytes([data[0], data[1]]) as usize)
        } else {
            Err(Box::new(CashuError::ReadError(data.len(), Self::HEADERSIZE)))
        }
    }
}

impl<T: GetSize> GetSize for CashuSeq64K<T> {
    fn get_size(&self) -> usize {
        let mut size = Self::HEADERSIZE;
        for with_size in &self.inner {
            size += with_size.get_size()
        }
        size
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sv2SigningKey {
    pub amount: u64,
    pub pubkey: [u8; 33],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sv2KeySet {
    pub id: u64,
    pub keys: CashuSeq64K<Sv2SigningKey>,
}

impl Default for Sv2KeySet {
    fn default() -> Self {
        Self {
            id: 0,
            keys: CashuSeq64K::new(Vec::new()).unwrap(),
        }
    }
}

impl TryFrom<KeySet> for Sv2KeySet {
    type Error = Box<dyn Error>;

    fn try_from(value: KeySet) -> Result<Self, Self::Error> {
        let id: u64 = KeysetId(value.id).into();

        let mut key_pairs = Vec::new();
            for (amount_str, public_key) in value.keys.keys() {
            let amount: u64 = amount_str.inner().into();
            // TODO investigate which is better Sv2BlindSignature parity bit or this
            let pubkey_bytes: [u8; 33] = public_key.to_bytes();
            let key_pair = Sv2SigningKey {
                amount,
                pubkey: pubkey_bytes,
            };
            key_pairs.push(key_pair);
        }

        let keys = CashuSeq64K::new(key_pairs)
            .map_err(|e| format!("Failed to create Seq064K: {:?}", e))?;

        Ok(Sv2KeySet { id, keys })
    }
}

impl TryFrom<Sv2KeySet> for KeySet {
    type Error = Box<dyn Error>;

    fn try_from(value: Sv2KeySet) -> Result<Self, Self::Error> {
        let id = *KeysetId::try_from(value.id)?;
        let mut keys_map: BTreeMap<AmountStr, PublicKey> = BTreeMap::new();
        for key_pair in value.keys.clone().into_inner() {
            let amount_str = AmountStr::from(Amount::from(key_pair.amount));
            let public_key = PublicKey::from_slice(&key_pair.pubkey[..])?;

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

impl<'a> From<Sv2KeySet> for EncodableField<'a> {
    fn from(value: Sv2KeySet) -> Self {
        // Encode `id` as a primitive field
        let id_field = EncodableField::Primitive(EncodablePrimitive::U64(value.id));

        // Encode each key in `keys` as a struct
        let keys_field: Vec<EncodableField> = value
            .keys
            .into_inner()
            .into_iter()
            .map(|key| {
                let amount_field = EncodableField::Primitive(EncodablePrimitive::U64(key.amount));
                
                // Convert 33-byte `pubkey` into parity_bit and 32-byte array
                let pubkey_bytes = key.pubkey;
                let parity_bit = pubkey_bytes[0] == 0x03;
                let pubkey_core = <[u8; 32]>::try_from(&pubkey_bytes[1..]).unwrap(); // Safe because we know the size is correct
                let pubkey_b032 = pubkey_core.into_b032();

                // Encode the converted fields
                let parity_field = EncodableField::Primitive(EncodablePrimitive::Bool(parity_bit));
                let pubkey_core_field =
                    EncodableField::Primitive(EncodablePrimitive::B032(pubkey_b032));

                EncodableField::Struct(vec![amount_field, parity_field, pubkey_core_field])
            })
            .collect();

        // Combine `id` and `keys` into the `Struct` variant
        EncodableField::Struct(vec![
            id_field,
            EncodableField::Struct(keys_field),
        ])
    }
}

impl Fixed for Sv2KeySet {
    const SIZE: usize = std::mem::size_of::<u64>() + 33;
}

impl<'a> Decodable<'a> for Sv2SigningKey {
    fn from_bytes(data: &'a mut [u8]) -> Result<Self, binary_sv2::Error> {
        let required_size = 8 + 33;
        if data.len() < required_size {
            // TODO use the right error
            return Err(binary_sv2::Error::OutOfBound);
        }

        let amount_bytes = &data[0..8];
        let amount = u64::from_le_bytes(amount_bytes.try_into().unwrap());

        let pubkey_bytes = &data[8..(8 + 33)];
        let mut pubkey = [0u8; 33];
        pubkey.copy_from_slice(pubkey_bytes);

        Ok(Sv2SigningKey { amount, pubkey })
    }

    // not needed
    fn get_structure(_: &[u8]) -> Result<std::vec::Vec<FieldMarker>, binary_codec_sv2::Error> {
        unimplemented!()
    }

    // not needed
    fn from_decoded_fields(_: Vec<decodable::DecodableField<'a>>) -> Result<Sv2SigningKey, binary_codec_sv2::Error> {
        unimplemented!()
    }
}

impl Encodable for Sv2SigningKey {
    fn to_bytes(self, dst: &mut [u8]) -> Result<usize, binary_sv2::Error> {
        let required_size = 8 + 33; // u64 (8 bytes) + pubkey (33 bytes)
        if dst.len() < required_size {
            // TODO use the right error
            return Err(binary_sv2::Error::OutOfBound);
        }

        dst[0..8].copy_from_slice(&self.amount.to_le_bytes());
        dst[8..(8 + 33)].copy_from_slice(&self.pubkey);

        Ok(required_size)
    }
}

// impl<'a> Encodable for Sv2KeySet {
//     fn to_bytes(self, dst: &mut [u8]) -> Result<usize, binary_sv2::Error> {
//         let keys = self.keys.into_inner();
        
//         let mut required_size = 8; // keyset id
//         let keys_length = keys.len();

//         // Encode all keys to get their total size
//         let mut keys_bytes = Vec::new();
//         for key in keys {
//             let mut key_bytes = vec![0u8; 41];
//             key.to_bytes(&mut key_bytes)?;
//             keys_bytes.extend_from_slice(&key_bytes);
//         }

//         required_size += 2; // Seq064K length prefix
//         required_size += keys_bytes.len();

//         if dst.len() < required_size {
//             // TODO use the right error
//             return Err(binary_sv2::Error::OutOfBound);
//         }

//         // Encode `id`
//         dst[0..8].copy_from_slice(&self.id.to_le_bytes());

//         // Encode length of `keys`
//         dst[8..10].copy_from_slice(&(keys_length as u16).to_le_bytes());

//         // Encode `keys`
//         dst[10..(10 + keys_bytes.len())].copy_from_slice(&keys_bytes);

//         Ok(required_size)
//     }
// }

impl<'a> Decodable<'a> for Sv2KeySet {
    fn from_bytes(data: &'a mut [u8]) -> Result<Self, binary_sv2::Error> {
        let mut offset = 0;

        // Decode `id`
        if data.len() < offset + 8 {
            // TODO use the right error
            return Err(binary_sv2::Error::OutOfBound);
        }
        let id_bytes = &data[offset..(offset + 8)];
        let id = u64::from_le_bytes(id_bytes.try_into().unwrap());
        offset += 8;

        // Decode length of `keys`
        if data.len() < offset + 2 {
            // TODO use the right error
            return Err(binary_sv2::Error::OutOfBound);
        }
        let length_bytes = &data[offset..(offset + 2)];
        let keys_length = u16::from_le_bytes(length_bytes.try_into().unwrap()) as usize;
        offset += 2;

        let mut keys = Vec::with_capacity(keys_length);
        for _ in 0..keys_length {
            if data.len() < offset + 41 {
                // TODO use the right error
                return Err(binary_sv2::Error::OutOfBound);
            }
            let key_data = &mut data[offset..(offset + 41)];
            let key = Sv2SigningKey::from_bytes(key_data)?;
            keys.push(key);
            offset += 41;
        }

        // TODO capture e and do something with it. New error type?
        let keys_seq = CashuSeq64K::new(keys).map_err(|e| binary_sv2::Error::DecodableConversionError)?;

        Ok(Sv2KeySet { id, keys: keys_seq })
    }

    // not needed
    fn get_structure(data: &[u8]) -> Result<Vec<FieldMarker>, binary_sv2::Error> {
        unimplemented!()
    }

    // not needed
    fn from_decoded_fields(data: Vec<decodable::DecodableField<'a>>) -> Result<Self, binary_sv2::Error> {
        unimplemented!()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn get_random_pubkey() -> Sv2SigningKey {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let mut pubkey = [0u8; 33];
        rng.fill(&mut pubkey[..]);

        Sv2SigningKey {
            amount: rng.gen::<u64>(),
            pubkey,
        }
    }

    fn get_random_keyset() -> Sv2KeySet {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        // TODO find max size of keyset
        let num_keys = rng.gen_range(1..10);
        let mut keys_vec = Vec::with_capacity(num_keys);
        for _ in 0..num_keys {
            keys_vec.push(get_random_pubkey());
        }
    
        Sv2KeySet {
            id: rng.gen::<u64>(),
            keys: CashuSeq64K::new(keys_vec.clone()).unwrap(),
        }
    }

    #[test]
    fn test_sv2_signing_key_encode_decode() {
        let original_key = get_random_pubkey();

        // encode it
        let mut buffer = [0u8; 41]; // 8 bytes for amount + 33 bytes for pubkey
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
        let original_keys = original_keyset.clone().keys.into_inner();

        let keys_length = original_keys.len();
        let required_size = 8 + 2 + (keys_length * 41); // id + length prefix + keys

        // encode it
        let mut buffer = vec![0u8; required_size];
        let encoded_size = original_keyset.clone().to_bytes(&mut buffer).unwrap();
        assert_eq!(encoded_size, required_size);

        // decode it
        let decoded_keyset = Sv2KeySet::from_bytes(&mut buffer).unwrap();
        assert_eq!(original_keyset.id, decoded_keyset.id);

        // check for equality
        let decoded_keys = decoded_keyset.keys.into_inner();
        assert_eq!(original_keys.len(), decoded_keys.len());

        for (original_key, decoded_key) in original_keys.iter().zip(decoded_keys.iter()) {
            assert_eq!(original_key.amount, decoded_key.amount);
            assert_eq!(original_key.pubkey, decoded_key.pubkey);
        }
    }
}