use cdk::{amount::{Amount, AmountStr}, nuts::{BlindSignature, BlindedMessage, CurrencyUnit, KeySet, PreMintSecrets, PublicKey}};
use decodable::{DecodableField, FieldMarker};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sv2BlindedMessage<'decoder> {
    pub parity_bit: bool,
    pub blinded_secret: PubKey<'decoder>,
}

// used for initialization
impl<'decoder> Default for Sv2BlindedMessage<'decoder> {
    fn default() -> Self {
        Self {
            parity_bit: false,
            blinded_secret: PubKey::from([0u8; 32]),
        }
    }
}

pub trait FromSv2BlindedMessage {
    fn from_sv2_blinded_message(
        msg: Sv2BlindedMessage,
        keyset_id: cdk::nuts::nut02::Id,
        amount: Amount,
    ) -> Self;
}

impl FromSv2BlindedMessage for BlindedMessage {
    fn from_sv2_blinded_message(
        sv2_msg: Sv2BlindedMessage,
        keyset_id: cdk::nuts::nut02::Id,
        amount: Amount,
    ) -> Self {
        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if sv2_msg.parity_bit { 0x03 } else { 0x02 };
        pubkey_bytes[1..].copy_from_slice(&sv2_msg.blinded_secret.inner_as_ref());

        let blinded_secret = cdk::nuts::PublicKey::from_slice(&pubkey_bytes)
            .expect("Invalid pubkey bytes");

        BlindedMessage {
            amount,
            keyset_id,
            blinded_secret,
            witness: None,
        }
    }
}

