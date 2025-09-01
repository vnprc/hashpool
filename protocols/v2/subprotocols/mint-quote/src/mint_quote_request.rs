use super::*;

/// Pool requests a mint quote from the mint service
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteRequest<'decoder> {
    /// Amount for the quote
    pub amount: u64,
    /// Currency unit (should be "HASH" for mining shares)
    pub unit: Str0255<'decoder>,
    /// Hash of the block header
    pub header_hash: U256<'decoder>,
    /// Optional description
    pub description: Sv2Option<'decoder, Str0255<'decoder>>,
    /// NUT-20 locking key - compressed public key (33 bytes) for the quote
    pub locking_key: CompressedPubKey<'decoder>,
}