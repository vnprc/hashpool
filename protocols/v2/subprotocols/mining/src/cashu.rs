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

/// Convert SV2 keyset bytes to CDK keyset ID with proper version detection
pub fn keyset_from_sv2_bytes(keyset_bytes: &[u8]) -> Result<cdk::nuts::nut02::Id, cdk::nuts::nut02::Error> {
    tracing::debug!("Converting keyset from {} bytes: {}", keyset_bytes.len(), cdk::util::hex::encode(keyset_bytes));
    
    if keyset_bytes.is_empty() {
        // Return a default Version00 keyset with all zeros
        let placeholder = [0u8; 8];
        tracing::debug!("Empty keyset bytes, returning placeholder: {}", cdk::util::hex::encode(&placeholder));
        return cdk::nuts::nut02::Id::from_bytes(&placeholder);
    }
    
    // Check if this looks like a real keyset (has any non-zero bytes)
    let has_real_data = keyset_bytes.iter().any(|&x| x != 0);
    
    if !has_real_data {
        // All zeros - create a Version00 placeholder
        let placeholder = [0u8; 8];
        return cdk::nuts::nut02::Id::from_bytes(&placeholder);
    }
    
    // Determine the expected format based on length and content
    let result = match keyset_bytes.len() {
        8 => {
            // Could be Version00 (1 + 7 bytes) - try parsing directly
            tracing::debug!("Parsing 8-byte keyset as Version00");
            cdk::nuts::nut02::Id::from_bytes(keyset_bytes)
        }
        33 => {
            // Could be Version01 (1 + 32 bytes) - try parsing directly  
            tracing::debug!("Parsing 33-byte keyset as Version01");
            cdk::nuts::nut02::Id::from_bytes(keyset_bytes)
        }
        len if len >= 8 => {
            // Take last 8 bytes and try as Version00 (keyset is right-padded)
            let mut bytes = [0u8; 8];
            let start = len - 8;
            bytes.copy_from_slice(&keyset_bytes[start..]);
            tracing::debug!("Truncating {}-byte keyset to 8 bytes for Version00: {}", len, cdk::util::hex::encode(&bytes));
            cdk::nuts::nut02::Id::from_bytes(&bytes)
        }
        _ => {
            // Too short, pad to 8 bytes for Version00
            let mut bytes = [0u8; 8];
            bytes[..keyset_bytes.len()].copy_from_slice(keyset_bytes);
            tracing::debug!("Padding {}-byte keyset to 8 bytes for Version00: {}", keyset_bytes.len(), cdk::util::hex::encode(&bytes));
            cdk::nuts::nut02::Id::from_bytes(&bytes)
        }
    };
    
    match &result {
        Ok(id) => tracing::debug!("Successfully converted keyset to ID: {}", cdk::util::hex::encode(id.to_bytes())),
        Err(e) => tracing::warn!("Failed to convert keyset: {:?}", e),
    }
    
    result
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
            unit: CurrencyUnit::Hash,
            keys: cdk::nuts::Keys::new(keys_map),
            final_expiry: None,
        })
    }
}

// TODO find a better place for this
// TODO make configurable
pub fn calculate_work(hash: [u8; 32]) -> u64 {
    calculate_work_in_range(hash, 1, 64)
}

/// Calculate work using exponential valuation (2^n) where n is number of leading zero bits.
/// - Each additional zero bit is exponentially more difficult to find
/// - Shares should be valued proportionally to their computational difficulty
///
/// Parameters:
/// - hash: The 32-byte hash to analyze
/// - min_leading_zeros: Minimum number of leading zero bits required (e.g., 36 for mainnet)
/// - max_representable_bits: Maximum representable difficulty bits (e.g., 64 to compress from 256)
///
/// Returns the exponential work value: 2^(min(leading_zeros, max_representable_bits))
pub fn calculate_work_in_range(hash: [u8; 32], min_leading_zeros: u32, max_representable_bits: u32) -> u64 {
    let leading_zero_bits = count_leading_zero_bits(hash);

    // Only count work above the minimum threshold
    if leading_zero_bits < min_leading_zeros {
        return 0; // Below minimum difficulty, no reward
    }

    // Cap at maximum representable bits to prevent overflow
    let effective_bits = std::cmp::min(leading_zero_bits, max_representable_bits);

    // Use exponential valuation: 2^n where n is effective difficulty bits
    // For values above 63, we'd overflow u64, so cap at 2^63
    if effective_bits >= 63 {
        1u64 << 63 // Maximum representable value in u64
    } else {
        1u64 << effective_bits
    }
}