pub fn to_sv2_blinded_message(domain_msg: &BlindedMessage) -> Sv2BlindedMessage<'static> {
    let mut pubkey_bytes = domain_msg.blinded_secret.to_bytes();
    let parity_bit = pubkey_bytes[0] == 0x03;

    // Construct an Sv2BlindedMessage without amount/keyset
    Sv2BlindedMessage {
        parity_bit,
        // strip off the first byte, used to store parity
        blinded_secret: PubKey::from_bytes(&mut pubkey_bytes[1..])
            .expect("Invalid pubkey data")
            .into_static(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlindedMessageSet {
    pub keyset_id: u64,
    pub messages: [Option<BlindedMessage>; 64],
}

impl TryFrom<PreMintSecrets> for BlindedMessageSet {
    type Error = binary_sv2::Error;

    fn try_from(pre_mint_secrets: PreMintSecrets) -> Result<Self, Self::Error> {
        let mut messages: [Option<BlindedMessage>; 64] = core::array::from_fn(|_| None);

        for pre_mint in &pre_mint_secrets.secrets {
            let index = amount_to_index(pre_mint.amount.into());
            if messages[index].is_some() {
                // TODO use better error
                return Err(binary_sv2::Error::DecodableConversionError);
            }
            messages[index] = Some(pre_mint.blinded_message.clone());
        }

        Ok(BlindedMessageSet {
            keyset_id: u64::from(KeysetId(pre_mint_secrets.keyset_id)),
            messages,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sv2BlindedMessageSet<'a> {
    pub keyset_id: u64,
    pub messages: [Sv2BlindedMessage<'a>; 64],
}

impl<'a> Sv2BlindedMessageSet<'a> {
    pub const MESSAGE_SIZE: usize = 33;
    pub const NUM_MESSAGES: usize = 64;
}

impl<'a> Default for Sv2BlindedMessageSet<'a> {
    fn default() -> Self {
        let default_msg = Sv2BlindedMessage {
            parity_bit: false,
            blinded_secret: PubKey::from([0u8; 32]),
        };
        Self {
            keyset_id: 0,
            messages: core::array::from_fn(|_| default_msg.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sv2BlindedMessageSetWire<'decoder> {
    pub keyset_id: u64,
    pub messages: B064K<'decoder>,
}

impl<'a> Default for Sv2BlindedMessageSetWire<'a> {
    fn default() -> Self {
        Self {
            keyset_id: 0,
            messages: B064K::Owned(Vec::new()),
        }
    }
}

impl From<BlindedMessageSet> for Sv2BlindedMessageSet<'static> {
    fn from(domain_set: BlindedMessageSet) -> Self {
        let messages = core::array::from_fn(|i| {
            domain_set.messages[i]
                .as_ref()
                .map_or(Sv2BlindedMessage::default(), |msg| to_sv2_blinded_message(msg))
        });

        Sv2BlindedMessageSet {
            keyset_id: domain_set.keyset_id,
            messages,
        }
    }
}

impl From<Sv2BlindedMessageSet<'_>> for BlindedMessageSet {
    fn from(sv2_set: Sv2BlindedMessageSet) -> Self {
        let keyset_id_obj = KeysetId::try_from(sv2_set.keyset_id)
            .expect("Could not convert keyset_id to domain type");

        let messages = core::array::from_fn(|i| {
            if sv2_set.messages[i] == Sv2BlindedMessage::default() {
                None
            } else {
                Some(BlindedMessage::from_sv2_blinded_message(
                    sv2_set.messages[i].clone(),
                    *keyset_id_obj,
                    Amount::from(index_to_amount(i)),
                ))
            }
        });

        BlindedMessageSet {
            keyset_id: sv2_set.keyset_id,
            messages,
        }
    }
}

impl<'a> From<Sv2BlindedMessageSet<'a>> for Sv2BlindedMessageSetWire<'a> {
    fn from(domain: Sv2BlindedMessageSet<'a>) -> Self {
        let mut buffer = vec![0u8; Sv2BlindedMessageSet::MESSAGE_SIZE * Sv2BlindedMessageSet::NUM_MESSAGES];
        for (i, msg) in domain.messages.iter().enumerate() {
            let start = i * Sv2BlindedMessageSet::MESSAGE_SIZE;
            let end = start + Sv2BlindedMessageSet::MESSAGE_SIZE;
            msg.clone()
                .to_bytes(&mut buffer[start..end])
                .expect("Encoding of Sv2BlindedMessage should not fail");
        }

        let b064k = B064K::try_from(buffer)
            .expect("Encoding 64 blinded messages into wire data should not fail");

        Sv2BlindedMessageSetWire {
            keyset_id: domain.keyset_id,
            messages: b064k,
        }
    }
}

impl<'a> TryFrom<Sv2BlindedMessageSetWire<'a>> for Sv2BlindedMessageSet<'a> {
    type Error = binary_sv2::Error;

    fn try_from(wire: Sv2BlindedMessageSetWire<'a>) -> Result<Self, Self::Error> {
        let raw = wire.messages.inner_as_ref();
        let expected_len = Sv2BlindedMessageSet::MESSAGE_SIZE * Sv2BlindedMessageSet::NUM_MESSAGES;
        if raw.len() != expected_len {
            return Err(binary_sv2::Error::DecodableConversionError);
        }

        let mut msgs = core::array::from_fn(|_| Sv2BlindedMessage::default());
        for (i, chunk) in raw.chunks(Sv2BlindedMessageSet::MESSAGE_SIZE).enumerate() {
            let mut buf = [0u8; Sv2BlindedMessageSet::MESSAGE_SIZE];
            buf.copy_from_slice(chunk);

            let parsed = Sv2BlindedMessage::from_bytes(&mut buf)
                .map_err(|_| binary_sv2::Error::DecodableConversionError)?
                .into_static();

            msgs[i] = parsed;
        }

        Ok(Sv2BlindedMessageSet {
            keyset_id: wire.keyset_id,
            messages: msgs,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sv2BlindSignature<'decoder> {
    pub parity_bit: bool,
    pub blind_signature: PubKey<'decoder>,
}

impl<'decoder> Default for Sv2BlindSignature<'decoder> {
    fn default() -> Self {
        Self {
            parity_bit: false,
            blind_signature: PubKey::from([0u8; 32]),
        }
    }
}

// Domain type for operating on blind sigs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sv2BlindSignatureSet<'a> {
    pub keyset_id: u64,
    pub signatures: [Sv2BlindSignature<'a>; 64],
}

impl<'a> Sv2BlindSignatureSet<'a> {
    pub const SIGNATURE_SIZE: usize = 33; 
    pub const NUM_SIGNATURES: usize = 64;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlindSignatureSet {
    pub keyset_id: u64,
    pub signatures: [Option<BlindSignature>; 64],
}

impl<'a> Default for Sv2BlindSignatureSet<'a> {
    fn default() -> Self {
        let default_sig = Sv2BlindSignature::default();
        let signatures = core::array::from_fn(|_| default_sig.clone());
        Sv2BlindSignatureSet {
            keyset_id: 0,
            signatures,
        }
    }
}

pub trait FromSv2BlindSignature {
    fn from_sv2(signature: Sv2BlindSignature, keyset_id: KeysetId, amount: u64) -> Self;
}

impl FromSv2BlindSignature for BlindSignature {
    fn from_sv2(signature: Sv2BlindSignature, keyset_id: KeysetId, amount: u64) -> Self {
        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if signature.parity_bit { 0x03 } else { 0x02 };
        pubkey_bytes[1..].copy_from_slice(&signature.blind_signature.inner_as_ref());

        BlindSignature {
            amount: amount.into(),
            keyset_id: *keyset_id,
            c: cdk::nuts::PublicKey::from_slice(&pubkey_bytes).unwrap(),
            dleq: None,
        }
    }
}

impl From<Sv2BlindSignatureSet<'_>> for BlindSignatureSet {
    fn from(set: Sv2BlindSignatureSet) -> Self {
        let keyset_id = KeysetId::try_from(set.keyset_id).expect("Failed to convert keyset_id");
        let signatures = core::array::from_fn(|i| {
            if set.signatures[i] == Sv2BlindSignature::default() {
                None
            } else {
                Some(BlindSignature::from_sv2(
                    set.signatures[i].clone(),
                    // TODO why is KeysetId interpreted as Id? fix it and remove this hack
                    KeysetId(keyset_id.clone()),
                    index_to_amount(i),
                ))
            }
        });

        BlindSignatureSet {
            keyset_id: set.keyset_id,
            signatures,
        }
    }
}

impl<'a> From<BlindSignatureSet> for Sv2BlindSignatureSet<'a> {
    fn from(set: BlindSignatureSet) -> Self {
        let signatures = core::array::from_fn(|i| {
            set.signatures[i].as_ref().map_or(Sv2BlindSignature::default(), |sig| {
                Sv2BlindSignature {
                    parity_bit: sig.c.to_bytes()[0] == 0x03,
                    blind_signature: PubKey::from(<[u8; 32]>::try_from(&sig.c.to_bytes()[1..]).unwrap()),
                }
            })
        });

        Sv2BlindSignatureSet {
            keyset_id: set.keyset_id,
            signatures,
        }
    }
}

// wire type for network transmission
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sv2BlindSignatureSetWire<'decoder> {
    pub keyset_id: u64,
    // 64 * 49 = 3136 bytes total, stored in a fixed-size B064K
    pub signatures: B064K<'decoder>,
}

impl<'a> Default for Sv2BlindSignatureSetWire<'a> {
    fn default() -> Self {
        Self {
            keyset_id: Default::default(),
            signatures: B064K::Owned(Vec::new()),
        }
    }
}

impl<'a> From<Sv2BlindSignatureSet<'a>> for Sv2BlindSignatureSetWire<'a> {
    fn from(domain: Sv2BlindSignatureSet<'a>) -> Self {
        let mut buf = [0u8; Sv2BlindSignatureSet::SIGNATURE_SIZE * Sv2BlindSignatureSet::NUM_SIGNATURES];

        for (i, sig) in domain.signatures.iter().enumerate() {
            let start = i * Sv2BlindSignatureSet::SIGNATURE_SIZE;
            let end = start + Sv2BlindSignatureSet::SIGNATURE_SIZE;

            sig.clone()
                .to_bytes(&mut buf[start..end])
                .expect("Encoding of blind signature should not fail");
        }

        let encoded = B064K::try_from(buf.to_vec())
            .expect("Encoding 64 blind signatures into wire data should not fail");

        Sv2BlindSignatureSetWire {
            keyset_id: domain.keyset_id,
            signatures: encoded,
        }
    }
}

impl<'a> TryFrom<Sv2BlindSignatureSetWire<'a>> for Sv2BlindSignatureSet<'a> {
    type Error = binary_sv2::Error;

    fn try_from(wire: Sv2BlindSignatureSetWire<'a>) -> Result<Self, Self::Error> {
        let raw = wire.signatures.inner_as_ref();
        let expected_len = Sv2BlindSignatureSet::SIGNATURE_SIZE * Sv2BlindSignatureSet::NUM_SIGNATURES;
        if raw.len() != expected_len {
            return Err(binary_sv2::Error::DecodableConversionError);
        }

        let mut signatures = core::array::from_fn(|_| Sv2BlindSignature::default());
        for (i, chunk) in raw.chunks(Sv2BlindSignatureSet::SIGNATURE_SIZE).enumerate() {
            let mut buf = [0u8; Sv2BlindSignatureSet::SIGNATURE_SIZE];
            buf.copy_from_slice(chunk);

            let sig = Sv2BlindSignature::from_bytes(&mut buf)
                .map(|sig| sig.into_static())
                .map_err(|_| binary_sv2::Error::DecodableConversionError)?;

            signatures[i] = sig;
        }

        Ok(Self {
            keyset_id: wire.keyset_id,
            signatures,
        })
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

fn index_to_amount(index: usize) -> u64 {
    1u64 << index
}

fn amount_to_index(amount: u64) -> usize {
    // check if amount value is a non-zero power of 2
    if amount == 0 || amount.count_ones() != 1 {
        panic!("invalid amount {}", amount);
    }
    amount.trailing_zeros() as usize
}

const WIRE_ITEM_SIZE: usize = 33;

/// common trait implemented by domain items
/// allowing them to be stored in a 64-element array
/// keyed by power-of-two amounts
pub trait DomainItem<'decoder>: Clone {
    type WireType: Default + Clone + PartialEq + Eq + Serialize + Deserialize<'decoder>;

    fn from_wire(
        wire_obj: Self::WireType,
        keyset_id: cdk::nuts::nut02::Id,
        amount_index: usize,
    ) -> Self;

    fn to_wire(&self) -> Self::WireType;

    fn get_amount(&self) -> u64;
}

/// 64-element container for domain items keyed by 2^index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainArray<T: for<'decoder> DomainItem<'decoder>> {
    pub keyset_id: u64,
    pub items: [Option<T>; 64],
}

impl<T: for<'decoder> DomainItem<'decoder>> DomainArray<T> {
    pub fn new(keyset_id: u64) -> Self {
        Self {
            keyset_id,
            items: core::array::from_fn(|_| None),
        }
    }

    // Insert by inferring index from the domain item’s amount value.
    pub fn insert(&mut self, item: T) {
        let idx = amount_to_index(item.get_amount());
        self.items[idx] = Some(item);
    }

    // Retrieve an item by amount index.
    pub fn get(&self, amount: u64) -> Option<&T> {
        let idx = amount_to_index(amount);
        self.items[idx].as_ref()
    }
}

/// wire struct for transmitting 64 domain items in a single B064K
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireArray<'decoder> {
    pub keyset_id: u64,
    pub data: B064K<'decoder>,
}

// manually implement encodable and decodable
// because for some unknown reason it won't compile using the derive traits
// even though there are two exact replicas of this struct
// that do compile with those derivations. wtf???

#[cfg(not(feature = "with_serde"))]
impl<'decoder> binary_sv2::Encodable for WireArray<'decoder> {
    fn to_bytes(self, dst: &mut [u8]) -> Result<usize, binary_sv2::Error> {
        // keyset_id
        if dst.len() < 8 {
            // TODO better error?
            return Err(binary_sv2::Error::DecodableConversionError);
        }
        dst[0..8].copy_from_slice(&self.keyset_id.to_le_bytes());
        let mut offset = 8;

        let inner = self.data.inner_as_ref();
        let needed = offset + inner.len();
        if needed > dst.len() {
            // TODO better error?
            return Err(binary_sv2::Error::DecodableConversionError);
        }
        dst[offset..needed].copy_from_slice(inner);
        offset += inner.len();

        Ok(offset)
    }
}

#[cfg(not(feature = "with_serde"))]
impl<'decoder> binary_sv2::Decodable<'decoder> for WireArray<'decoder> {
    fn from_bytes(data: &'decoder mut [u8]) -> Result<Self, binary_sv2::Error> {
        // keyset_id
        if data.len() < 8 {
            return Err(binary_sv2::Error::DecodableConversionError);
        }
        let keyset_id_le: [u8; 8] = data[0..8].try_into().unwrap();
        let keyset_id = u64::from_le_bytes(keyset_id_le);

        let rest = &data[8..];
        let data_b064k = B064K::try_from(rest.to_vec())
            .map_err(|_| binary_sv2::Error::DecodableConversionError)?;

        Ok(WireArray {
            keyset_id,
            data: data_b064k,
        })
    }

    fn get_structure(_data: &[u8]) -> Result<Vec<FieldMarker>, binary_sv2::Error> {
        // don't use dynamic structure info
        Ok(vec![])
    }

    fn from_decoded_fields(_data: Vec<DecodableField<'decoder>>) -> Result<Self, binary_sv2::Error> {
        //TODO better error?
        Err(binary_sv2::Error::DecodableConversionError)
    }
}

impl<T> From<DomainArray<T>> for WireArray<'_> 
where
    for<'d> T: DomainItem<'d>,
{
    fn from(domain: DomainArray<T>) -> Self {
        let mut buffer = vec![0u8; WIRE_ITEM_SIZE * 64];

        for (i, maybe_item) in domain.items.iter().enumerate() {
            let offset = i * WIRE_ITEM_SIZE;
            let chunk = &mut buffer[offset..offset + WIRE_ITEM_SIZE];

            // Convert the domain item to wire form, or use the default if None.
            let wire_obj = maybe_item
                .as_ref()
                .map(|item| item.to_wire())
                .unwrap_or_else(|| T::WireType::default());

            wire_obj
                .to_bytes(chunk)
                .expect("Encoding should not fail");
        }

        let b064k = B064K::try_from(buffer).expect("domain items exceed B064K");
        Self {
            keyset_id: domain.keyset_id,
            data: b064k,
        }
    }
}

impl<T> TryFrom<WireArray<'_>> for DomainArray<T>
where
    for <'d> T: DomainItem<'d>,
{
    type Error = binary_sv2::Error;

    fn try_from(wire: WireArray<'_>) -> Result<Self, Self::Error> {
        let raw = wire.data.inner_as_ref();
        // TODO evaluate T::WireType::SIZE as an alternative to this constant
        let expected_len = WIRE_ITEM_SIZE * 64;
        if raw.len() != expected_len {
            return Err(binary_sv2::Error::DecodableConversionError);
        }

        let keyset_id_obj =
            KeysetId::try_from(wire.keyset_id).map_err(|_| binary_sv2::Error::DecodableConversionError)?;

        let mut result = DomainArray::new(wire.keyset_id);

        for (i, chunk) in raw.chunks(WIRE_ITEM_SIZE).enumerate() {
            let mut buf = [0u8; WIRE_ITEM_SIZE];
            buf.copy_from_slice(chunk);

            let wire_item = T::WireType::from_bytes(&mut buf)
                .map_err(|_| binary_sv2::Error::DecodableConversionError)?;

            if wire_item != T::WireType::default() {
                let domain_item = T::from_wire(wire_item, *keyset_id_obj, i);
                result.items[i] = Some(domain_item);
            }
        }

        Ok(result)
    }
}

// TODO replace existing types with these
// pub type BlindedMessageSet = DomainArray<BlindedMessage>;
// pub type Sv2BlindedMessageSetWire<'decoder> = WireArray<'decoder>;

impl<'decoder> DomainItem<'decoder> for BlindedMessage {
    type WireType = Sv2BlindedMessage<'decoder>;

    fn from_wire(
        wire_obj: Self::WireType,
        keyset_id: cdk::nuts::nut02::Id,
        amount_index: usize,
    ) -> Self {
        let amount = Amount::from(index_to_amount(amount_index));
        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if wire_obj.parity_bit { 0x03 } else { 0x02 };
        pubkey_bytes[1..].copy_from_slice(&wire_obj.blinded_secret.inner_as_ref());

        let blinded_secret =
            cdk::nuts::PublicKey::from_slice(&pubkey_bytes).expect("Invalid pubkey bytes");

        BlindedMessage {
            amount,
            keyset_id,
            blinded_secret,
            witness: None,
        }
    }

    fn to_wire(&self) -> Self::WireType {
        let mut pubkey_bytes = self.blinded_secret.to_bytes();
        let parity_bit = pubkey_bytes[0] == 0x03;
        let pubkey_data = &mut pubkey_bytes[1..];

        Sv2BlindedMessage {
            parity_bit,
            blinded_secret: PubKey::from_bytes(pubkey_data)
                .expect("Invalid pubkey data")
                .into_static(),
        }
    }

    fn get_amount(&self) -> u64 {
        self.amount.into()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use rand::Rng;

    fn get_random_signing_key() -> Sv2SigningKey<'static> {
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
        let mut rng = rand::thread_rng();

        let mut signature_bytes = [0u8; 32];
        rng.fill(&mut signature_bytes[..]);

        Sv2BlindSignature {
            blind_signature: PubKey::from_bytes(&mut signature_bytes).unwrap().into_static(),
            parity_bit: rng.gen(),
        }
    }

    fn get_random_sigset() -> Sv2BlindSignatureSet<'static> {
        let mut rng = rand::thread_rng();

        let mut sigs: [Sv2BlindSignature<'static>; 64] = array::from_fn(|_| get_random_signature());
        for i in 0..64 {
            sigs[i] = get_random_signature();
        }

        Sv2BlindSignatureSet {
            keyset_id: rng.gen::<u64>(),
            signatures: sigs,
        }
    }

    fn get_random_blinded_message() -> Sv2BlindedMessage<'static> {
        let mut rng = rand::thread_rng();

        let mut pubkey_bytes = [0u8; 32];
        rng.fill(&mut pubkey_bytes[..]);

        Sv2BlindedMessage {
            blinded_secret: PubKey::from_bytes(&mut pubkey_bytes).unwrap().into_static(),
            parity_bit: rng.gen(),
        }
    }

    fn get_random_msgset() -> Sv2BlindedMessageSet<'static> {
        let mut rng = rand::thread_rng();

        let mut messages: [Sv2BlindedMessage<'static>; 64] = array::from_fn(|_| get_random_blinded_message());
        for i in 0..64 {
            messages[i] = get_random_blinded_message();
        }

        Sv2BlindedMessageSet {
            keyset_id: rng.gen::<u64>(),
            messages,
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

        assert_eq!(wire_sigset.keyset_id, domain_sigset.keyset_id);
        assert_eq!(original_sigset.signatures, domain_sigset.signatures);
    }

    #[test]
    fn test_sv2_blinded_msg_set_domain_wire_conversion() {
        let original_msgset = get_random_msgset();
        let wire_msgset: Sv2BlindedMessageSetWire = original_msgset.clone().into();
        let domain_msgset: Sv2BlindedMessageSet = wire_msgset.clone().try_into().unwrap();

        assert_eq!(wire_msgset.keyset_id, domain_msgset.keyset_id);
        assert_eq!(original_msgset.messages, domain_msgset.messages);
    }
}