use super::*;

/// Mint service responds with quote details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteResponse<'decoder> {
    /// Unique quote identifier
    pub quote_id: Str0255<'decoder>,
    /// Amount for the quote
    pub amount: u64,
    /// Currency unit
    pub unit: Str0255<'decoder>,
    /// Quote expiration timestamp (Unix timestamp)
    pub expires_at: u64,
    /// Quote state/status
    pub state: u8,
}