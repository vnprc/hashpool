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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

// Domain type for operating on blind sigs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sv2BlindSignatureSet<'a> {
    // TODO add keyset ID?
    pub signatures: [Sv2BlindSignature<'a>; 64],
}

impl<'a> Sv2BlindSignatureSet<'a> {
    /// Each Sv2BlindSignature is encoded as:
    ///  - 8 bytes for `amount`
    ///  - 8 bytes for `keyset_id`
    ///  - 1 byte for `parity_bit`
    ///  - 32 bytes for the `blind_signature` pubkey
    pub const SIGNATURE_SIZE: usize = 49; 
    pub const NUM_SIGNATURES: usize = 64;
}

impl<'a> Default for Sv2BlindSignatureSet<'a> {
    fn default() -> Self {
        let default_sig = Sv2BlindSignature::default();
        let signatures = core::array::from_fn(|_| default_sig.clone());
        Sv2BlindSignatureSet { signatures }
    }
}

// wire type for network transmission
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sv2BlindSignatureSetWire<'decoder> {
    // 64 * 49 = 3136 bytes total, stored in a fixed-size B064K
    pub signatures: B064K<'decoder>,
}

impl<'a> TryFrom<Sv2BlindSignatureSetWire<'a>> for [Sv2BlindSignature<'a>; 64] {
    type Error = binary_sv2::Error;

    fn try_from(wire: Sv2BlindSignatureSetWire<'a>) -> Result<Self, Self::Error> {
        let raw = wire.signatures.inner_as_ref();
        let expected_len = Sv2BlindSignatureSet::SIGNATURE_SIZE * Sv2BlindSignatureSet::NUM_SIGNATURES;
        if raw.len() != expected_len {
            return Err(binary_sv2::Error::DecodableConversionError);
        }

        let mut out = core::array::from_fn(|_| Sv2BlindSignature::default());
        for (i, chunk) in raw.chunks(Sv2BlindSignatureSet::SIGNATURE_SIZE).enumerate() {
            let mut buf = [0u8; Sv2BlindSignatureSet::SIGNATURE_SIZE];
            buf.copy_from_slice(chunk);

            let sig = Sv2BlindSignature::from_bytes(&mut buf)
                .map_err(|_| binary_sv2::Error::DecodableConversionError)?;

            out[i] = sig.into_static();
        }
        Ok(out)
    }
}

impl<'a> TryFrom<&[Sv2BlindSignature<'a>; 64]> for Sv2BlindSignatureSetWire<'a> {
    type Error = binary_sv2::Error;

    fn try_from(domain_sigs: &[Sv2BlindSignature<'a>; 64]) -> Result<Self, Self::Error> {
        let mut buf = [0u8; Sv2BlindSignatureSet::SIGNATURE_SIZE * Sv2BlindSignatureSet::NUM_SIGNATURES];
        for (i, sig) in domain_sigs.iter().enumerate() {
            let start = i * Sv2BlindSignatureSet::SIGNATURE_SIZE;
            let end   = start + Sv2BlindSignatureSet::SIGNATURE_SIZE;

            sig.clone()
                .to_bytes(&mut buf[start..end])
                .map_err(|_| binary_sv2::Error::DecodableConversionError)?;
        }

        let encoded = B064K::try_from(buf.to_vec())
            .map_err(|_| binary_sv2::Error::DecodableConversionError)?;

        Ok(Sv2BlindSignatureSetWire { signatures: encoded })
    }
}

impl<'a> From<Sv2BlindSignatureSet<'a>> for Sv2BlindSignatureSetWire<'a> {
    fn from(domain: Sv2BlindSignatureSet<'a>) -> Self {
        // Should never fail if you have exactly 64 signatures
        (&domain.signatures).try_into()
            .expect("Encoding 64 blind signatures into wire data should not fail")
    }
}

impl<'a> TryFrom<Sv2BlindSignatureSetWire<'a>> for Sv2BlindSignatureSet<'a> {
    type Error = binary_sv2::Error;

    fn try_from(wire: Sv2BlindSignatureSetWire<'a>) -> Result<Self, Self::Error> {
        let signatures: [Sv2BlindSignature<'a>; 64] = wire.clone().try_into()?;
        Ok(Self { signatures })
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
            id: 0, // ID can be set later by the caller
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

        let mut keys_map: BTreeMap<AmountStr, PublicKey> = BTreeMap::new();
        for signing_key in value.keys.iter() {
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
    
        let mut keys: [Sv2SigningKey<'static>; 64] = array::from_fn(|_| get_random_signing_key());
        for i in 0..64 {
            keys[i] = get_random_signing_key();
        }

        Sv2KeySet {
            // TODO this is an invalid keyset_id, does it matter?
            id: rng.gen::<u64>(),
            keys,
        }
    }

    fn get_random_signature() -> Sv2BlindSignature<'static> {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let mut signature_bytes = [0u8; 32];
        rng.fill(&mut signature_bytes[..]);

        Sv2BlindSignature {
            keyset_id: rng.gen::<u64>(),
            amount: rng.gen::<u64>(),
            blind_signature: PubKey::from_bytes(&mut signature_bytes).unwrap().into_static(),
            parity_bit: rng.gen(),
        }
    }

    fn get_random_sigset() -> Sv2BlindSignatureSet<'static> {
        let mut sigs: [Sv2BlindSignature<'static>; 64] = array::from_fn(|_| get_random_signature());
        for i in 0..64 {
            sigs[i] = get_random_signature();
        }

        Sv2BlindSignatureSet {
            signatures: sigs,
        }
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
    fn test_sv2_keyset_domain_wire_conversion() {
        let original_keyset = get_random_keyset();
        let wire_keyset: Sv2KeySetWire = original_keyset.clone().into();
        let domain_keyset: Sv2KeySet = wire_keyset.clone().try_into().unwrap();

        assert_eq!(wire_keyset.id, domain_keyset.id);
        assert_eq!(original_keyset.keys, domain_keyset.keys);
    }

    #[test]
    fn test_sv2_blind_sig_set_domain_wire_conversion() {
        let original_sigset = get_random_sigset();
        let wire_sigset: Sv2BlindSignatureSet = original_sigset.clone().into();
        let domain_sigset: Sv2BlindSignatureSet = wire_sigset.clone().try_into().unwrap();

        // TODO prolly need to add keyset id for easy validation
        // assert_eq!(wire_sigset.id, domain_keyset.id);
        assert_eq!(original_sigset.signatures, domain_sigset.signatures);
    }
}