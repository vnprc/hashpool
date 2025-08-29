use std::sync::Arc;
use cdk::mint::Mint;
use roles_logic_sv2::parsers::{PoolMessages, MintQuote};
use mint_quote_sv2::MintQuoteResponse;
use codec_sv2::{StandardEitherFrame, StandardSv2Frame};
use const_sv2::MESSAGE_TYPE_MINT_QUOTE_REQUEST;
use binary_sv2::{self, Str0255, Sv2Option, U256, CompressedPubKey};
use anyhow::Result;
use tracing::info;

/// Process mint quote messages
pub async fn process_mint_quote_message(
    mint: Arc<Mint>,
    message_type: u8,
    payload: &[u8],
    _sender: &async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    info!("Received mint quote message - processing with mint");
    
    match message_type {
        MESSAGE_TYPE_MINT_QUOTE_REQUEST => {
            // Parse the payload into a MintQuoteRequest 
            let mut payload_copy = payload.to_vec();
            let parsed_request: mint_pool_messaging::MintQuoteRequest = binary_sv2::from_bytes(&mut payload_copy)
                .map_err(|e| anyhow::anyhow!("Failed to parse MintQuoteRequest: {:?}", e))?;
            
            // Create a static lifetime version for the conversion function
            let request_static = create_static_mint_quote_request(parsed_request)?;
            
            // Convert SV2 MintQuoteRequest to CDK MintQuoteMiningShareRequest
            let cdk_request = convert_sv2_to_cdk_quote_request(request_static)?;
            
            // Process with CDK mint
            match mint.create_mint_mining_share_quote(cdk_request).await {
                Ok(quote_response) => {
                    info!("Successfully created mint quote: {:?}", quote_response);
                    // TODO: Send response back to pool
                    Ok(())
                }
                Err(e) => {
                    tracing::error!("Failed to create mint quote: {}", e);
                    // TODO: Send error response back to pool
                    Err(anyhow::anyhow!("Mint quote creation failed: {}", e))
                }
            }
        },
        _ => {
            tracing::warn!("Received unsupported mint quote message type: 0x{:02x}", message_type);
            Ok(())
        }
    }
}

/// Send quote response back to pool
#[allow(dead_code)]
async fn send_quote_response(
    response: MintQuoteResponse<'static>,
    sender: &async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    let pool_response = PoolMessages::MintQuote(
        MintQuote::MintQuoteResponse(response)
    );
    
    let sv2_frame: StandardSv2Frame<PoolMessages> = pool_response.try_into()
        .map_err(|e| anyhow::anyhow!("Failed to create SV2 frame: {:?}", e))?;
    let either_frame = sv2_frame.into();
    
    sender.send(either_frame).await
        .map_err(|e| anyhow::anyhow!("Failed to send response: {}", e))?;
        
    Ok(())
}

/// Create a static lifetime version of MintQuoteRequest from a borrowed one
fn create_static_mint_quote_request(
    parsed_request: mint_pool_messaging::MintQuoteRequest
) -> Result<mint_pool_messaging::MintQuoteRequest<'static>> {
    // Convert the borrowed data to owned data with static lifetime
    let unit_str = String::from_utf8_lossy(parsed_request.unit.inner_as_ref()).to_string();
    let unit_static = Str0255::try_from(unit_str)
        .map_err(|e| anyhow::anyhow!("Invalid unit string: {:?}", e))?;
    
    let description_static = if let Some(desc) = parsed_request.description.into_inner() {
        let desc_str = String::from_utf8_lossy(desc.inner_as_ref()).to_string();
        let desc_static = Str0255::try_from(desc_str)
            .map_err(|e| anyhow::anyhow!("Invalid description string: {:?}", e))?;
        Sv2Option::new(Some(desc_static))
    } else {
        Sv2Option::new(None)
    };
    
    // Create owned versions of the other fields
    let header_hash_bytes = parsed_request.header_hash.inner_as_ref().to_vec();
    let header_hash_static = U256::try_from(header_hash_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid header hash: {:?}", e))?;
    
    let locking_key_bytes = parsed_request.locking_key.inner_as_ref().to_vec();  
    let locking_key_static = CompressedPubKey::try_from(locking_key_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid locking key: {:?}", e))?;
    
    let keyset_id_bytes = parsed_request.keyset_id.inner_as_ref().to_vec();
    let keyset_id_static = U256::try_from(keyset_id_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid keyset ID: {:?}", e))?;
    
    Ok(mint_pool_messaging::MintQuoteRequest {
        amount: parsed_request.amount,
        unit: unit_static,
        header_hash: header_hash_static,
        description: description_static,
        locking_key: locking_key_static,
        keyset_id: keyset_id_static,
    })
}

/// Convert SV2 MintQuoteRequest to CDK MintQuoteMiningShareRequest  
fn convert_sv2_to_cdk_quote_request(
    sv2_request: mint_pool_messaging::MintQuoteRequest<'static>,
) -> Result<cdk::nuts::nutXX::MintQuoteMiningShareRequest> {
    use cdk::secp256k1::hashes::Hash as CdkHashTrait;
    
    // Convert amount (already u64)
    let amount = cdk::Amount::from(sv2_request.amount);
    
    // Convert unit (should be "HASH")  
    let unit = cdk::nuts::CurrencyUnit::Hash;
    
    // Convert header hash from SV2 U256 to CDK Hash
    let header_hash_bytes = sv2_request.header_hash.inner_as_ref();
    let header_hash = CdkHashTrait::from_slice(header_hash_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid header hash: {}", e))?;
    
    // Convert description (optional)  
    let description = sv2_request.description.into_inner().map(|s| {
        String::from_utf8_lossy(s.inner_as_ref()).to_string()
    });
    
    // Convert locking key (compressed public key)
    let pubkey_bytes = sv2_request.locking_key.inner_as_ref();
    let pubkey = cdk::nuts::PublicKey::from_slice(pubkey_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid locking pubkey: {}", e))?;
    
    // Convert keyset ID from SV2 U256 to CDK format
    let keyset_id_bytes = sv2_request.keyset_id.inner_as_ref();
    let keyset_id = mining_sv2::cashu::keyset_from_sv2_bytes(keyset_id_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to convert keyset ID: {}", e))?;
    
    Ok(cdk::nuts::nutXX::MintQuoteMiningShareRequest {
        amount,
        unit,
        header_hash,
        description,
        pubkey,
        keyset_id,
    })
}