use anyhow::Result;
use cdk::mint::Mint;
use codec_sv2::{StandardEitherFrame, StandardSv2Frame};
use roles_logic_sv2::parsers_sv2::AnyMessage;
use std::sync::Arc;
use tracing::info;

use super::{
    super::epoch::EpochManager, super::message_types::MintMessageType,
    quote_processing::process_mint_quote_message,
};

/// Type alias for frames used in mint/pool communication
/// Uses AnyMessage to work with Connection channel types
type MintFrame = StandardEitherFrame<AnyMessage<'static>>;

/// Handle SV2 connection frames and process mint quote requests
pub async fn handle_sv2_connection(
    mint: Arc<Mint>,
    epochs: Arc<EpochManager>,
    receiver: async_channel::Receiver<MintFrame>,
    sender: async_channel::Sender<MintFrame>,
) -> Result<()> {
    info!("Starting SV2 message handling loop");

    while let Ok(either_frame) = receiver.recv().await {
        if let Err(e) = process_sv2_frame(&mint, &epochs, either_frame, &sender).await {
            tracing::error!("Error processing SV2 frame: {}", e);
            // Continue processing other frames
        }
    }

    Ok(())
}

/// Process a single SV2 frame
async fn process_sv2_frame(
    mint: &Arc<Mint>,
    epochs: &Arc<EpochManager>,
    either_frame: MintFrame,
    sender: &async_channel::Sender<MintFrame>,
) -> Result<()> {
    tracing::debug!("Received SV2 either frame");

    match either_frame {
        StandardEitherFrame::Sv2(incoming) => {
            process_sv2_message(mint, epochs, incoming, sender).await
        }
        StandardEitherFrame::HandShake(_) => {
            tracing::debug!("Received handshake frame - ignoring");
            Ok(())
        }
    }
}

/// Process an SV2 message frame
async fn process_sv2_message(
    mint: &Arc<Mint>,
    epochs: &Arc<EpochManager>,
    mut incoming: StandardSv2Frame<AnyMessage<'static>>,
    sender: &async_channel::Sender<MintFrame>,
) -> Result<()> {
    tracing::debug!("Received SV2 frame");

    let message_type = incoming
        .get_header()
        .ok_or_else(|| anyhow::anyhow!("No header set"))?
        .msg_type();
    let payload = incoming.payload().to_vec();

    tracing::debug!(
        "Received message type: 0x{:02x}, payload length: {} bytes",
        message_type,
        payload.len()
    );

    let msg_type = MintMessageType::from_code(message_type);
    match msg_type {
        // Setup responses (handled during connection, log if received again)
        MintMessageType::Unknown(0x00) | MintMessageType::Unknown(0x01) => {
            tracing::debug!("Received setup response during connection");
            Ok(())
        }
        // Mint quote messages from pool
        MintMessageType::MintQuoteRequest => {
            tracing::info!("Received MintQuoteRequest from pool, processing quote");
            process_mint_quote_message(mint.clone(), epochs.clone(), message_type, &payload, sender)
                .await
        }
        MintMessageType::MintQuoteResponse | MintMessageType::MintQuoteError => {
            tracing::warn!(
                "Received unexpected response message from pool: {:?}",
                msg_type
            );
            Ok(())
        }
        MintMessageType::Unknown(code) => {
            tracing::warn!("Received unsupported message type: 0x{:02x}", code);
            Ok(())
        }
    }
}
