//! Mint service connection management
//!
//! Handles Noise-encrypted TCP connections with the Cashu mint service,
//! including handshake protocol and message framing.
//!
//! Phase 2 Implementation:
//! - Listens for incoming connections from the mint service
//! - Performs Noise protocol handshake as responder
//! - Sets up encrypted communication channel
//! - Prepares for Phase 3 message exchange

use async_channel::{Receiver, Sender};
use binary_sv2::from_bytes;
use mint_quote_sv2::{MESSAGE_TYPE_MINT_QUOTE_ERROR, MESSAGE_TYPE_MINT_QUOTE_RESPONSE};
use hex;
use mint_pool_messaging::{
    quote_request_frame_bytes, MintPoolMessageHub, MintQuoteError, MintQuoteResponse,
    ParsedMintQuoteRequest, Role,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{watch, RwLock},
};
use tracing::{debug, error, info, warn};

use key_utils::{Secp256k1PublicKey, Secp256k1SecretKey};
use noise_sv2::Responder;
use stratum_common::{
    network_helpers_sv2::noise_connection::Connection,
    roles_logic_sv2::{
        codec_sv2::{HandshakeRole, StandardEitherFrame, StandardSv2Frame},
        parsers_sv2::AnyMessage,
    },
};

/// Message type alias for mint protocol communication
/// Uses AnyMessage to support all SV2 message types during handshake
pub type MintMessage = AnyMessage<'static>;

/// Frame type for mint/pool communication
pub type MintFrame = StandardEitherFrame<MintMessage>;

/// Manages the connection to the mint service
pub struct MintConnection {
    /// Remote address of the mint service
    address: SocketAddr,
    /// Authority secret key for Noise protocol
    authority_secret_key: Secp256k1SecretKey,
    /// Authority public key for Noise protocol
    authority_public_key: Secp256k1PublicKey,
    /// Certificate validity duration
    cert_validity_duration: Duration,
    /// Sender for encrypted frames to mint (Arc<RwLock> for safe sharing)
    sender: Arc<RwLock<Option<Sender<MintFrame>>>>,
    /// Connection state
    is_connected: Arc<RwLock<bool>>,
}

impl MintConnection {
    /// Create a new mint connection instance with default authentication keys
    pub fn new(address: SocketAddr) -> Self {
        // Use default keys for basic connectivity
        // Phase 3: These should be loaded from pool config
        use secp256k1::rand::thread_rng;

        let secret_key = secp256k1::SecretKey::new(&mut thread_rng());
        let authority_secret_key = Secp256k1SecretKey(secret_key);
        let authority_public_key = Secp256k1PublicKey::from(authority_secret_key);

        Self {
            address,
            authority_secret_key,
            authority_public_key,
            cert_validity_duration: Duration::from_secs(3600), // 1 hour default
            sender: Arc::new(RwLock::new(None)),
            is_connected: Arc::new(RwLock::new(false)),
        }
    }

    /// Create a new mint connection instance with specified authentication keys
    pub fn with_keys(
        address: SocketAddr,
        authority_secret_key: Secp256k1SecretKey,
        authority_public_key: Secp256k1PublicKey,
        cert_validity_duration: Duration,
    ) -> Self {
        Self {
            address,
            authority_secret_key,
            authority_public_key,
            cert_validity_duration,
            sender: Arc::new(RwLock::new(None)),
            is_connected: Arc::new(RwLock::new(false)),
        }
    }

    /// Get the sender for the encrypted connection (once established)
    pub fn get_sender(&self) -> Arc<RwLock<Option<Sender<MintFrame>>>> {
        self.sender.clone()
    }

    /// Get mint service address
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Check if connected
    pub async fn is_connected(&self) -> bool {
        *self.is_connected.read().await
    }

    /// Establish connection with mint service
    ///
    /// Phase 2: Listens for an incoming connection from the mint
    /// and performs the SV2 Noise handshake as the responder.
    pub async fn establish_connection(
        &mut self,
        hub: Arc<MintPoolMessageHub>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            "🔗 Attempting to establish mint connection on {}",
            self.address
        );

        // Create TCP listener as the pool (responder)
        let listener = TcpListener::bind(self.address).await?;
        info!("📡 Listening for mint service on {}", self.address);

        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    info!("✓ Accepted connection from mint service: {}", peer_addr);

