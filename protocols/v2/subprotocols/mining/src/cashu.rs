use cdk::{amount::Amount, nuts::{BlindSignature, BlindedMessage, CurrencyUnit, KeySet, PreMintSecrets, PublicKey, Keys}};
use rand::RngCore;
use secp256k1::{Secp256k1, SecretKey, PublicKey as SecpPublicKey};
use core::array;
use std::{collections::BTreeMap, convert::{TryFrom, TryInto}};
pub use std::error::Error;
use tracing::warn;

#[cfg(not(feature = "with_serde"))]
pub use binary_sv2::binary_codec_sv2::{self, Decodable as Deserialize, Encodable as Serialize, *};
#[cfg(not(feature = "with_serde"))]
pub use derive_codec_sv2::{Decodable as Deserialize, Encodable as Serialize};

#[derive(Debug)]
pub enum CashuConversionError {
    SeqExceedsMaxSize { actual: usize, max: usize },
    ReadError { actual: usize, expected: usize },
    DuplicateAmountIndex { index: usize },
}

impl std::fmt::Display for CashuConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use CashuConversionError::*;
        match self {
            SeqExceedsMaxSize { actual, max } => {
                write!(f, "Sequence exceeds max size: got {}, max is {}", actual, max)
            }
            ReadError { actual, expected } => {
                write!(f, "Read error: got {}, expected at least {}", actual, expected)
            }
            DuplicateAmountIndex { index } => {
                write!(f, "Duplicate blinded message at index {}", index)
            }
        }
    }
}

