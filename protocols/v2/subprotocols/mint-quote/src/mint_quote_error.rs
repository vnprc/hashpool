use super::*;

/// Error response from mint service
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteError<'decoder> {
    /// Error code
    pub error_code: u32,
    /// Error message
    pub error_message: Str0255<'decoder>,
}