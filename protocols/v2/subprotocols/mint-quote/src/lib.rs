//! # Stratum V2 Mint Quote Protocol Messages
//!
//! SV2 message types for communication between mining pools and mint services.

// Re-export binary_sv2 as a named module so derive_codec_sv2 1.1.2 generated code
// (which uses `super::binary_sv2::...` paths) can resolve correctly.
pub use binary_sv2;
pub use binary_sv2::*;
pub use derive_codec_sv2::{Decodable as Deserialize, Encodable as Serialize};

use core::convert::TryInto;

// Mint-Quote subprotocol protocol discriminant
pub const SV2_MINT_QUOTE_PROTOCOL_DISCRIMINANT: u8 = 3;

// Mint-Quote subprotocol message types (0x80-0x82)
pub const MESSAGE_TYPE_MINT_QUOTE_REQUEST: u8 = 0x80;
pub const MESSAGE_TYPE_MINT_QUOTE_RESPONSE: u8 = 0x81;
pub const MESSAGE_TYPE_MINT_QUOTE_ERROR: u8 = 0x82;

// Mint-Quote subprotocol channel bits
pub const CHANNEL_BIT_MINT_QUOTE_REQUEST: bool = true;
pub const CHANNEL_BIT_MINT_QUOTE_RESPONSE: bool = true;
pub const CHANNEL_BIT_MINT_QUOTE_ERROR: bool = true;

/// Type alias for a compressed secp256k1 public key (33 bytes).
pub type CompressedPubKey<'a> = B0255<'a>;

mod mint_quote_error;
mod mint_quote_request;
mod mint_quote_response;
mod notification;

pub use mint_quote_error::MintQuoteError;
pub use mint_quote_request::MintQuoteRequest;
pub use mint_quote_response::MintQuoteResponse;
pub use notification::{
    MintQuoteFailure, MintQuoteNotification, CHANNEL_BIT_MINT_QUOTE_FAILURE,
    CHANNEL_BIT_MINT_QUOTE_NOTIFICATION, MESSAGE_TYPE_MINT_QUOTE_FAILURE,
    MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION,
};