impl std::error::Error for CashuConversionError {}

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
    type Error = CashuConversionError;

    fn try_from(pre_mint_secrets: PreMintSecrets) -> Result<Self, Self::Error> {
        let mut items: [Option<BlindedMessage>; NUM_MESSAGES] = core::array::from_fn(|_| None);

        for pre_mint in &pre_mint_secrets.secrets {
            let index = amount_to_index(pre_mint.amount.into());
            if items[index].is_some() {
                return Err(CashuConversionError::DuplicateAmountIndex { index: index });
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
use cdk::nuts::nutXX::MintQuoteMiningShareRequest;

pub fn format_quote_event_json(req: &MintQuoteMiningShareRequest, msgs: &[BlindedMessage]) -> String {
    use std::fmt::Write;
    use cdk::nuts::CurrencyUnit;
    use cdk::util::hex;
    use serde_json;

    let mut out = String::new();
    out.push_str("{\"quote_request\":{");

    match &req.unit {
        CurrencyUnit::Custom(s) => write!(out, "\"unit\":\"{}\",", s).unwrap(),
        currency_unit => write!(out, "\"unit\":\"{}\",", currency_unit).unwrap(),
    }

    write!(
        out,
        "\"amount\":{},\"header_hash\":\"{}\",",
        req.amount.to_string(),
        hex::encode(req.header_hash.to_byte_array())
    ).unwrap();

    match &req.description {
        Some(d) => write!(out, "\"description\":\"{}\",", d).unwrap(),
        None => write!(out, "\"description\":null,").unwrap(),
    }

    match &req.pubkey {
        Some(pk) => write!(out, "\"pubkey\":\"{}\"", hex::encode(pk.to_bytes())).unwrap(),
        None => write!(out, "\"pubkey\":null").unwrap(),
    }

    out.push_str("},\"blinded_messages\":[");
    for (i, m) in msgs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(
            out,
            "{{\"amount\":{},\"id\":\"{}\",\"B_\":\"{}\",\"witness\":",
            m.amount.to_string(),
            hex::encode(m.keyset_id.to_bytes()),
            hex::encode(m.blinded_secret.to_bytes())
        ).unwrap();

        match &m.witness {
            Some(w) => {
                let json = serde_json::to_value(w).unwrap();
                write!(out, "{}", json).unwrap();
                out.push('}');
            }
            None => out.push_str("null}"),
        }
    }
    out.push_str("]}");
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
            let id = cdk::nuts::nut02::Id::from(&keys_map);
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

#[test]
fn test_sv2_keyset_roundtrip() {
    use super::*;
    use cdk::nuts::{KeySet, Keys, PublicKey};
    use cdk::amount::Amount;

    // Build dummy Keys map
    let mut map = BTreeMap::new();
    for i in 0..64 {
        let amount = Amount::from(1u64 << i);
        let pubkey = PublicKey::from_slice(&[0x02; 33]).unwrap();
        map.insert(amount, pubkey);
    }

    // Construct KeySet
    let keys = Keys::new(map);
    let id = cdk::nuts::nut02::Id::from(&keys);
    let keyset = KeySet {
        id,
        unit: CurrencyUnit::Custom("HASH".to_string()),
        keys,
    };

    // Convert KeySet to Sv2KeySet and back
    let sv2: Sv2KeySet = keyset.clone().try_into().expect("Conversion to Sv2KeySet failed");
    let roundtrip: KeySet = sv2.try_into().expect("Conversion back to KeySet failed");

    // Confirm roundtrip ID and key equality
    assert_eq!(keyset.id.to_bytes(), roundtrip.id.to_bytes());
    let original_keys: BTreeMap<_, _> = keyset.keys.iter().map(|(k, v)| (*k, *v)).collect();
    let roundtrip_keys: BTreeMap<_, _> = roundtrip.keys.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(original_keys, roundtrip_keys);
}

#[test]
fn test_sv2_signing_keys_to_keys_outputs_valid_keys() {
    let sv2_keyset = test_sv2_keyset();
    let keys = sv2_signing_keys_to_keys(&sv2_keyset.keys).expect("Should produce valid Keys");

    // Convert to BTreeMap for structure validation
    let map: BTreeMap<_, _> = keys.iter().map(|(k, v)| (*k, *v)).collect();

    assert_eq!(map.len(), sv2_keyset.keys.len());
    for k in sv2_keyset.keys.iter() {
        assert!(map.contains_key(&Amount::from(k.amount)));
    }
}

#[test]
fn test_calculate_keyset_id_consistency() {
    let sv2_keyset = test_sv2_keyset();
    let id = calculate_keyset_id(&sv2_keyset.keys);
    assert_ne!(id, 0, "Keyset ID must not default to 0");
}

#[test]
fn test_format_quote_event_json() {
    use cdk::amount::Amount;
    use cdk::nuts::nutXX::MintQuoteMiningShareRequest;
    use cdk::nuts::{BlindedMessage, CurrencyUnit, PublicKey};
    use bitcoin_hashes::sha256;

    // Construct a fake SHA256 header hash
    let hash = sha256::Hash::hash(b"test");

    // Construct the request
    let req = MintQuoteMiningShareRequest {
        amount: Amount::from(1000u64),
        unit: CurrencyUnit::Custom("HASH".to_string()),
        header_hash: hash,
        description: Some("test quote".to_string()),
        pubkey: None,
    };

    // Construct the blinded message
    let blinded_secret = PublicKey::from_slice(&[0x02; 33]).unwrap();
    let blinded_msg = BlindedMessage {
        amount: Amount::from(1000u64),
        keyset_id: cdk::nuts::nut02::Id::from(&cdk::nuts::Keys::new(BTreeMap::new())),
        blinded_secret,
        witness: None,
    };

    let output = format_quote_event_json(&req, &[blinded_msg]);

    assert!(output.contains("test quote"));
    assert!(output.contains("HASH"));
}

fn test_sv2_keyset() -> Sv2KeySet<'static> {
    use secp256k1::{Secp256k1, SecretKey, PublicKey};
    use rand::{Rng, RngCore};

    let secp = Secp256k1::new();
    let mut rng = rand::thread_rng();

    let keys = array::from_fn(|i| {
        // Generate a valid private key
        let mut bytes = [0u8; 32];
        rng.try_fill_bytes(&mut bytes);
        let sk = SecretKey::from_slice(&bytes).unwrap();

        // Get its compressed public key
        let pk = PublicKey::from_secret_key(&secp, &sk);
        let pubkey_bytes = pk.serialize(); // [u8; 33]

        let parity_bit = pubkey_bytes[0] == 0x03;
        let inner_bytes: [u8; 32] = pubkey_bytes[1..].try_into().unwrap();

        Sv2SigningKey {
            amount: 1u64 << i,
            parity_bit,
            pubkey: PubKey::from_bytes(&mut inner_bytes.clone()).unwrap().into_static(),
        }
    });

    Sv2KeySet {
        id: rng.gen(),
        keys,
    }
}

#[test]
fn test_blinded_message_array_roundtrip() {
    use super::*;
    use std::collections::BTreeMap;
    use cdk::nuts::Keys;

    // Derive a deterministic (but throw-away) key-set ID
    let keyset = Keys::new(BTreeMap::new());
    let keyset_id_obj = cdk::nuts::nut02::Id::from(&keyset);
    let keyset_id_u64: u64 = KeysetId(keyset_id_obj).into();

    // Build domain array with 64 blinded messages
    let mut domain = BlindedMessageSet::new(keyset_id_u64);
    for i in 0..NUM_MESSAGES {
        let amount = Amount::from(index_to_amount(i));
        domain.insert(make_blinded_message(amount, keyset_id_obj));
    }

    // Domain → wire → domain
    let wire: Sv2BlindedMessageSetWire = domain.clone().into();
    let roundtrip: BlindedMessageSet =
        wire.try_into().expect("wire-to-domain decoding must succeed");

    assert_eq!(domain, roundtrip, "round-trip altered message set");
}

#[test]
fn test_amount_index_roundtrip() {
    for i in 0..NUM_MESSAGES {
        let amt = index_to_amount(i);
        assert_eq!(amount_to_index(amt), i);
    }
}

#[test]
fn test_calculate_work_leading_zeros() {
    // 0x00…00 -> 256 leading zeros
    assert_eq!(calculate_work([0u8; 32]), 256);
    // 0x00…01 -> 255
    let mut one = [0u8; 32];
    one[31] = 1;
    assert_eq!(calculate_work(one), 255);
    // 0x00…10 -> 31 zero bytes (248 bits) + 3 leading-zero bits = 251
    let mut sixteen = [0u8; 32];
    sixteen[31] = 0x10;
    assert_eq!(calculate_work(sixteen), 251);
}

#[test]
fn test_blind_signature_array_roundtrip() {
    let keyset_id = cdk::nuts::nut02::Id::from(&cdk::nuts::Keys::new(BTreeMap::new()));
    let ks_u64: u64 = KeysetId(keyset_id).into();

    let mut domain = BlindSignatureSet::new(ks_u64);
    for i in 0..NUM_MESSAGES {
        let amt = Amount::from(index_to_amount(i));
        domain.insert(make_blind_signature_for_amount(amt, keyset_id));
    }

    let wire: Sv2BlindSignatureSetWire = domain.clone().into();
    let roundtrip: BlindSignatureSet = wire.try_into().unwrap();
    assert_eq!(domain, roundtrip);
}

#[test]
fn test_domain_array_insert_and_get() {
    let keyset_id = cdk::nuts::nut02::Id::from(&cdk::nuts::Keys::new(BTreeMap::new()));
    let id_u64: u64 = KeysetId(keyset_id).into();

    let mut domain = BlindedMessageSet::new(id_u64);
    let amount = Amount::from(8u64); // 2^3
    let bm = make_blinded_message_for_amount(amount, keyset_id);
    domain.insert(bm.clone());

    assert!(domain.get(8).is_some());
    assert_eq!(domain.get(8).unwrap(), &bm);
}

#[test]
fn test_sv2_keyset_wire_roundtrip() {
    let sv2 = test_sv2_keyset();          // helper from existing code
    let wire: Sv2KeySetWire = (&sv2.keys).try_into().unwrap();
    let domain: Sv2KeySet = wire.clone().try_into().unwrap();
    let wire2: Sv2KeySetWire = (&domain.keys).try_into().unwrap();
    assert_eq!(wire, wire2);
}

/// Generate a valid `BlindedMessage` for the given `amount` and `keyset_id`.
fn make_blinded_message(
    amount: Amount,
    keyset_id: cdk::nuts::nut02::Id,
) -> BlindedMessage {
    let secp = Secp256k1::new();
    let mut rng = rand::thread_rng();

    // Fresh secret key → compressed pubkey → Cashu `PublicKey`
    let sk = fresh_secret_key(&mut rng);
    let pk = SecpPublicKey::from_secret_key(&secp, &sk);
    let blinded_secret =
        PublicKey::from_slice(&pk.serialize()).expect("compressed pubkey is always 33 B");

    BlindedMessage {
        amount,
        keyset_id,
        blinded_secret,
        witness: None,
    }
}

/// Generate a valid `secp256k1::SecretKey`.
fn fresh_secret_key(rng: &mut impl RngCore) -> SecretKey {
    loop {
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);

        // `from_slice` returns Err if `bytes` ≥ group order or zero.
        if let Ok(sk) = SecretKey::from_slice(&bytes) {
            return sk;
        }
    }
}

fn make_pubkey() -> cdk::nuts::PublicKey {
    use secp256k1::{Secp256k1, PublicKey as SecpPublicKey};
    let secp = Secp256k1::new();
    let mut rng = rand::thread_rng();
    let sk = fresh_secret_key(&mut rng);
    let pk: SecpPublicKey = SecpPublicKey::from_secret_key(&secp, &sk);
    cdk::nuts::PublicKey::from_slice(&pk.serialize()).unwrap()
}

fn make_blinded_message_for_amount(
    amount: Amount,
    keyset_id: cdk::nuts::nut02::Id,
) -> BlindedMessage {
    BlindedMessage {
        amount,
        keyset_id,
        blinded_secret: make_pubkey(),
        witness: None,
    }
}

fn make_blind_signature_for_amount(
    amount: Amount,
    keyset_id: cdk::nuts::nut02::Id,
) -> BlindSignature {
    BlindSignature {
        amount,
        keyset_id,
        c: make_pubkey(),
        dleq: None,
    }
}