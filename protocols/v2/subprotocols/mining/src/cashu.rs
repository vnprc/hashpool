use cdk::nuts::KeySet;
use core::array;
use std::convert::{TryFrom, TryInto};
use ehash::{calculate_keyset_id, build_cdk_keyset, signing_keys_from_cdk, KeysetConversionError, KeysetId, SigningKey};
pub use std::error::Error;

#[cfg(not(feature = "with_serde"))]
pub use binary_sv2::binary_codec_sv2::{self, Decodable as Deserialize, Encodable as Serialize, *};
#[cfg(not(feature = "with_serde"))]
pub use derive_codec_sv2::{Decodable as Deserialize, Encodable as Serialize};


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

impl<'a> From<SigningKey> for Sv2SigningKey<'a> {
    fn from(key: SigningKey) -> Self {
        Sv2SigningKey {
            amount: key.amount,
            parity_bit: key.parity_bit,
            pubkey: key.pubkey,
        }
    }
}

impl<'a> From<&Sv2SigningKey<'a>> for SigningKey {
    fn from(key: &Sv2SigningKey<'a>) -> Self {
        SigningKey {
            amount: key.amount,
            parity_bit: key.parity_bit,
            pubkey: key.pubkey.clone().into_static(),
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

        let signing_keys: Vec<SigningKey> = keys.iter().map(SigningKey::from).collect();
        Ok(Sv2KeySetWire {
            id: calculate_keyset_id(&signing_keys),
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
    type Error = KeysetConversionError;

    fn try_from(value: KeySet) -> Result<Self, Self::Error> {
        let signing_keys: [SigningKey; 64] = signing_keys_from_cdk(&value)?;
        let KeySet { id: cdk_id, .. } = value;
        let id: u64 = KeysetId(cdk_id).into();
        let sv2_keys_vec = Vec::from(signing_keys)
            .into_iter()
            .map(Sv2SigningKey::from)
            .collect::<Vec<_>>();
        let keys: [Sv2SigningKey<'a>; 64] = sv2_keys_vec
            .try_into()
            .map_err(|_| KeysetConversionError::InvalidKeyCount(0))?;
        Ok(Sv2KeySet { id, keys })
    }
}

impl<'a> TryFrom<Sv2KeySet<'a>> for KeySet {
    type Error = KeysetConversionError;

    fn try_from(value: Sv2KeySet) -> Result<Self, Self::Error> {
        let signing_keys_vec = value.keys.iter().map(SigningKey::from).collect::<Vec<_>>();
        let signing_keys: [SigningKey; 64] = signing_keys_vec
            .try_into()
            .map_err(|_| KeysetConversionError::InvalidKeyCount(0))?;
        build_cdk_keyset(value.id, &signing_keys)
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin_hashes::sha256;
    use cdk::amount::Amount;
    use rand::{Rng, RngCore};
    use secp256k1::{PublicKey as SecpPublicKey, Secp256k1, SecretKey};
    use std::collections::BTreeMap;
    use ehash::{signing_keys_to_cdk, calculate_keyset_id, SigningKey};

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
        let signing_keys: Vec<SigningKey> = sv2_keyset.keys.iter().map(SigningKey::from).collect();
        let keys = signing_keys_to_cdk(&signing_keys).unwrap();
        assert_eq!(keys.len(), sv2_keyset.keys.len());

        for k in sv2_keyset.keys.iter() {
            assert!(keys.contains_key(&Amount::from(k.amount)));
        }
    }

    #[test]
    fn test_calculate_keyset_id_nonzero() {
        let sv2_keyset = test_sv2_keyset();
        let signing_keys: Vec<SigningKey> = sv2_keyset.keys.iter().map(SigningKey::from).collect();
        let id = calculate_keyset_id(&signing_keys);
        assert_ne!(id, 0);
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