                    match self.perform_handshake(stream, hub.clone(), peer_addr).await {
                        Ok(()) => {
                            info!(
                                "Mint connection with {} closed; awaiting next attempt",
                                peer_addr
                            );
                        }
                        Err(e) => {
                            error!("❌ Handshake or connection error with {}: {}", peer_addr, e);
                            warn!("Waiting for next connection attempt...");
                        }
                    }
                }
                Err(e) => {
                    error!("❌ Error accepting connection: {}", e);
                    return Err(Box::new(e));
                }
            }
        }
    }

    /// Perform SV2 Noise handshake with mint service
    ///
    /// This establishes the Noise-encrypted channel and performs
    /// the SV2 SetupConnection/SetupConnectionSuccess exchange.
    /// Returns the sender for sending frames through the encrypted connection.
    async fn perform_handshake(
        &self,
        stream: TcpStream,
        hub: Arc<MintPoolMessageHub>,
        peer_addr: SocketAddr,
    ) -> Result<(), String> {
        debug!("Starting Noise handshake as responder...");

        // Create Noise responder for the handshake using authority keys
        let responder = Responder::from_authority_kp(
            &self.authority_public_key.clone().into_bytes(),
            &self.authority_secret_key.clone().into_bytes(),
            self.cert_validity_duration,
        )
        .map_err(|e| format!("Failed to create responder: {:?}", e))?;

        // Perform Noise connection handshake with mint service
        // This establishes the encrypted channel using Noise protocol
        // Uses AnyMessage to support the SV2 SetupConnection messages
        let (receiver, sender) =
            Connection::new::<MintMessage>(stream, HandshakeRole::Responder(responder))
                .await
                .map_err(|e| format!("Connection failed: {:?}", e))?;

        debug!("Noise handshake completed");
        info!(
            "✅ SV2 Noise handshake successful with mint at {}",
            peer_addr
        );

        // Phase 3: Receive SetupConnection from mint and respond with SetupConnectionSuccess
        let _setup_frame =
            tokio::time::timeout(std::time::Duration::from_secs(10), receiver.recv())
                .await
                .map_err(|_| "Timeout waiting for SetupConnection from mint".to_string())?
                .map_err(|e| format!("Error receiving SetupConnection: {}", e))?;

        debug!("Received SetupConnection frame from mint during handshake");
        info!("✅ SetupConnection received from mint");

        // Build SetupConnectionSuccess response
        use stratum_common::roles_logic_sv2::common_messages_sv2::SetupConnectionSuccess;

        let setup_success = SetupConnectionSuccess {
            flags: 0,
            used_version: 2,
        };

        // Wrap message in frame for transmission
        let response_msg = MintMessage::Common(
            stratum_common::roles_logic_sv2::parsers_sv2::CommonMessages::SetupConnectionSuccess(
                setup_success,
            ),
        );

        // Convert to StandardSv2Frame then wrap in EitherFrame
        let sv2_frame: stratum_common::roles_logic_sv2::codec_sv2::StandardSv2Frame<MintMessage> =
            response_msg
                .try_into()
                .map_err(|e| format!("Failed to convert response to frame: {:?}", e))?;

        // Wrap in EitherFrame
        let frame = stratum_common::roles_logic_sv2::codec_sv2::StandardEitherFrame::Sv2(sv2_frame);

        // Send response through encrypted channel
        sender
            .send(frame)
            .await
            .map_err(|e| format!("Failed to send SetupConnectionSuccess: {}", e))?;

        info!("✅ SetupConnectionSuccess sent to mint - handshake complete");

        {
            let mut sender_lock = self.sender.write().await;
            *sender_lock = Some(sender.clone());
        }
        *self.is_connected.write().await = true;

        let connection_id = format!("mint-{}", peer_addr);
        hub.register_connection(connection_id.clone(), Role::Pool)
            .await;

        let sender_arc = self.sender.clone();
        let hub_for_requests = hub.clone();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let forward_connection_id = connection_id.clone();
        let forward_handle = tokio::spawn(async move {
            forward_hub_requests_to_mint(
                hub_for_requests,
                sender_arc,
                forward_connection_id,
                shutdown_rx,
            )
            .await;
        });

        let processing_result = process_mint_frames(receiver, hub.clone()).await;

        let _ = shutdown_tx.send(true);
        let _ = forward_handle.await;

        hub.unregister_connection(&connection_id).await;
        {
            let mut sender_lock = self.sender.write().await;
            sender_lock.take();
        }
        *self.is_connected.write().await = false;

        if let Err(e) = &processing_result {
            error!("Error processing mint frames from {}: {}", peer_addr, e);
        }

        processing_result
    }
}

