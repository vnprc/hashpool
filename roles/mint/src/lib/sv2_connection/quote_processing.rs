use anyhow::Result;
use binary_sv2::Str0255;
use cdk::mint::Mint;
use codec_sv2::{StandardEitherFrame, StandardSv2Frame};
use const_sv2::MESSAGE_TYPE_MINT_QUOTE_REQUEST;
use mint_pool_messaging::{mint_quote_response_from_cdk, parse_mint_quote_request};
use mint_quote_sv2::MintQuoteResponse;
use roles_logic_sv2::parsers::PoolMessages;
use std::sync::Arc;
use tracing::info;

/// Process mint quote messages
pub async fn process_mint_quote_message(
    mint: Arc<Mint>,
    message_type: u8,
    payload: &[u8],
    sender: &async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    info!("Received mint quote message - processing with mint");

    match message_type {
        MESSAGE_TYPE_MINT_QUOTE_REQUEST => {
            let parsed_request = parse_mint_quote_request(payload)
                .map_err(|e| anyhow::anyhow!("Failed to parse MintQuoteRequest: {e}"))?;
            let share_hash = parsed_request.share_hash;

            let cdk_request = parsed_request
                .to_cdk_request()
                .map_err(|e| anyhow::anyhow!("Failed to convert MintQuoteRequest: {e}"))?;

            match mint.create_mint_mining_share_quote(cdk_request).await {
                Ok(quote_response) => {
                    info!(
                        "Successfully created mint quote: quote_id={} share_hash={} amount={}",
                        quote_response.id, share_hash, parsed_request.request.amount,
                    );

                    let sv2_response = mint_quote_response_from_cdk(share_hash, quote_response)
                        .map_err(|e| {
                            anyhow::anyhow!("Failed to convert mint quote response: {e}")
                        })?;

                    // Send response back to pool
                    send_quote_response_to_pool(sv2_response, sender).await?;

                    Ok(())
                }
                Err(e) => {
                    tracing::error!("Failed to create mint quote: {}", e);

                    // Send error response back to pool
                    send_quote_error_to_pool(e.to_string(), sender).await?;

                    Err(anyhow::anyhow!("Mint quote creation failed: {}", e))
                }
            }
        }
        _ => {
            tracing::warn!(
                "Received unsupported mint quote message type: 0x{:02x}",
                message_type
            );
            Ok(())
        }
    }
}
/// Send MintQuoteResponse back to pool via TCP connection
async fn send_quote_response_to_pool(
    response: MintQuoteResponse<'static>,
    sender: &async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    let quote_id_str =
        std::str::from_utf8(response.quote_id.inner_as_ref()).unwrap_or("invalid_utf8");

    info!(
        "🚀 Sending mint quote response via TCP connection: quote_id={}",
        quote_id_str
    );

    // Create pool message for the response
    let pool_message = PoolMessages::Minting(roles_logic_sv2::parsers::Minting::MintQuoteResponse(
        response,
    ));

    // Convert to SV2 frame and send via TCP
    let sv2_frame: StandardSv2Frame<PoolMessages> = pool_message
        .try_into()
        .map_err(|e| anyhow::anyhow!("Failed to create SV2 frame: {:?}", e))?;

    let either_frame = sv2_frame.into();
    sender
        .send(either_frame)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send quote response: {}", e))?;

    info!("✅ Successfully sent mint quote response to pool via TCP");
    Ok(())
}

/// Send MintQuoteError back to pool  
async fn send_quote_error_to_pool(
    error_message: String,
    sender: &async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    use mint_quote_sv2::MintQuoteError;

    // Create error code (generic error = 1)
    let error_code = 1u32;

    // Create error message
    let error_msg = Str0255::try_from(error_message)
        .map_err(|e| anyhow::anyhow!("Error message too long: {:?}", e))?;

    let error_response = MintQuoteError {
        error_code,
        error_message: error_msg,
    };

    // Create pool message
    let pool_message = PoolMessages::Minting(roles_logic_sv2::parsers::Minting::MintQuoteError(
        error_response,
    ));

    // Convert to SV2 frame and send
    let sv2_frame: StandardSv2Frame<PoolMessages> = pool_message
        .try_into()
        .map_err(|e| anyhow::anyhow!("Failed to create SV2 frame: {:?}", e))?;

    let either_frame = sv2_frame.into();
    sender
        .send(either_frame)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send quote error: {}", e))?;

    info!("Successfully sent mint quote error to pool");
    Ok(())
}
