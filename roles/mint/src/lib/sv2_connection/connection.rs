use anyhow::Result;
use cdk::mint::Mint;
use codec_sv2::{HandshakeRole, StandardEitherFrame, StandardSv2Frame};
use noise_sv2::Initiator;
use const_sv2::MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS;
use key_utils::Secp256k1PublicKey;
use network_helpers_sv2::noise_connection::Connection;
use roles_logic_sv2::parsers_sv2::{AnyMessage, CommonMessages};
use shared_config::Sv2MessagingConfig;
use std::sync::Arc;
use tokio::net::TcpStream;
use tracing::info;

use super::{
    message_handler::handle_sv2_connection, setup_connection::build_mint_setup_connection,
    state_machine::ConnectionStateMachine,
};

/// Connect to pool via SV2 with Noise encryption
pub async fn connect_to_pool_sv2(mint: Arc<Mint>, sv2_config: Sv2MessagingConfig) {
    info!(
        "Connecting to pool SV2 endpoint: {}",
        sv2_config.mint_listen_address
    );

    loop {
        // Create fresh state machine for each connection attempt
        let mut state_machine = ConnectionStateMachine::new();

        match TcpStream::connect(&sv2_config.mint_listen_address).await {
            Ok(stream) => {
                // Transition to Connecting state after TCP establishment
                if let Err(e) = state_machine.tcp_connected() {
                    tracing::error!("State transition error: {}", e);
                    continue;
                }
                info!(
                    "✅ TCP connection established - state: {}",
                    state_machine.current_state()
                );

                match establish_sv2_connection(stream, &mut state_machine, &sv2_config).await {
                    Ok((receiver, sender)) => {
                        if let Err(e) = handle_sv2_connection(mint.clone(), receiver, sender).await
                        {
                            tracing::error!("SV2 connection error: {}", e);
                            state_machine.error(format!("Connection error: {}", e));
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to establish SV2 connection: {}", e);
                        state_machine.error(format!("Establishment error: {}", e));
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to connect to pool at {}: {}",
                    sv2_config.mint_listen_address,
                    e
                );
                state_machine.error(format!("TCP connection failed: {}", e));
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

/// Type alias for frames used in mint/pool communication
/// Uses AnyMessage to work with Connection channel types
type MintFrame = StandardEitherFrame<AnyMessage<'static>>;

/// Establish SV2 connection: Noise handshake + SetupConnection negotiation
/// This implements the full SV2 connection flow following the translator pattern
async fn establish_sv2_connection(
    stream: TcpStream,
    state_machine: &mut ConnectionStateMachine,
    sv2_config: &Sv2MessagingConfig,
) -> Result<(
    async_channel::Receiver<MintFrame>,
    async_channel::Sender<MintFrame>,
)> {
    // Phase 3a: Create Noise protocol initiator for mint role
    // Get the pool's public key from config
    let pool_public_key_str = sv2_config
        .pool_authority_public_key
        .as_ref()
        .ok_or_else(|| {
            anyhow::anyhow!("Pool authority public key not configured in sv2_messaging config")
        })?;

    // Deserialize the pool's public key using the same method as the pool config
    let pool_pub_key: Secp256k1PublicKey = pool_public_key_str
        .parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse pool public key: {:?}", e))?;

    let pool_public_key_bytes = pool_pub_key.into_bytes();
    let initiator = Initiator::from_raw_k(pool_public_key_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to create Initiator: {:?}", e))?;

    // Phase 3b: Perform full Noise handshake using network_helpers Connection
    // This establishes the encrypted channel with the pool
    // Using AnyMessage to support all SV2 message types during handshake
    let (receiver, sender) =
        Connection::new::<AnyMessage>(stream, HandshakeRole::Initiator(initiator))
            .await
            .map_err(|e| anyhow::anyhow!("Noise handshake failed: {:?}", e))?;

    // Transition state: Noise handshake complete
    state_machine
        .noise_handshake_complete()
        .map_err(|e| anyhow::anyhow!("Failed to transition to SetupInProgress: {}", e))?;
    info!(
        "🔐 Noise handshake complete - state: {}",
        state_machine.current_state()
    );

    // Phase 3c: Build and send SetupConnection message
    let setup_connection = build_mint_setup_connection("127.0.0.1", 3333, "mint-01")?;
    info!(
        "📋 Built SetupConnection: version {}-{}",
        setup_connection.min_version, setup_connection.max_version
    );

    // Convert SetupConnection to SV2 frame properly via AnyMessage
    // This is the correct way to wrap a message in an SV2 frame with header
    use roles_logic_sv2::parsers_sv2::CommonMessages;
    let sv2_frame: StandardSv2Frame<AnyMessage> =
        AnyMessage::Common(CommonMessages::SetupConnection(setup_connection))
            .try_into()
            .map_err(|e| anyhow::anyhow!("Failed to convert SetupConnection to frame: {:?}", e))?;

    let frame = StandardEitherFrame::Sv2(sv2_frame);

    sender
        .send(frame)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send SetupConnection: {}", e))?;

    info!("📤 Sent SetupConnection through encrypted channel");

    // Phase 3d: Receive and validate SetupConnection response from pool
    // The pool should respond with SetupConnectionSuccess
    match tokio::time::timeout(std::time::Duration::from_secs(10), receiver.recv()).await {
        Ok(Ok(response_frame)) => {
            info!("✅ Received SetupConnection response from pool");

            // Extract and validate the Sv2 frame
            // In Phase 3, we validate that we received a valid Sv2 response frame
            // In Phase 4, we can implement full SetupConnectionSuccess parsing
            match response_frame {
                StandardEitherFrame::Sv2(mut frame) => {
                    // Extract message type and payload
                    let header = frame.get_header().ok_or_else(|| {
                        anyhow::anyhow!("SetupConnectionSuccess response missing header")
                    })?;

                    let message_type = header.msg_type();
                    let payload = frame.payload().to_vec();

                    info!(
                        "✅ Pool responded with Sv2 frame (type: 0x{:02x}, {} bytes)",
                        message_type,
                        payload.len()
                    );

                    // Phase 4: Full SetupConnectionSuccess parsing
                    // Validate message type is SetupConnectionSuccess (0x1)
                    if message_type != MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS {
                        return Err(anyhow::anyhow!(
                            "Expected SetupConnectionSuccess (0x{:02x}), got 0x{:02x}",
                            MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
                            message_type
                        ));
                    }

                    // Parse the message from (message_type, payload) tuple
                    let mut payload_mut = payload.clone();
                    let message: AnyMessage = (message_type, payload_mut.as_mut_slice())
                        .try_into()
                        .map_err(|e| {
                            anyhow::anyhow!("Failed to parse SetupConnectionSuccess: {:?}", e)
                        })?;

                    // Convert to static lifetime for use in async context
                    let message_static = match message {
                        AnyMessage::Common(m) => match m {
                            CommonMessages::SetupConnectionSuccess(success_msg) => {
                                success_msg.into_static()
                            }
                            _ => {
                                return Err(anyhow::anyhow!(
                                    "Expected SetupConnectionSuccess, got different CommonMessage"
                                ));
                            }
                        },
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Expected CommonMessages variant, got different message type"
                            ));
                        }
                    };

                    // Validate protocol version
                    if message_static.used_version != 2 {
                        return Err(anyhow::anyhow!(
                            "Pool negotiated unsupported version: {} (expected 2)",
                            message_static.used_version
                        ));
                    }

                    info!("🔗 SetupConnectionSuccess validated");
                    info!("   - Negotiated version: {}", message_static.used_version);
                    info!("   - Pool feature flags: 0x{:08x}", message_static.flags);
                }
                StandardEitherFrame::HandShake(_) => {
                    return Err(anyhow::anyhow!(
                        "Unexpected HandShake frame in response, expected Sv2 frame"
                    ));
                }
            }
        }
        Ok(Err(e)) => {
            tracing::warn!(
                "Channel recv error waiting for SetupConnection response: {}",
                e
            );
            return Err(anyhow::anyhow!("Channel error receiving response: {}", e));
        }
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Timeout (10s) waiting for SetupConnection response from pool"
            ));
        }
    }

    // Transition state: SetupConnection accepted
    state_machine
        .setup_connection_accepted()
        .map_err(|e| anyhow::anyhow!("Failed to transition to Ready: {}", e))?;
    info!(
        "📋 SetupConnection negotiated - state: {}",
        state_machine.current_state()
    );

    // Create bounded channels for message handler
    // Bridge task will forward from encrypted connection channels
    let (msg_tx, msg_rx) = async_channel::bounded::<MintFrame>(100);

    // Spawn bridge task to forward encrypted frames to handler
    tokio::spawn(async move {
        while let Ok(frame) = receiver.recv().await {
            if let Err(e) = msg_tx.send(frame).await {
                tracing::error!("Bridge forward error: {}", e);
                break;
            }
        }
    });

    info!(
        "✅ Mint SV2 connection ready (state: {})",
        state_machine.current_state()
    );

    Ok((msg_rx, sender))
}
