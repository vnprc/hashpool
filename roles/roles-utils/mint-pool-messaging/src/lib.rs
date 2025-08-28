//! # Mint-Pool Messaging Infrastructure
//! 
//! This crate provides the core messaging infrastructure for communication
//! between mining pools and mint services using SV2 messages over MPSC channels.

use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

pub use mint_quote_sv2::{MintQuoteRequest, MintQuoteResponse, MintQuoteError};

/// Role identifier for connections
#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    Pool,
    Mint,
}

mod message_hub;
mod message_codec;
mod channel_manager;

pub use message_hub::MintPoolMessageHub;
pub use message_codec::{MessageCodec, MessageType};
pub use channel_manager::{ChannelManager, ChannelError};

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