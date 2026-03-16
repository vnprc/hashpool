use anyhow::Result;
use binary_sv2::Str0255;
use cdk::mint::{Mint, MintQuoteRequest, MintQuoteResponse as CdkMintQuoteResponse};
use cdk::{
    cdk_payment::{PaymentIdentifier, WaitPaymentResponse},
    nuts::CurrencyUnit,
    Amount,
};
use codec_sv2::StandardEitherFrame;
use mint_quote_sv2::MESSAGE_TYPE_MINT_QUOTE_REQUEST;
use hex;
use mint_pool_messaging::{
    mint_quote_response_from_cdk, parse_mint_quote_request, quote_error_frame_bytes,
    quote_response_frame_bytes, MintQuoteError,
};
use mint_quote_sv2::MintQuoteResponse;
use roles_logic_sv2::parsers_sv2::AnyMessage;
use std::sync::Arc;
use tracing::info;

use codec_sv2::StandardSv2Frame;
use ehash::calculate_difficulty;

/// Type alias for frames used in mint/pool communication
/// Uses AnyMessage to work with Connection channel types
type MintFrame = StandardEitherFrame<AnyMessage<'static>>;

/// Process mint quote messages
pub async fn process_mint_quote_message(
    mint: Arc<Mint>,
    message_type: u8,
    payload: &[u8],
    sender: &async_channel::Sender<MintFrame>,
) -> Result<()> {
    info!("Received mint quote message - processing with mint");

    match message_type {
        MESSAGE_TYPE_MINT_QUOTE_REQUEST => {
            let parsed_request = parse_mint_quote_request(payload)
                .map_err(|e| anyhow::anyhow!("Failed to parse MintQuoteRequest: {e}"))?;
            let share_hash = parsed_request.share_hash;
            // Convert to big-endian for difficulty calculation (hash is little-endian bytes).
            let mut share_hash_bytes = *share_hash.as_bytes();
            share_hash_bytes.reverse();
            let leading_zero_bits = calculate_difficulty(share_hash_bytes);
            let amount = parsed_request.request.amount;
            let locking_key_hex = hex::encode(parsed_request.request.locking_key.inner_as_ref());
            tracing::debug!(
                "Parsed mint quote request: share_hash={} locking_key={}",
                hex::encode(share_hash.as_bytes()),
                locking_key_hex
            );

            let cdk_custom_request = parsed_request
                .to_cdk_request()
                .map_err(|e| anyhow::anyhow!("Failed to convert MintQuoteRequest: {e}"))?;

            let mint_quote_request = MintQuoteRequest::Custom {
                method: "ehash".to_string(),
                request: cdk_custom_request,
            };

            match mint.get_mint_quote(mint_quote_request).await {
                Ok(cdk_response) => {
                    let custom_response = match cdk_response {
                        CdkMintQuoteResponse::Custom { response, .. } => response,
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Expected Custom mint quote response, got different variant"
                            ));
                        }
                    };

                    let quote_id_str = custom_response.quote.to_string();
                    info!(
                        "Successfully created mint quote: quote_id={} share_hash={} amount={}",
                        quote_id_str, share_hash, amount,
                    );

                    // Mark quote as paid immediately — pool validated the share before sending this message.
                    let header_hash_hex = hex::encode(share_hash.as_bytes());
                    let amount_with_unit = Amount::new(amount, CurrencyUnit::Custom("hash".to_string()));

                    mint.pay_mint_quote_for_request_id(WaitPaymentResponse {
                        payment_identifier: PaymentIdentifier::CustomId(header_hash_hex.clone()),
                        payment_amount: amount_with_unit,
                        payment_id: header_hash_hex,
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to pay ehash quote: {e}"))?;

                    let sv2_response = mint_quote_response_from_cdk(share_hash, custom_response)
                        .map_err(|e| {
                            anyhow::anyhow!("Failed to convert mint quote response: {e}")
                        })?;

                    send_quote_response_to_pool(sv2_response, sender).await?;

                    Ok(())
                }
                Err(e) => {
                    let amount_exponent = if amount > 0 {
                        Some(amount.trailing_zeros())
                    } else {
                        None
                    };

                    let estimated_min_difficulty =
                        amount_exponent.map(|exp| leading_zero_bits.saturating_sub(exp));

                    if amount == 0 {
                        tracing::error!(
                            share_hash = %share_hash,
                            leading_zero_bits,
                            locking_key = %locking_key_hex,
                            "Failed to create mint quote: {} (computed amount is 0 HASH; share likely below pool minimum difficulty)",
                            e
                        );
                    } else {
                        tracing::error!(
                            share_hash = %share_hash,
                            amount,
                            leading_zero_bits,
                            estimated_min_difficulty = estimated_min_difficulty,
                            locking_key = %locking_key_hex,
                            "Failed to create mint quote: {}",
                            e
                        );
                    }

                    let error_message = if amount == 0 {
                        format!(
                            "Share below pool minimum difficulty: leading_zero_bits={} produced 0 HASH",
                            leading_zero_bits
                        )
                    } else {
                        e.to_string()
                    };

                    // Send error response back to pool
                    send_quote_error_to_pool(error_message.clone(), sender).await?;

                    Err(anyhow::anyhow!(
                        "Mint quote creation failed: {}",
                        error_message
                    ))
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
    sender: &async_channel::Sender<MintFrame>,
) -> Result<()> {
    let quote_id_str =
        std::str::from_utf8(response.quote_id.inner_as_ref()).unwrap_or("invalid_utf8");

    info!("🚀 Sending quote response: quote_id={}", quote_id_str);

    // Encode MintQuoteResponse using binary_sv2 codec serialization
    // The response is serialized to bytes using the Encodable trait,
    // with message type byte (0x81) prepended for frame identification
    let frame_bytes = quote_response_frame_bytes(&response)
        .map_err(|e| anyhow::anyhow!("Failed to encode mint quote response frame: {e}"))?;

    // Create Sv2Frame from encoded bytes and wrap in StandardEitherFrame for transmission
    let sv2_frame = StandardSv2Frame::from_bytes_unchecked(frame_bytes.into());
    let frame = StandardEitherFrame::Sv2(sv2_frame);

    sender
        .send(frame)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send quote response frame: {}", e))?;

    info!("✅ Quote response sent (quote_id={})", quote_id_str);
    Ok(())
}

/// Send MintQuoteError back to pool
async fn send_quote_error_to_pool(
    error_message: String,
    sender: &async_channel::Sender<MintFrame>,
) -> Result<()> {
    // Create error code (generic error = 1)
    let error_code = 1u32;

    info!(
        "⚠️  Sending error response: code={}, message={}",
        error_code, error_message
    );

    // Use frame codec to encode error response
    let error = MintQuoteError {
        error_code,
        error_message: Str0255::try_from(error_message.clone())
            .map_err(|e| anyhow::anyhow!("Failed to encode error message: {e:?}"))?
            .into_static(),
    };

    let frame_bytes = quote_error_frame_bytes(&error)
        .map_err(|e| anyhow::anyhow!("Failed to encode mint quote error frame: {e}"))?;

    // Create Sv2Frame from encoded bytes and wrap in StandardEitherFrame for transmission
    let sv2_frame = StandardSv2Frame::from_bytes_unchecked(frame_bytes.into());
    let frame = StandardEitherFrame::Sv2(sv2_frame);

    sender
        .send(frame)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send error frame: {}", e))?;

    info!(
        "✅ Quote error sent (code={}, message={})",
        error_code, error_message
    );
    Ok(())
}
