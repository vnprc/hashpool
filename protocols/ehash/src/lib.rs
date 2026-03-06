//! Shared helpers for Hashpool ehash calculations.
//!
//! Keeping these utilities in a dedicated crate minimizes the amount of
//! Cashu-specific logic that needs to live inside the upstream Stratum V2
//! protocol crates.

// Re-export binary_sv2 as a named module so derive_codec_sv2 1.1.2 generated code
// (which uses `super::binary_sv2::...` paths) can resolve correctly.
pub use binary_sv2;

use std::fmt;

pub mod keyset;
pub mod locking_key;
pub mod message_type;
pub mod quote;
pub mod share;
pub mod sv2;
pub mod work;

pub use keyset::{
    build_cdk_keyset, calculate_keyset_id, keyset_from_sv2_bytes, signing_keys_from_cdk,
    signing_keys_to_cdk, KeysetConversionError, KeysetId, SigningKey,
};
pub use locking_key::{parse_locking_key, LockingKeyError};
pub use message_type::{MessageType, MessageTypeError, MintQuoteMessage};
pub use quote::{
    build_mint_quote_request, mint_quote_response_from_cdk, parse_mint_quote_request,
    validate_header_hash, HeaderHashError, ParsedMintQuoteRequest, QuoteBuildError,
    QuoteConversionError, QuoteParseError,
};
pub use share::{ShareHash, ShareHashError};
pub use sv2::{Sv2KeySet, Sv2KeySetWire, Sv2SigningKey};
pub use work::{calculate_difficulty, calculate_ehash_amount};

/// Errors that can occur during ehash quote dispatch operations.
///
/// These errors represent failures in the quote dispatch pipeline, from
/// locking key validation through quote submission to the mint service.
#[derive(Debug, Clone)]
pub enum QuoteDispatchError {
    /// Locking key is missing for the channel
    MissingLockingKey(u32),
    /// Locking key has invalid format or length
    InvalidLockingKeyFormat { channel_id: u32, length: usize },
    /// Failed to parse locking key as a compressed public key
    InvalidLockingKey { channel_id: u32, reason: String },
    /// Quote dispatcher is not available
    MintDispatcherUnavailable,
    /// Quote dispatch failed with the given error message
    QuoteDispatchFailed(String),
}

impl fmt::Display for QuoteDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLockingKey(channel_id) => {
                write!(f, "Missing locking key for channel {}", channel_id)
            }
            Self::InvalidLockingKeyFormat {
                channel_id,
                length,
            } => {
                write!(
                    f,
                    "Invalid locking key format for channel {}: expected 33 bytes, got {}",
                    channel_id, length
                )
            }
            Self::InvalidLockingKey {
                channel_id,
                reason,
            } => {
                write!(
                    f,
                    "Failed to parse locking key for channel {}: {}",
                    channel_id, reason
                )
            }
            Self::MintDispatcherUnavailable => {
                write!(f, "Mint dispatcher is not available")
            }
            Self::QuoteDispatchFailed(msg) => {
                write!(f, "Quote dispatch failed: {}", msg)
            }
        }
    }
}

impl std::error::Error for QuoteDispatchError {}
