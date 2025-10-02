use anyhow::Result;
use binary_sv2::{self, CompressedPubKey, Str0255, Sv2Option};
use cdk::mint::Mint;
use codec_sv2::{StandardEitherFrame, StandardSv2Frame};
use const_sv2::MESSAGE_TYPE_MINT_QUOTE_REQUEST;
use ehash::ShareHash;
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
            // Parse the payload into a MintQuoteRequest
            let mut payload_copy = payload.to_vec();
            let parsed_request: mint_pool_messaging::MintQuoteRequest =
                binary_sv2::from_bytes(&mut payload_copy)
                    .map_err(|e| anyhow::anyhow!("Failed to parse MintQuoteRequest: {:?}", e))?;

            let share_hash = ShareHash::from_u256(&parsed_request.header_hash)
                .map_err(|e| anyhow::anyhow!("Invalid header hash: {e}"))?;

            // Create a static lifetime version for the conversion function
            let request_static = create_static_mint_quote_request(parsed_request, share_hash)?;

            // Convert SV2 MintQuoteRequest to CDK MintQuoteMiningShareRequest
            let cdk_request = convert_sv2_to_cdk_quote_request(request_static, share_hash)?;

            // Process with CDK mint
            match mint.create_mint_mining_share_quote(cdk_request).await {
                Ok(quote_response) => {
                    info!(
                        "Successfully created mint quote: quote_id={} share_hash={}",
                        quote_response.id, share_hash,
                    );

                    // Convert CDK response to SV2 MintQuoteResponse
                    let sv2_response =
                        convert_cdk_to_sv2_quote_response(quote_response, share_hash)?;

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

/// Create a static lifetime version of MintQuoteRequest from a borrowed one
fn create_static_mint_quote_request(
    parsed_request: mint_pool_messaging::MintQuoteRequest,
    share_hash: ShareHash,
) -> Result<mint_pool_messaging::MintQuoteRequest<'static>> {
    // Convert the borrowed data to owned data with static lifetime
    let unit_str = String::from_utf8_lossy(parsed_request.unit.inner_as_ref()).to_string();
    let unit_static =
        Str0255::try_from(unit_str).map_err(|e| anyhow::anyhow!("Invalid unit string: {:?}", e))?;

    let description_static = if let Some(desc) = parsed_request.description.into_inner() {
        let desc_str = String::from_utf8_lossy(desc.inner_as_ref()).to_string();
        let desc_static = Str0255::try_from(desc_str)
            .map_err(|e| anyhow::anyhow!("Invalid description string: {:?}", e))?;
        Sv2Option::new(Some(desc_static))
    } else {
        Sv2Option::new(None)
    };

    // Create owned versions of the other fields
    let header_hash_static = share_hash
        .into_u256()
        .map_err(|e| anyhow::anyhow!("Invalid header hash: {e}"))?;

    let locking_key_bytes = parsed_request.locking_key.inner_as_ref().to_vec();
    let locking_key_static = CompressedPubKey::try_from(locking_key_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid locking key: {:?}", e))?;

    Ok(mint_pool_messaging::MintQuoteRequest {
        amount: parsed_request.amount,
        unit: unit_static,
        header_hash: header_hash_static,
        description: description_static,
        locking_key: locking_key_static,
    })
}

/// Convert SV2 MintQuoteRequest to CDK MintQuoteMiningShareRequest  
fn convert_sv2_to_cdk_quote_request(
    sv2_request: mint_pool_messaging::MintQuoteRequest<'static>,
    share_hash: ShareHash,
) -> Result<cdk::nuts::nutXX::MintQuoteMiningShareRequest> {
    use cdk::secp256k1::hashes::Hash as CdkHashTrait;

    // Convert amount (already u64)
    let amount = cdk::Amount::from(sv2_request.amount);

    // Convert unit (should be "HASH")
    let unit = cdk::nuts::CurrencyUnit::Hash;

    // Convert header hash from shared representation to CDK Hash
    debug_assert_eq!(
        sv2_request.header_hash.inner_as_ref(),
        share_hash.as_bytes()
    );
    let header_hash = CdkHashTrait::from_slice(share_hash.as_bytes())
        .map_err(|e| anyhow::anyhow!("Invalid header hash: {}", e))?;

    // Convert description (optional)
    let description = sv2_request
        .description
        .into_inner()
        .map(|s| String::from_utf8_lossy(s.inner_as_ref()).to_string());

    // Convert locking key (compressed public key)
    let pubkey_bytes = sv2_request.locking_key.inner_as_ref();
    let pubkey = cdk::nuts::PublicKey::from_slice(pubkey_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid locking pubkey: {}", e))?;

    Ok(cdk::nuts::nutXX::MintQuoteMiningShareRequest {
        amount,
        unit,
        header_hash,
        description,
        pubkey,
    })
}

/// Convert CDK MintQuote to SV2 MintQuoteResponse
fn convert_cdk_to_sv2_quote_response(
    cdk_quote: cdk::mint::MintQuote,
    share_hash: ShareHash,
) -> Result<MintQuoteResponse<'static>> {
    // Convert quote ID (UUID) to string and then to Str0255
    let quote_id_str = cdk_quote.id.to_string();
    let quote_id = Str0255::try_from(quote_id_str)
        .map_err(|e| anyhow::anyhow!("Invalid quote ID string: {:?}", e))?;
    let header_hash = share_hash
        .into_u256()
        .map_err(|e| anyhow::anyhow!("Invalid header hash: {e}"))?;

    Ok(MintQuoteResponse {
        quote_id,
        header_hash,
    })
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
