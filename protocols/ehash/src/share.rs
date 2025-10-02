use core::fmt;
use std::convert::TryFrom;

use binary_sv2::{Deserialize, PubKey, U256};
use thiserror::Error;

/// Canonical representation of a 32-byte share header hash used when minting quotes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShareHash([u8; 32]);

impl ShareHash {
    /// Construct a `ShareHash` from a 32-byte array.
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the underlying bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Consume the hash and return the inner array.
    pub const fn into_inner(self) -> [u8; 32] {
        self.0
    }

    /// Convert this hash into an owned `PubKey<'static>`.
    pub fn into_pubkey(self) -> Result<PubKey<'static>, ShareHashError> {
        let mut bytes = self.0;
        PubKey::from_bytes(&mut bytes)
            .map(|pk| pk.into_static())
            .map_err(|_| ShareHashError::InvalidEncoding)
    }

    /// Convert this hash into an owned `U256<'static>`.
    pub fn into_u256(self) -> Result<U256<'static>, ShareHashError> {
        let vec = self.0.to_vec();
        vec.try_into().map_err(|_| ShareHashError::InvalidEncoding)
    }

    /// Build a `ShareHash` from an SV2 `PubKey` value.
    pub fn from_pubkey(value: &PubKey<'_>) -> Result<Self, ShareHashError> {
        ShareHash::try_from(value.inner_as_ref())
    }

    /// Build a `ShareHash` from an SV2 `U256` value.
    pub fn from_u256(value: &U256<'_>) -> Result<Self, ShareHashError> {
        ShareHash::try_from(value.inner_as_ref())
    }
}

impl fmt::Display for ShareHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

/// Errors that can occur while decoding share hashes.
#[derive(Debug, Error)]
pub enum ShareHashError {
    #[error("share hash must be 32 bytes, got {actual}")]
    InvalidLength { actual: usize },
    #[error("share hash is not a valid encoded value")]
    InvalidEncoding,
}

impl TryFrom<&[u8]> for ShareHash {
    type Error = ShareHashError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.len() != 32 {
            return Err(ShareHashError::InvalidLength {
                actual: bytes.len(),
            });
        }
        let mut array = [0u8; 32];
        array.copy_from_slice(bytes);
        Ok(ShareHash(array))
    }
}

impl From<[u8; 32]> for ShareHash {
    fn from(value: [u8; 32]) -> Self {
        ShareHash::new(value)
    }
}

impl From<ShareHash> for [u8; 32] {
    fn from(value: ShareHash) -> Self {
        value.0
    }
}