async fn forward_hub_requests_to_mint(
    hub: Arc<MintPoolMessageHub>,
    sender_arc: Arc<RwLock<Option<Sender<MintFrame>>>>,
    connection_id: String,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    match hub.subscribe_quote_requests().await {
        Ok(mut rx) => loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        debug!("Stopping quote forwarder for {} (shutdown signalled)", connection_id);
                        break;
                    }
                }
                result = rx.recv() => {
                    match result {
                        Ok(parsed_request) => {
                            if let Err(e) = send_quote_request_to_mint(&sender_arc, &parsed_request).await {
                                error!(
                                    "Failed to forward quote request via mint connection {}: {}",
                                    connection_id, e
                                );
                                if e.contains("unavailable") {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            debug!(
                                "Quote forwarder for {} exiting: broadcast receiver error: {}",
                                connection_id, e
                            );
                            break;
                        }
                    }
                }
            }
        },
        Err(e) => {
            error!(
                "Unable to subscribe to hub quote requests for mint bridge {}: {}",
                connection_id, e
            );
        }
    }
}

async fn send_quote_request_to_mint(
    sender_arc: &Arc<RwLock<Option<Sender<MintFrame>>>>,
    parsed: &ParsedMintQuoteRequest,
) -> Result<(), String> {
    let sender = {
        let guard = sender_arc.read().await;
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| "mint sender unavailable".to_string())?
    };

    debug!(
        "Forwarding mint quote request: share_hash={}",
        hex::encode(parsed.share_hash.as_bytes())
    );
    debug!(
        "Mint quote locking key: {}",
        hex::encode(parsed.request.locking_key.inner_as_ref())
    );

    let frame_bytes = quote_request_frame_bytes(&parsed.request)
        .map_err(|e| format!("failed to build quote request frame: {e}"))?;
    let frame = StandardSv2Frame::from_bytes_unchecked(frame_bytes.into());
    let frame = MintFrame::Sv2(frame);
    sender
        .send(frame)
        .await
        .map_err(|e| format!("failed to send quote request: {}", e))
}

async fn process_mint_frames(
    receiver: Receiver<MintFrame>,
    hub: Arc<MintPoolMessageHub>,
) -> Result<(), String> {
    let rx = receiver;
    while let Ok(frame) = rx.recv().await {
        match frame {
            MintFrame::Sv2(mut sv2_frame) => {
                let header = sv2_frame
                    .get_header()
                    .ok_or_else(|| "missing SV2 header".to_string())?;
                let msg_type = header.msg_type();
                let mut payload = sv2_frame.payload().to_vec();

                match msg_type {
                    MESSAGE_TYPE_MINT_QUOTE_RESPONSE => {
                        let response = decode_mint_quote_response(&mut payload)?;
                        hub.send_quote_response(response)
                            .await
                            .map_err(|e| format!("failed to dispatch quote response: {:?}", e))?;
                    }
                    MESSAGE_TYPE_MINT_QUOTE_ERROR => {
                        let error_msg = decode_mint_quote_error(&mut payload)?;
                        hub.send_quote_error(error_msg)
                            .await
                            .map_err(|e| format!("failed to dispatch quote error: {:?}", e))?;
                    }
                    other => {
                        debug!("Ignoring mint frame with msg_type=0x{:02x}", other);
                    }
                }
            }
            other => {
                debug!("Received non-SV2 frame from mint: {:?}", other);
            }
        }
    }

    Ok(())
}

fn decode_mint_quote_response(payload: &mut [u8]) -> Result<MintQuoteResponse<'static>, String> {
    let response: MintQuoteResponse =
        from_bytes(payload).map_err(|e| format!("failed to decode MintQuoteResponse: {:?}", e))?;
    Ok(response.into_static())
}

fn decode_mint_quote_error(payload: &mut [u8]) -> Result<MintQuoteError<'static>, String> {
    let error_msg: MintQuoteError =
        from_bytes(payload).map_err(|e| format!("failed to decode MintQuoteError: {:?}", e))?;
    Ok(error_msg.into_static())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_mint_connection_creation() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 34260);
        let conn = MintConnection::new(addr);

        assert_eq!(conn.address(), addr);
    }

    #[tokio::test]
    async fn test_mint_connection_not_connected_initially() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 34260);
        let conn = MintConnection::new(addr);

        assert!(!conn.is_connected().await);
    }
}