/// Count the number of leading zero bits in a hash
pub fn count_leading_zero_bits(hash: [u8; 32]) -> u32 {
    let mut count = 0u32;

    for byte in hash {
        if byte == 0 {
            count += 8; // Each zero byte adds 8 bits
        } else {
            // Count the leading zeros in the current byte
            count += byte.leading_zeros();
            break; // Stop counting after the first non-zero byte
        }
    }

    count
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
            unit: CurrencyUnit::Hash,
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
            unit: CurrencyUnit::Hash,
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
        // All zeros: 256 leading zero bits, capped at 64, gets 2^63 (max u64 value we can represent)
        assert_eq!(calculate_work([0u8; 32]), 1u64 << 63);

        // 255 leading zeros (one bit set at the end): gets 2^63 (still capped)
        let mut one = [0u8; 32];
        one[31] = 1;
        assert_eq!(calculate_work(one), 1u64 << 63);

        // 251 leading zeros (0x10 = bit 4 set): gets 2^63 (still capped)
        let mut sixteen = [0u8; 32];
        sixteen[31] = 0x10;
        assert_eq!(calculate_work(sixteen), 1u64 << 63);

        // Test configurable function with lower thresholds
        // 40 leading zeros: bytes 0-4 are zero (40 bits), byte 5 has bit set
        let mut test_hash = [0u8; 32];
        test_hash[5] = 1; // First non-zero bit is at bit 47 (5*8 + 7), so 47 leading zeros
        assert_eq!(calculate_work_in_range(test_hash, 36, 50), 1u64 << 47);

        // Below minimum: should get 0
        let mut low_hash = [0u8; 32];
        low_hash[3] = 1; // 3*8 + 7 = 31 leading zeros, below minimum of 36
        assert_eq!(calculate_work_in_range(low_hash, 36, 50), 0);
    }

    #[test]
    fn test_count_leading_zero_bits() {
        // All zeros should have 256 leading zero bits
        assert_eq!(count_leading_zero_bits([0u8; 32]), 256);

        let mut one = [0u8; 32];
        one[31] = 1;
        assert_eq!(count_leading_zero_bits(one), 255);

        // 0x10 in last byte: 4 leading zeros in that byte + 31*8 from previous bytes = 251
        let mut sixteen = [0u8; 32];
        sixteen[31] = 0x10;
        assert_eq!(count_leading_zero_bits(sixteen), 251);

        // First bit set should have 0 leading zeros
        let mut first_bit = [0u8; 32];
        first_bit[0] = 0x80;
        assert_eq!(count_leading_zero_bits(first_bit), 0);

        // Second bit set should have 1 leading zero
        let mut second_bit = [0u8; 32];
        second_bit[0] = 0x40;
        assert_eq!(count_leading_zero_bits(second_bit), 1);
    }

    #[test]
    fn test_exponential_valuation() {
        // Test that each additional zero bit doubles the reward
        // Use lower thresholds to avoid overflow in tests

        // 10 leading zeros: 2^10 = 1024
        let mut hash1 = [0u8; 32];
        hash1[1] = 0x40; // Sets bit 6 of byte 1: 8 + 1 = 9 leading zeros
        hash1[1] = 0x20; // Sets bit 5 of byte 1: 8 + 2 = 10 leading zeros
        assert_eq!(calculate_work_in_range(hash1, 5, 20), 1u64 << 10);

        // 11 leading zeros: 2^11 = 2048 (double the previous)
        let mut hash2 = [0u8; 32];
        hash2[1] = 0x10; // Sets bit 4 of byte 1: 8 + 3 = 11 leading zeros
        assert_eq!(calculate_work_in_range(hash2, 5, 20), 1u64 << 11);
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