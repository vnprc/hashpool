use cdk::{amount::{Amount, AmountStr}, nuts::{BlindSignature, BlindedMessage, CurrencyUnit, KeySet, PreMintSecrets, PublicKey}};
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

pub type BlindedMessageSet = DomainArray<BlindedMessage>;
pub type Sv2BlindedMessageSetWire<'decoder> = WireArray<'decoder>;

impl TryFrom<PreMintSecrets> for BlindedMessageSet {
    type Error = binary_sv2::Error;

    fn try_from(pre_mint_secrets: PreMintSecrets) -> Result<Self, Self::Error> {
        let mut items: [Option<BlindedMessage>; NUM_MESSAGES] = core::array::from_fn(|_| None);

        for pre_mint in &pre_mint_secrets.secrets {
            let index = amount_to_index(pre_mint.amount.into());
            if items[index].is_some() {
                // TODO use better error
                return Err(binary_sv2::Error::DecodableConversionError);
            }
            items[index] = Some(pre_mint.blinded_message.clone());
        }

        Ok(BlindedMessageSet {
            keyset_id: u64::from(KeysetId(pre_mint_secrets.keyset_id)),
            items,
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

pub type BlindSignatureSet = DomainArray<BlindSignature>;
pub type Sv2BlindSignatureSetWire<'decoder> = WireArray<'decoder>;

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
const NUM_MESSAGES: usize = 64;

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
    pub items: [Option<T>; NUM_MESSAGES],
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireArray<'decoder> {
    pub keyset_id: u64,
    // WARNING you can't call this field 'data'
    // or you get obscure compile errors unrelated to the field name
    pub encoded_data: B064K<'decoder>,
}

impl<'a> Default for WireArray<'a> {
    fn default() -> Self {
        Self {
            keyset_id: 0,
            encoded_data: B064K::Owned(Vec::new()),
        }
    }
}

impl<T> From<DomainArray<T>> for WireArray<'_> 
where
    for<'d> T: DomainItem<'d>,
{
    fn from(domain: DomainArray<T>) -> Self {
        let mut buffer = vec![0u8; WIRE_ITEM_SIZE * NUM_MESSAGES];

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
            encoded_data: b064k,
        }
    }
}

impl<T> TryFrom<WireArray<'_>> for DomainArray<T>
where
    for <'d> T: DomainItem<'d>,
{
    type Error = binary_sv2::Error;

    fn try_from(wire: WireArray<'_>) -> Result<Self, Self::Error> {
        let raw = wire.encoded_data.inner_as_ref();
        // TODO evaluate T::WireType::SIZE as an alternative to this constant
        let expected_len = WIRE_ITEM_SIZE * NUM_MESSAGES;
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

impl<'decoder> DomainItem<'decoder> for BlindSignature {
    type WireType = Sv2BlindSignature<'decoder>;

    fn from_wire(
        wire_obj: Self::WireType,
        keyset_id: cdk::nuts::nut02::Id,
        amount_index: usize,
    ) -> Self {
        let amount = Amount::from(index_to_amount(amount_index));
        let mut pubkey_bytes = [0u8; 33];
        pubkey_bytes[0] = if wire_obj.parity_bit { 0x03 } else { 0x02 };
        pubkey_bytes[1..].copy_from_slice(&wire_obj.blind_signature.inner_as_ref());

        let signature =
            cdk::nuts::PublicKey::from_slice(&pubkey_bytes).expect("Invalid pubkey bytes");

        BlindSignature {
            amount,
            keyset_id,
            c: signature,
            dleq: None,
        }
    }

    fn to_wire(&self) -> Self::WireType {
        let mut pubkey_bytes = self.c.to_bytes();
        let parity_bit = pubkey_bytes[0] == 0x03;
        let pubkey_data = &mut pubkey_bytes[1..];

        Sv2BlindSignature {
            parity_bit,
            blind_signature: PubKey::from_bytes(pubkey_data)
                .expect("Invalid pubkey data")
                .into_static(),
        }
    }

    fn get_amount(&self) -> u64 {
        self.amount.into()
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
use cdk::nuts::nut04::MintQuoteMiningShareRequest;

pub fn format_quote_event_json(req: &MintQuoteMiningShareRequest, msgs: &[BlindedMessage]) -> String {
    use std::fmt::Write;
    use cdk::nuts:: CurrencyUnit;
    use cdk::util::hex;
    use serde_json;

    let mut quote_part = String::new();
    {
        let w: &mut dyn Write = &mut quote_part;

        write!(w, "{{\"amount\":{},", req.amount.to_string()).unwrap();

        match &req.unit {
            CurrencyUnit::Custom(s) => write!(w, "\"unit\":\"{}\",", s).unwrap(),
            currency_unit => write!(w, "\"unit\":\"{}\",", currency_unit).unwrap(),
        }

        write!(
            w,
            "\"header_hash\":\"{}\",",
            hex::encode(req.header_hash.to_byte_array())
        )
        .unwrap();

        match &req.description {
            Some(d) => write!(w, "\"description\":\"{}\",", d).unwrap(),
            None => write!(w, "\"description\":null,").unwrap(),
        }

        match &req.pubkey {
            Some(pk) => write!(w, "\"pubkey\":\"{}\"", hex::encode(pk.to_bytes())).unwrap(),
            None => write!(w, "\"pubkey\":null").unwrap(),
        }
    }

    let mut out = String::new();
    out.push_str("{\"quote_request\":");
    out.push_str(&quote_part);
    out.push_str(",\"blinded_messages\":[");

    for (i, m) in msgs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }

        write!(
            out,
            "{{\"amount\":{},\"keyset_id\":\"{}\",\"blinded_secret\":\"{}\",\"witness\":",
            m.amount.to_string(),
            hex::encode(m.keyset_id.to_bytes()),
            hex::encode(m.blinded_secret.to_bytes())
        )
        .unwrap();

        match &m.witness {
            Some(w) => {
                let json = serde_json::to_value(w).unwrap();
                write!(out, "{}", json).unwrap();
                out.push('}');
            }
            None => out.push_str("null}"),
        }
    }

    out.push_str("]}}");
    out
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
    
        let mut keys: [Sv2SigningKey<'static>; NUM_MESSAGES] = array::from_fn(|_| get_random_signing_key());
        for i in 0..NUM_MESSAGES {
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

        let mut sigs: [Sv2BlindSignature<'static>; NUM_MESSAGES] = array::from_fn(|_| get_random_signature());
        for i in 0..NUM_MESSAGES {
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

        let mut messages: [Sv2BlindedMessage<'static>; NUM_MESSAGES] = array::from_fn(|_| get_random_blinded_message());
        for i in 0..NUM_MESSAGES {
            messages[i] = get_random_blinded_message();
        }

        Sv2BlindedMessageSet {
            keyset_id: rng.gen::<u64>(),
            items: messages,
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
        assert_eq!(original_msgset.items, domain_msgset.items);
    }
}