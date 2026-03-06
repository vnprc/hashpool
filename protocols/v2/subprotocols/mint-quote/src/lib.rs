//! # Stratum V2 Mint Quote Protocol Messages
//!
//! SV2 message types for communication between mining pools and mint services.

pub use binary_sv2::binary_codec_sv2::{self, Decodable as Deserialize, Encodable as Serialize, *};
pub use derive_codec_sv2::{Decodable as Deserialize, Encodable as Serialize};

use core::convert::TryInto;

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
