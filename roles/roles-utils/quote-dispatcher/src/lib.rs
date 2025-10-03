//! Quote dispatcher for mining pool quote submissions.
//!
//! This crate handles all mint quote logic, keeping it separate from the core
//! pool message handling to minimize changes to upstream SRI code.

use std::sync::Arc;

use bitcoin_hashes::{sha256::Hash as Sha256Hash, Hash};
use binary_sv2::CompressedPubKey;
use ehash::calculate_ehash_amount;
use mint_pool_messaging::{build_parsed_quote_request, MintPoolMessageHub, PendingQuoteContext};
use shared_config::Sv2MessagingConfig;
use tracing::{debug, error, info};

/// Callback trait for quote events.
///
/// Implementations can track stats or perform other actions when quotes are created.
pub trait QuoteEventCallback: Send + Sync {
    /// Called when a quote is successfully created.
    fn on_quote_created(&self, channel_id: u32, amount: u64);
}

/// Dispatcher for submitting mint quotes.
///
/// This handles all the logic for creating and dispatching quote requests
/// to the mint service, keeping this functionality isolated from pool logic.
pub struct QuoteDispatcher {
    hub: Arc<MintPoolMessageHub>,
    sv2_config: Option<Sv2MessagingConfig>,
    minimum_difficulty: u32,
    callback: Option<Arc<dyn QuoteEventCallback>>,
}

impl QuoteDispatcher {
    /// Create a new quote dispatcher.
    pub fn new(
        hub: Arc<MintPoolMessageHub>,
        sv2_config: Option<Sv2MessagingConfig>,
        minimum_difficulty: u32,
    ) -> Self {
        Self {
            hub,
            sv2_config,
            minimum_difficulty,
            callback: None,
        }
    }

    /// Set the callback for quote events.
    pub fn with_callback(mut self, callback: Arc<dyn QuoteEventCallback>) -> Self {
        self.callback = Some(callback);
        self
    }

    /// Submit a quote for a share.
    ///
    /// This is the main entry point called by the pool when a share is accepted.
    ///
    /// # Arguments
    ///
    /// * `header_hash` - The share header hash (32 bytes)
    /// * `locking_pubkey` - The miner's locking public key
    /// * `channel_id` - The channel ID
    /// * `sequence_number` - The sequence number from the share submission
    pub fn submit_quote(
        &self,
        header_hash: &[u8],
        locking_pubkey: CompressedPubKey<'static>,
        channel_id: u32,
        sequence_number: u32,
    ) -> Result<(), roles_logic_sv2::Error> {
        let hash = Sha256Hash::from_slice(header_hash)
            .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Invalid header hash: {e}")))?;

        let amount = calculate_ehash_amount(hash.to_byte_array(), self.minimum_difficulty);

        // Notify callback if set
        if let Some(ref callback) = self.callback {
            callback.on_quote_created(channel_id, amount);
        }

        // Check if messaging is enabled
        let messaging_enabled = self.sv2_config.as_ref().map(|cfg| cfg.enabled).unwrap_or(true);
        if !messaging_enabled {
            debug!(
                "SV2 messaging disabled; skipping mint quote dispatch for channel {}",
                channel_id
            );
            return Ok(());
        }

        // Build the parsed quote request
        let parsed = build_parsed_quote_request(amount, header_hash, locking_pubkey)
            .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Failed to build quote: {e}")))?;

        let context = PendingQuoteContext {
            channel_id,
            sequence_number,
            amount,
        };

        let share_hash_hex = hex::encode(parsed.share_hash.as_bytes());
        let hub = self.hub.clone();

        // Spawn async task to dispatch via hub
        tokio::spawn(async move {
            if let Err(e) = hub.send_quote_request(parsed, context).await {
                error!("Failed to dispatch mint quote request via hub: {}", e);
            } else {
                info!(
                    "Queued mint quote request via hub: share_hash={}",
                    share_hash_hex
                );
            }
        });

        Ok(())
    }
}
