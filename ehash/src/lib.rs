//! External Ehash Extension for Hashpool
//!
//! This crate provides a clean external extension for adding ehash/ecash functionality
//! to Stratum V2 mining pools without modifying core SRI protocol implementations.
//!
//! The extension works by intercepting SRI messages at the byte level, appending
//! TLV (Type-Length-Value) fields for ehash-specific data like locking pubkeys.

#![no_std]

extern crate alloc;

use core::fmt;

// Make these available for tests
#[cfg(test)]
use alloc::{vec, vec::Vec};

// Re-export TLV infrastructure from internal crate
pub use cashu_extension_sv2::{
    CashuTlvParser, CashuTlvEncoder, CashuExtensionFields, 
    CASHU_EXTENSION_ID, FIELD_TYPE_LOCKING_PUBKEY,
    compute_share_hash, append_cashu_tlv_to_message, extract_cashu_tlv_from_message,
    calculate_core_message_size
};

pub mod interceptor;
pub mod message_detector;
pub mod extension_state;

pub use interceptor::{MessageInterceptor, EhashMessageInterceptor};
pub use message_detector::MessageTypeDetector;
pub use extension_state::ExtensionState;

/// Errors that can occur during message interception
#[derive(Debug, Clone)]
pub enum InterceptorError {
    /// Failed to parse message header
    InvalidMessageHeader,
    /// Failed to detect message type
    UnknownMessageType,
    /// TLV encoding/decoding failed
    TlvError(alloc::string::String),
    /// Message too short to contain required data
    InsufficientData,
    /// Extension not negotiated for this connection
    ExtensionNotNegotiated,
}

impl fmt::Display for InterceptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMessageHeader => write!(f, "Invalid message header"),
            Self::UnknownMessageType => write!(f, "Unknown message type"),
            Self::TlvError(msg) => write!(f, "TLV error: {}", msg),
            Self::InsufficientData => write!(f, "Insufficient data"),
            Self::ExtensionNotNegotiated => write!(f, "Extension not negotiated"),
        }
    }
}

/// Extension data extracted from messages
#[derive(Debug, Clone, Default)]
pub struct ExtensionData {
    /// Ehash-specific fields (reusing Cashu TLV infrastructure)
    pub ehash_fields: CashuExtensionFields,
}

/// Result type for interceptor operations
pub type InterceptorResult<T> = Result<T, InterceptorError>;