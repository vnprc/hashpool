use std::sync::Arc;
use cdk::mint::Mint;
use shared_config::Sv2MessagingConfig;
use tokio::net::TcpStream;
use network_helpers_sv2::plain_connection_tokio::PlainConnection;
use tracing::info;
use anyhow::Result;

use super::message_handler::handle_sv2_connection;

/// Connect to pool via SV2 TCP connection and listen for quote requests
pub async fn connect_to_pool_sv2(
    mint: Arc<Mint>,
    sv2_config: Sv2MessagingConfig,
) {
    info!("Connecting to pool SV2 endpoint: {}", sv2_config.mint_listen_address);
    
    loop {
        match TcpStream::connect(&sv2_config.mint_listen_address).await {
            Ok(stream) => {
                info!("✅ Successfully connected to pool SV2 endpoint");
                
                // Create SV2 connection with plain connection helper
                let (receiver, sender) = PlainConnection::new(stream).await;
                
                if let Err(e) = handle_sv2_connection(mint.clone(), receiver, sender).await {
                    tracing::error!("SV2 connection error: {}", e);
                }
            },
            Err(e) => {
                tracing::warn!("❌ Failed to connect to pool SV2 endpoint: {:?}", e);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}