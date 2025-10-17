//! Message type definitions for the mint-quote protocol.
//!
//! This module provides protocol-level type checking and identification for
//! mint-quote messages, independent of any messaging infrastructure.

use const_sv2::{
    MESSAGE_TYPE_MINT_QUOTE_ERROR, MESSAGE_TYPE_MINT_QUOTE_REQUEST,
    MESSAGE_TYPE_MINT_QUOTE_RESPONSE,
};
use mint_quote_sv2::{MintQuoteError, MintQuoteRequest, MintQuoteResponse};

/// Error type for message type operations
#[derive(Debug, thiserror::Error)]
pub enum MessageTypeError {
    #[error("Invalid message type: {0}")]
    InvalidMessageType(u8),
}

/// Message types for the mint-quote protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    MintQuoteRequest = MESSAGE_TYPE_MINT_QUOTE_REQUEST as isize,
    MintQuoteResponse = MESSAGE_TYPE_MINT_QUOTE_RESPONSE as isize,
    MintQuoteError = MESSAGE_TYPE_MINT_QUOTE_ERROR as isize,
}

impl MessageType {
    /// Convert a u8 message type to a MessageType enum
    pub fn from_u8(value: u8) -> Result<Self, MessageTypeError> {
        match value {
            MESSAGE_TYPE_MINT_QUOTE_REQUEST => Ok(MessageType::MintQuoteRequest),
            MESSAGE_TYPE_MINT_QUOTE_RESPONSE => Ok(MessageType::MintQuoteResponse),
            MESSAGE_TYPE_MINT_QUOTE_ERROR => Ok(MessageType::MintQuoteError),
            _ => Err(MessageTypeError::InvalidMessageType(value)),
        }
    }

    /// Convert the MessageType to a u8
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    /// Check if a message type byte is a mint quote message
    pub fn is_mint_quote_message(message_type: u8) -> bool {
        matches!(
            message_type,
            MESSAGE_TYPE_MINT_QUOTE_REQUEST
                | MESSAGE_TYPE_MINT_QUOTE_RESPONSE
                | MESSAGE_TYPE_MINT_QUOTE_ERROR
        )
    }

    /// Get the message type for a request
    pub fn request() -> Self {
        MessageType::MintQuoteRequest
    }

    /// Get the message type for a response
    pub fn response() -> Self {
        MessageType::MintQuoteResponse
    }

    /// Get the message type for an error
    pub fn error() -> Self {
        MessageType::MintQuoteError
    }
}

/// Enum representing any mint quote message
#[derive(Debug, Clone)]
pub enum MintQuoteMessage {
    Request(MintQuoteRequest<'static>),
    Response(MintQuoteResponse<'static>),
    Error(MintQuoteError<'static>),
}

impl MintQuoteMessage {
    /// Get the message type for this message
    pub fn message_type(&self) -> MessageType {
        match self {
            MintQuoteMessage::Request(_) => MessageType::MintQuoteRequest,
            MintQuoteMessage::Response(_) => MessageType::MintQuoteResponse,
            MintQuoteMessage::Error(_) => MessageType::MintQuoteError,
        }
    }
}
