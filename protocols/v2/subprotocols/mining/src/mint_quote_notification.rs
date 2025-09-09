#[cfg(not(feature = "with_serde"))]
use alloc::vec::Vec;
#[cfg(not(feature = "with_serde"))]
use binary_sv2::binary_codec_sv2;
use binary_sv2::{Deserialize, Serialize, Str0255, U256};
#[cfg(not(feature = "with_serde"))]
use core::convert::TryInto;

/// Custom extension message sent from Pool to Translator after mint quote is ready
/// This is sent AFTER the standard SubmitSharesSuccess to provide quote information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintQuoteNotification<'decoder> {
    /// Channel ID this quote is for
    pub channel_id: u32,
    /// Sequence number of the original share submission
    pub sequence_number: u32,
    /// Share hash (matches the hash in SubmitSharesExtended)
    pub share_hash: U256<'decoder>,
    /// Quote ID from the mint for ecash token creation
    #[cfg_attr(feature = "with_serde", serde(borrow))]
    pub quote_id: Str0255<'decoder>,
    /// Amount of work/difficulty for this share
    pub amount: u64,
}

/// Error notification when mint quote fails
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintQuoteFailure<'decoder> {
    /// Channel ID this failure is for
    pub channel_id: u32,
    /// Sequence number of the original share submission
    pub sequence_number: u32,
    /// Share hash that failed to get a quote
    pub share_hash: U256<'decoder>,
    /// Error message from mint
    #[cfg_attr(feature = "with_serde", serde(borrow))]
    pub error_message: Str0255<'decoder>,
}

#[cfg(feature = "with_serde")]
use binary_sv2::GetSize;
#[cfg(feature = "with_serde")]
impl<'d> GetSize for MintQuoteNotification<'d> {
    fn get_size(&self) -> usize {
        self.channel_id.get_size()
            + self.sequence_number.get_size()
            + self.share_hash.get_size()
            + self.quote_id.get_size()
            + self.amount.get_size()
    }
}

#[cfg(feature = "with_serde")]
impl<'d> GetSize for MintQuoteFailure<'d> {
    fn get_size(&self) -> usize {
        self.channel_id.get_size()
            + self.sequence_number.get_size()
            + self.share_hash.get_size()
            + self.error_message.get_size()
    }
}

#[cfg(feature = "with_serde")]
impl<'a> MintQuoteNotification<'a> {
    pub fn into_static(self) -> MintQuoteNotification<'static> {
        panic!("This function shouldn't be called by the Message Generator");
    }
    pub fn as_static(&self) -> MintQuoteNotification<'static> {
        panic!("This function shouldn't be called by the Message Generator");
    }
}

#[cfg(feature = "with_serde")]
impl<'a> MintQuoteFailure<'a> {
    pub fn into_static(self) -> MintQuoteFailure<'static> {
        panic!("This function shouldn't be called by the Message Generator");
    }
    pub fn as_static(&self) -> MintQuoteFailure<'static> {
        panic!("This function shouldn't be called by the Message Generator");
    }
}