use super::*;
use const_sv2::{MESSAGE_TYPE_MINT_QUOTE_REQUEST, MESSAGE_TYPE_MINT_QUOTE_RESPONSE, MESSAGE_TYPE_MINT_QUOTE_ERROR};

/// Message types for the mint-quote protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    MintQuoteRequest = MESSAGE_TYPE_MINT_QUOTE_REQUEST as isize,
    MintQuoteResponse = MESSAGE_TYPE_MINT_QUOTE_RESPONSE as isize,
    MintQuoteError = MESSAGE_TYPE_MINT_QUOTE_ERROR as isize,
}

impl MessageType {
    pub fn from_u8(value: u8) -> MessagingResult<Self> {
        match value {
            MESSAGE_TYPE_MINT_QUOTE_REQUEST => Ok(MessageType::MintQuoteRequest),
            MESSAGE_TYPE_MINT_QUOTE_RESPONSE => Ok(MessageType::MintQuoteResponse),
            MESSAGE_TYPE_MINT_QUOTE_ERROR => Ok(MessageType::MintQuoteError),
            _ => Err(MessagingError::InvalidMessageType(value)),
        }
    }
    
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }
}

/// Simple message codec for mint-quote messages
/// Note: Full SV2 framing will be added in later phases
pub struct MessageCodec;

impl MessageCodec {
    /// Get the message type for a request
    pub fn get_request_type() -> MessageType {
        MessageType::MintQuoteRequest
    }
    
    /// Get the message type for a response
    pub fn get_response_type() -> MessageType {
        MessageType::MintQuoteResponse
    }
    
    /// Get the message type for an error
    pub fn get_error_type() -> MessageType {
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
    pub fn message_type(&self) -> MessageType {
        match self {
            MintQuoteMessage::Request(_) => MessageType::MintQuoteRequest,
            MintQuoteMessage::Response(_) => MessageType::MintQuoteResponse,
            MintQuoteMessage::Error(_) => MessageType::MintQuoteError,
        }
    }
}