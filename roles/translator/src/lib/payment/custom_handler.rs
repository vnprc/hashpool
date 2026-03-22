use async_trait::async_trait;

/// Extension point for handling custom SV2 Mining message types (0xC0–0xFF).
///
/// The pool sends non-standard message types to the translator for CDK payment
/// integration. This trait allows hashpool to handle those messages without
/// modifying the upstream stratum-apps channel manager logic.
///
/// Once sv2-apps merges this interface upstream, import it from stratum-apps
/// instead and delete this local definition.
#[async_trait]
pub trait CustomMiningMessageHandler: Send + Sync + std::fmt::Debug {
    /// Called for Mining message types outside the standard SV2 range.
    ///
    /// `msg_type` is the raw message type byte (e.g., 0xC0).
    /// `payload` is the raw frame payload bytes (after the SV2 header).
    async fn handle_custom_message(
        &self,
        msg_type: u8,
        payload: &[u8],
    ) -> Result<(), anyhow::Error>;
}

/// No-op implementation for deployments without CDK payment integration.
#[derive(Debug)]
pub struct NoopCustomMiningMessageHandler;

#[async_trait]
impl CustomMiningMessageHandler for NoopCustomMiningMessageHandler {
    async fn handle_custom_message(
        &self,
        _msg_type: u8,
        _payload: &[u8],
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }
}
