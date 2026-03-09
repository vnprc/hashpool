//! Message type conversions between mint protocol and SV2 formats
//!
//! Phase 2: Message Type Mapping Deliverable
//! Maps mint-specific messages to current SRI 1.5.0 message enums


/// Mint protocol message type codes
pub mod message_types {
    /// MintQuoteRequest message type
    pub const MINT_QUOTE_REQUEST: u8 = 0x80;

    /// MintQuoteResponse message type
    pub const MINT_QUOTE_RESPONSE: u8 = 0x81;

    /// MintQuoteError message type
    pub const MINT_QUOTE_ERROR: u8 = 0x82;
}

/// Identifies the type of a received mint protocol message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintMessageType {
    /// MintQuoteRequest - request for mint quote from pool
    MintQuoteRequest,

    /// MintQuoteResponse - response with mint quote
    MintQuoteResponse,

    /// MintQuoteError - error response
    MintQuoteError,

    /// Unknown message type
    Unknown(u8),
}

#[allow(dead_code)]
impl MintMessageType {
    /// Parse message type from byte code
    pub fn from_code(code: u8) -> Self {
        match code {
            message_types::MINT_QUOTE_REQUEST => MintMessageType::MintQuoteRequest,
            message_types::MINT_QUOTE_RESPONSE => MintMessageType::MintQuoteResponse,
            message_types::MINT_QUOTE_ERROR => MintMessageType::MintQuoteError,
            other => MintMessageType::Unknown(other),
        }
    }

    /// Convert to message type code
    pub fn to_code(&self) -> Option<u8> {
        match self {
            MintMessageType::MintQuoteRequest => Some(message_types::MINT_QUOTE_REQUEST),
            MintMessageType::MintQuoteResponse => Some(message_types::MINT_QUOTE_RESPONSE),
            MintMessageType::MintQuoteError => Some(message_types::MINT_QUOTE_ERROR),
            MintMessageType::Unknown(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_from_code() {
        assert_eq!(
            MintMessageType::from_code(0x80),
            MintMessageType::MintQuoteRequest
        );
        assert_eq!(
            MintMessageType::from_code(0x81),
            MintMessageType::MintQuoteResponse
        );
        assert_eq!(
            MintMessageType::from_code(0x82),
            MintMessageType::MintQuoteError
        );
        assert_eq!(
            MintMessageType::from_code(0xFF),
            MintMessageType::Unknown(0xFF)
        );
    }

    #[test]
    fn test_message_type_to_code() {
        assert_eq!(MintMessageType::MintQuoteRequest.to_code(), Some(0x80));
        assert_eq!(MintMessageType::MintQuoteResponse.to_code(), Some(0x81));
        assert_eq!(MintMessageType::MintQuoteError.to_code(), Some(0x82));
        assert_eq!(MintMessageType::Unknown(0xFF).to_code(), None);
    }
}
