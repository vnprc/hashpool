//! Cashu Extension for Stratum V2
//! 
//! This extension adds ecash/Cashu mint integration to the Stratum V2 protocol.
//! It uses TLV (Type-Length-Value) fields to extend existing messages without
//! modifying the core protocol.

#![no_std]

#[cfg(feature = "with_serde")]
extern crate alloc;

pub mod tlv;
pub mod negotiation;
pub mod hash_computation;
pub mod message_utils;

pub use tlv::{CashuTlvParser, CashuTlvEncoder, CashuExtensionFields};
pub use negotiation::{RequestExtensions, RequestExtensionsSuccess, RequestExtensionsError, ExtensionState, ExtensionNegotiator};
pub use hash_computation::compute_share_hash;
pub use message_utils::{append_cashu_tlv_to_message, extract_cashu_tlv_from_message, calculate_core_message_size};

/// Extension ID for Cashu integration
pub const CASHU_EXTENSION_ID: u16 = 0x0003;

/// Field types within the Cashu extension
pub const FIELD_TYPE_LOCKING_PUBKEY: u8 = 0x01;