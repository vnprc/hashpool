use super::*;

/// Mint service responds with quote details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteResponse<'decoder> {
    /// Unique quote identifier - only field the pool actually needs
    pub quote_id: Str0255<'decoder>,
    /// Header hash that was used to generate this quote
    pub header_hash: U256<'decoder>,
}