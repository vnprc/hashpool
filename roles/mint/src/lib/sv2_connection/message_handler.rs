use std::sync::Arc;
use cdk::mint::Mint;
use roles_logic_sv2::parsers::PoolMessages;
use codec_sv2::StandardEitherFrame;
use mint_pool_messaging::MessageType;
use tracing::info;
use anyhow::Result;

use super::quote_processing::process_mint_quote_message;

/// Handle SV2 connection frames and process mint quote requests
pub async fn handle_sv2_connection(
    mint: Arc<Mint>,
    receiver: async_channel::Receiver<StandardEitherFrame<PoolMessages<'static>>>,
    sender: async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    info!("Starting SV2 message handling loop");
    
    while let Ok(either_frame) = receiver.recv().await {
        if let Err(e) = process_sv2_frame(&mint, either_frame, &sender).await {
            tracing::error!("Error processing SV2 frame: {}", e);
            // Continue processing other frames
        }
    }
    
    Ok(())
}

/// Process a single SV2 frame
async fn process_sv2_frame(
    mint: &Arc<Mint>,
    either_frame: StandardEitherFrame<PoolMessages<'static>>,
    sender: &async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    tracing::debug!("Received SV2 either frame");
    
    match either_frame {
        StandardEitherFrame::Sv2(incoming) => {
            process_sv2_message(mint, incoming, sender).await
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
    mut incoming: codec_sv2::StandardSv2Frame<PoolMessages<'static>>,
    sender: &async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    tracing::debug!("Received SV2 frame");
    
    let message_type = incoming
        .get_header()
        .ok_or_else(|| anyhow::anyhow!("No header set"))?
        .msg_type();
    let payload = incoming.payload();
    
    tracing::debug!("Received message type: 0x{:02x}, payload length: {} bytes", message_type, payload.len());
    
    if MessageType::is_mint_quote_message(message_type) {
        process_mint_quote_message(mint.clone(), message_type, payload, sender).await
    } else {
        tracing::warn!("Received non-mint-quote message type: 0x{:02x}", message_type);
        Ok(())
    }
}

