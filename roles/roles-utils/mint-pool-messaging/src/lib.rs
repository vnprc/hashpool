//! # Mint-Pool Messaging Infrastructure
//!
//! This crate provides the core messaging infrastructure for communication
//! between mining pools and mint services using SV2 messages over MPSC channels.

use std::sync::Arc;

use binary_sv2::{self, CompressedPubKey};
use std::convert::TryFrom;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

pub use ehash::{
    build_mint_quote_request, mint_quote_response_from_cdk, parse_mint_quote_request,
    ParsedMintQuoteRequest, QuoteBuildError, QuoteConversionError, QuoteParseError, ShareHash,
    ShareHashError,
};
pub use mint_quote_sv2::{MintQuoteError, MintQuoteRequest, MintQuoteResponse};

/// Role identifier for connections
#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    Pool,
    Mint,
}

mod channel_manager;
mod message_codec;
mod message_hub;

pub use channel_manager::{ChannelError, ChannelManager};
pub use message_codec::{MessageCodec, MessageType};
pub use message_hub::{
    MessageHubStats, MintPoolMessageHub, MintQuoteResponseEvent, PendingQuoteContext,
};

/// Configuration for the messaging system
#[derive(Debug, Clone)]
pub struct MessagingConfig {
    /// Buffer size for broadcast channels
    pub broadcast_buffer_size: usize,
    /// Buffer size for MPSC channels  
    pub mpsc_buffer_size: usize,
    /// Maximum number of retries for failed messages
    pub max_retries: u32,
    /// Timeout for message operations in milliseconds
    pub timeout_ms: u64,
}

impl Default for MessagingConfig {
    fn default() -> Self {
        Self {
            broadcast_buffer_size: 1000,
            mpsc_buffer_size: 100,
            max_retries: 3,
            timeout_ms: 5000,
        }
    }
}

/// Errors that can occur in the messaging system
#[derive(Error, Debug)]
pub enum MessagingError {
    #[error("Channel closed: {0}")]
    ChannelClosed(String),
    #[error("Message timeout")]
    Timeout,
    #[error("Encoding error: {0}")]
    Encoding(String),
    #[error("Decoding error: {0}")]
    Decoding(String),
    #[error("Invalid message type: {0}")]
    InvalidMessageType(u8),
    #[error("Connection error: {0}")]
    Connection(String),
}

/// Result type for messaging operations
pub type MessagingResult<T> = Result<T, MessagingError>;

fn map_share_hash_error(err: ShareHashError) -> QuoteBuildError {
    match err {
        ShareHashError::InvalidLength { actual } => {
            QuoteBuildError::InvalidHeaderHashLength(actual)
        }
        ShareHashError::InvalidEncoding => {
            QuoteBuildError::InvalidHeaderHash(binary_sv2::Error::DecodableConversionError)
        }
    }
}

/// Build a fully-parsed mint quote request ready for broadcast through the message hub.
pub fn build_parsed_quote_request(
    amount: u64,
    header_hash: &[u8],
    locking_key: CompressedPubKey<'static>,
) -> Result<ParsedMintQuoteRequest, QuoteBuildError> {
    let share_hash = ShareHash::try_from(header_hash).map_err(map_share_hash_error)?;
    let request = build_mint_quote_request(amount, share_hash.as_bytes(), locking_key)?;
    Ok(ParsedMintQuoteRequest {
        request,
        share_hash,
    })
}
