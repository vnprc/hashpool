use mining_sv2::{MintQuoteNotification, MintQuoteFailure};
use tracing::{debug, info, warn, error};
use cdk::wallet::Wallet;
use std::sync::Arc;

// Message type constants for extension messages
const MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION: u8 = 0xC0;
const MESSAGE_TYPE_MINT_QUOTE_FAILURE: u8 = 0xC1;

/// Handle extension messages from pool
pub async fn handle_extension_message(
    message_type: u8,
    payload: &[u8],
    wallet: Arc<Wallet>,
) -> Result<(), Box<dyn std::error::Error>> {
    debug!("🎯 Handling extension message type: 0x{:02x}, payload length: {}", message_type, payload.len());
    
    match message_type {
        MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION => {
            // Parse the notification
            let mut payload_copy = payload.to_vec();
            let notification: MintQuoteNotification = binary_sv2::from_bytes(&mut payload_copy)
                .map_err(|e| format!("Failed to parse MintQuoteNotification: {:?}", e))?;
            
            let share_hash = notification.share_hash.inner_as_ref().to_vec();
            let quote_id = String::from_utf8_lossy(
                notification.quote_id.inner_as_ref()
            ).to_string();
            
            debug!("Received mint quote {} for share {}", 
                  quote_id, hex::encode(&share_hash));
            
            match wallet.mint_quote_state_mining_share(&quote_id).await {
                Ok(_) => {
                    debug!("Persisted quote {} to wallet database", quote_id);
                }
                Err(e) => {
                    error!("Failed to fetch and persist quote {}: {:?}", quote_id, e);
                }
            }
            
            Ok(())
        }
        MESSAGE_TYPE_MINT_QUOTE_FAILURE => {
            // Parse the failure
            let mut payload_copy = payload.to_vec();
            let failure: MintQuoteFailure = binary_sv2::from_bytes(&mut payload_copy)
                .map_err(|e| format!("Failed to parse MintQuoteFailure: {:?}", e))?;
            
            warn!("Mint quote failed for share {:?}: {}", 
                  failure.share_hash.inner_as_ref(),
                  String::from_utf8_lossy(failure.error_message.inner_as_ref()));
            
            Ok(())
        }
        _ => {
            debug!("Unknown extension message type: 0x{:02x}", message_type);
            Ok(())
        }
    }
}