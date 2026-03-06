use std::fmt;
use super::*;

/// Message type byte for MintQuoteNotification (extension range 0xC0+)
pub const MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION: u8 = 0xC0;
/// Message type byte for MintQuoteFailure (extension range 0xC0+)
pub const MESSAGE_TYPE_MINT_QUOTE_FAILURE: u8 = 0xC1;

/// Channel bit for MintQuoteNotification — channel-specific message
pub const CHANNEL_BIT_MINT_QUOTE_NOTIFICATION: bool = true;
/// Channel bit for MintQuoteFailure — channel-specific message
pub const CHANNEL_BIT_MINT_QUOTE_FAILURE: bool = true;

/// Notification sent to downstream when a quote becomes payable.
/// Extension message (0xC0) for the Mining protocol.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MintQuoteNotification<'decoder> {
    /// Channel identifier - required as first field per SV2 spec section 3.2.1
    pub channel_id: u32,
    pub quote_id: Str0255<'decoder>,
    pub amount: u64,
}

impl fmt::Display for MintQuoteNotification<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MintQuoteNotification(quote_id: {}, amount: {})",
            self.quote_id.as_utf8_or_hex(),
            self.amount
        )
    }
}

/// Failure notification if a quote cannot be processed.
/// Extension message (0xC1) for the Mining protocol.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MintQuoteFailure<'decoder> {
    /// Channel identifier - required as first field per SV2 spec section 3.2.1
    pub channel_id: u32,
    pub quote_id: Str0255<'decoder>,
    pub error_code: u32,
    pub error_message: Str0255<'decoder>,
}

impl fmt::Display for MintQuoteFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MintQuoteFailure(quote_id: {}, error_code: {}, error_message: {})",
            self.quote_id.as_utf8_or_hex(),
            self.error_code,
            self.error_message.as_utf8_or_hex()
        )
    }
}
