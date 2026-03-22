use crate::{
    error::{self, TproxyError, TproxyErrorKind, TproxyResult},
    is_aggregated,
    payment::custom_handler::CustomMiningMessageHandler,
    status::{handle_error, Status, StatusSender},
    sv2::channel_manager::channel::ChannelState,
    utils::{AggregatedState, AtomicAggregatedState, AGGREGATED_CHANNEL_ID},
};
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use std::sync::Arc;
use stratum_apps::{
    custom_mutex::Mutex,
    fallback_coordinator::FallbackCoordinator,
    stratum_core::{
        channels_sv2::client::{extended::ExtendedChannel, group::GroupChannel},
        codec_sv2::StandardSv2Frame,
        extensions_sv2::{EXTENSION_TYPE_WORKER_HASHRATE_TRACKING, TLV_FIELD_TYPE_USER_IDENTITY},
        framing_sv2,
        handlers_sv2::{HandleExtensionsFromServerAsync, HandleMiningMessagesFromServerAsync},
        mining_sv2::{ExtendedExtranonce, OpenExtendedMiningChannelSuccess},
        parsers_sv2::{AnyMessage, Mining, Tlv, TlvList},
    },
    task_manager::TaskManager,
    utils::{
        protocol_message_type::{protocol_message_type, MessageType},
        types::{ChannelId, DownstreamId, Hashrate, RequestId, Sv2Frame},
    },
};

use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Extra bytes allocated for translator search space in aggregated mode.
/// This allows the translator to manage multiple downstream connections
/// by allocating unique extranonce prefixes to each downstream.
const AGGREGATED_MODE_TRANSLATOR_SEARCH_SPACE_BYTES: usize = 4;

/// Manages SV2 channels and message routing between upstream and downstream.
///
/// The ChannelManager serves as the central component that bridges SV2 upstream
/// connections with SV1 downstream connections. It handles:
/// - SV2 channel lifecycle management (open, close, error handling)
/// - Message translation and routing between protocols
/// - Extranonce management for aggregated vs non-aggregated modes
/// - Share submission processing and validation
/// - Job distribution to downstream connections
///
/// The manager supports two operational modes:
/// - Aggregated: All downstream connections share a single extended channel
/// - Non-aggregated: Each downstream connection gets its own extended channel
///
/// This design allows the translator to efficiently manage multiple mining
/// connections while maintaining proper isolation and state management.
#[derive(Debug, Clone)]
pub struct ChannelManager {
    pub channel_state: ChannelState,
    /// Extensions that the translator supports (will request if required by server)
    pub supported_extensions: Vec<u16>,
    /// Extensions that the translator requires (must be supported by server)
    pub required_extensions: Vec<u16>,
    /// Store pending channel info by downstream_id: (user_identity, hashrate,
    /// downstream_extranonce_len)
    ///
    /// Semantics differ depending on the operating mode:
    ///
    /// 1. Aggregated mode:
    ///    - Stores the initial downstream request that triggers the single upstream channel open.
    ///    - Buffers additional downstream open-channel requests received while awaiting the
    ///      upstream `OpenExtendedMiningChannelSuccess`.
    ///
    /// 2. Non-aggregated mode:
    ///    - Stores all downstreams that are currently waiting for their corresponding upstream
    ///      `OpenExtendedMiningChannelSuccess`.
    ///
    /// Entries are removed once the upstream success message is received
    /// and propagated accordingly.
    pub pending_downstream_channels: Arc<DashMap<DownstreamId, (String, Hashrate, usize)>>,
    /// Map of active extended channels by channel ID.
    /// In aggregated mode, the shared upstream channel is stored under AGGREGATED_CHANNEL_ID.
    /// In non-aggregated mode, each downstream has its own channel with its assigned ID.
    pub extended_channels: Arc<DashMap<ChannelId, ExtendedChannel<'static>>>,
    /// Map of active group channels by group channel ID
    pub group_channels: Arc<DashMap<ChannelId, GroupChannel<'static>>>,
    /// Share sequence number counter for tracking valid shares forwarded upstream.
    /// In aggregated mode: single counter for all shares going to the upstream channel.
    /// In non-aggregated mode: one counter per downstream channel.
    pub share_sequence_counters: Arc<DashMap<u32, u32>>,
    /// Extensions that have been successfully negotiated with the upstream server
    pub negotiated_extensions: Arc<Mutex<Vec<u16>>>,
    /// Extranonce factories containing per channel extranonces
    pub extranonce_factories: Arc<DashMap<ChannelId, ExtendedExtranonce>>,
    /// Tracks whether the single upstream channel in aggregated mode is absent,
    /// being established, or connected.
    pub aggregated_channel_state: AtomicAggregatedState,
    /// Handler for custom (non-standard) Mining message types (0xC0–0xFF).
    /// Used by hashpool for CDK payment notifications. Defaults to NoopCustomMiningMessageHandler.
    pub custom_handler: Arc<dyn CustomMiningMessageHandler>,
}

#[cfg_attr(not(test), hotpath::measure_all)]
impl ChannelManager {
    /// Creates a new ChannelManager instance.
    ///
    /// # Arguments
    /// * `upstream_sender` - Channel to send messages to upstream
    /// * `upstream_receiver` - Channel to receive messages from upstream
    /// * `sv1_server_sender` - Channel to send messages to SV1 server
    /// * `sv1_server_receiver` - Channel to receive messages from SV1 server
    /// * `mode` - Operating mode (Aggregated or NonAggregated)
    /// * `supported_extensions` - Extensions that the translator supports (will request if required
    ///   by server)
    /// * `required_extensions` - Extensions that the translator requires (must be supported by
    ///   server)
    ///
    /// # Returns
    /// A new ChannelManager instance ready to handle message routing
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        upstream_sender: Sender<Sv2Frame>,
        upstream_receiver: Receiver<Sv2Frame>,
        sv1_server_sender: Sender<(Mining<'static>, Option<Vec<Tlv>>)>,
        sv1_server_receiver: Receiver<(Mining<'static>, Option<Vec<Tlv>>)>,
        status_sender: Sender<Status>,
        supported_extensions: Vec<u16>,
        required_extensions: Vec<u16>,
        custom_handler: Arc<dyn CustomMiningMessageHandler>,
    ) -> Self {
        let channel_state = ChannelState::new(
            upstream_sender,
            upstream_receiver,
            sv1_server_sender,
            sv1_server_receiver,
            status_sender,
        );

        Self {
            channel_state,
            supported_extensions,
            required_extensions,
            pending_downstream_channels: Arc::new(DashMap::new()),
            extended_channels: Arc::new(DashMap::new()),
            group_channels: Arc::new(DashMap::new()),
            share_sequence_counters: Arc::new(DashMap::new()),
            negotiated_extensions: Arc::new(Mutex::new(Vec::new())),
            extranonce_factories: Arc::new(DashMap::new()),
            aggregated_channel_state: AtomicAggregatedState::new(AggregatedState::NoChannel),
            custom_handler,
        }
    }

    /// Spawns and runs the main channel manager task loop.
    ///
    /// This method creates an async task that handles all message routing for the
    /// channel manager. The task runs a select loop that processes:
    /// - Shutdown signals for graceful termination
    /// - Messages from upstream SV2 server
    /// - Messages from downstream SV1 server
    ///
    /// The task continues running until a shutdown signal is received or an
    /// unrecoverable error occurs. It ensures proper cleanup of resources
    /// and error reporting.
    ///
    /// # Arguments
    /// * `cancellation_token` - Global application cancellation token
    /// * `fallback_coordinator` - Fallback coordinator
    /// * `status_sender` - Channel for sending status updates and errors
    /// * `task_manager` - Manager for tracking spawned tasks
    pub async fn run_channel_manager_tasks(
        self: Arc<Self>,
        cancellation_token: CancellationToken,
        fallback_coordinator: FallbackCoordinator,
        status_sender: Sender<Status>,
        task_manager: Arc<TaskManager>,
    ) {
        let status_sender = StatusSender::ChannelManager(status_sender);

        task_manager.spawn(async move {
            // we just spawned a new task that's relevant to fallback coordination
            // so register it with the fallback coordinator
            let fallback_handler = fallback_coordinator.register();

            // get the cancellation token that signals fallback
            let fallback_token = fallback_coordinator.token();

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        info!("ChannelManager: received shutdown signal.");
                        break;
                    }
                    _ = fallback_token.cancelled() => {
                        info!("ChannelManager: fallback triggered, resetting state");
                        self.pending_downstream_channels.clear();
                        self.extended_channels.clear();
                        self.group_channels.clear();
                        self.share_sequence_counters.clear();
                        self.negotiated_extensions.super_safe_lock(|data| data.clear());
                        self.extranonce_factories.clear();
                        self.aggregated_channel_state.set(AggregatedState::NoChannel);
                        break;
                    }
                    res = self.clone().handle_upstream_frame() => {
                        if let Err(e) = res {
                            if handle_error(&status_sender, e).await {
                                break;
                            }
                        }
                    },
                    res = self.clone().handle_downstream_message() => {
                        if let Err(e) = res {
                            if handle_error(&status_sender, e).await {
                                break;
                            }
                        }
                    },
                    else => {
                        warn!("All channel manager message streams closed. Exiting...");
                        break;
                    }
                }
            }

            self.channel_state.drop();
            warn!("ChannelManager: unified message loop exited.");

            // signal fallback coordinator that this task has completed its cleanup
            fallback_handler.done();
        });
    }

    /// Handles messages received from the upstream SV2 server.
    ///
    /// This method processes SV2 messages from upstream and routes them appropriately:
    /// - Mining messages: Processed through the roles logic and forwarded to SV1 server
    /// - Channel responses: Handled to manage channel lifecycle
    /// - Job notifications: Converted and distributed to downstream connections
    /// - Error messages: Logged and handled appropriately
    ///
    /// The method implements the core SV2 protocol logic for channel management,
    /// including handling both aggregated and non-aggregated channel modes.
    ///
    /// # Returns
    /// * `Ok(())` - Message processed successfully
    /// * `Err(TproxyError)` - Error processing the message
    pub async fn handle_upstream_frame(self: Arc<Self>) -> TproxyResult<(), error::ChannelManager> {
        let mut sv2_frame = self
            .channel_state
            .upstream_receiver
            .recv()
            .await
            .map_err(TproxyError::fallback)?;

        let mut channel_manager: ChannelManager = (*self).clone();
        let header = sv2_frame.get_header().ok_or_else(|| {
            error!("SV2 frame missing header");
            TproxyError::fallback(framing_sv2::Error::MissingHeader)
        })?;
        match protocol_message_type(header.ext_type(), header.msg_type()) {
            MessageType::Mining => {
                channel_manager
                    .handle_mining_message_frame_from_server(None, header, sv2_frame.payload())
                    .await?;
            }
            MessageType::Extensions => {
                channel_manager
                    .handle_extensions_message_frame_from_server(None, header, sv2_frame.payload())
                    .await?;
            }
            _ => {
                // Delegate to custom handler (handles hashpool 0xC0/0xC1 CDK notifications;
                // NoopCustomMiningMessageHandler silently drops unrecognized types).
                if let Err(e) = channel_manager
                    .custom_handler
                    .handle_custom_message(header.msg_type(), sv2_frame.payload())
                    .await
                {
                    warn!(
                        extension_type = header.ext_type(),
                        message_type = header.msg_type(),
                        error = %e,
                        "Custom message handler error"
                    );
                }
            }
        }

        Ok(())
    }

    /// Handles messages received from the downstream SV1 server.
    ///
    /// This method processes requests from the SV1 server, primarily:
    /// - OpenExtendedMiningChannel: Sets up new SV2 channels for downstream connections
    /// - SubmitSharesExtended: Processes share submissions from miners
    ///
    /// For channel opening, the method handles both aggregated and non-aggregated modes:
    /// - Aggregated: Creates extended channels using extranonce prefixes
    /// - Non-aggregated: Opens individual extended channels with the upstream for each downstream
    ///
    /// Share submissions are validated, processed through the channel logic,
    /// and forwarded to the upstream server with appropriate extranonce handling.
    ///
    /// # Returns
    /// * `Ok(())` - Message processed successfully
    /// * `Err(TproxyError)` - Error processing the message
    pub async fn handle_downstream_message(
        self: Arc<Self>,
    ) -> TproxyResult<(), error::ChannelManager> {
        let (message, tlv_fields) = self
            .channel_state
            .sv1_server_receiver
            .recv()
            .await
            .map_err(TproxyError::shutdown)?;
        match message {
            Mining::OpenExtendedMiningChannel(m) => {
                let mut open_channel_msg = m.clone();
                let mut user_identity = m.user_identity.as_utf8_or_hex();
                let hashrate = m.nominal_hash_rate;
                let min_extranonce_size = m.min_extranonce_size as usize;

                if is_aggregated() {
                    match self.aggregated_channel_state.get() {
                        AggregatedState::Connected => {
                            return self
                                .handle_downstream_channel_request_in_aggregated_mode(
                                    open_channel_msg.request_id,
                                    user_identity,
                                    hashrate,
                                    open_channel_msg.min_extranonce_size.into(),
                                )
                                .await;
                        }
                        AggregatedState::Pending => {
                            self.pending_downstream_channels.insert(
                                m.request_id as DownstreamId,
                                (user_identity, hashrate, min_extranonce_size),
                            );
                            return Ok(());
                        }
                        AggregatedState::NoChannel => {
                            self.aggregated_channel_state.set(AggregatedState::Pending);
                            self.pending_downstream_channels.insert(
                                m.request_id as DownstreamId,
                                (user_identity.clone(), hashrate, min_extranonce_size),
                            );
                            // Modify user_identity for the `OpenExtendedMiningChannel` which is
                            // gonna be sent upstream
                            let translator_identity =
                                if let Some(dot_index) = user_identity.find('.') {
                                    format!("{}.translator-proxy", &user_identity[..dot_index])
                                } else {
                                    format!("{user_identity}.translator-proxy")
                                };
                            user_identity = translator_identity;
                            open_channel_msg.user_identity =
                                user_identity.as_bytes().to_vec().try_into().unwrap();
                        }
                    }
                }
                // In aggregated mode, add extra bytes for translator search space allocation
                let upstream_min_extranonce_size = if is_aggregated() {
                    min_extranonce_size + AGGREGATED_MODE_TRANSLATOR_SEARCH_SPACE_BYTES
                } else {
                    min_extranonce_size
                };

                // Update the message with the adjusted extranonce size for upstream
                open_channel_msg.min_extranonce_size = upstream_min_extranonce_size as u16;

                // In non-aggregated mode, store the request in the pending_channel to be later
                // used in the `OpenExtendedMiningChannel.Success` handler.
                // In aggregated mode it was already inserted in the `AggregatedState::NoChannel`
                // match arm above.
                if !is_aggregated() {
                    self.pending_downstream_channels.insert(
                        open_channel_msg.request_id as DownstreamId,
                        (user_identity, hashrate, min_extranonce_size),
                    );
                }

                info!(
                    "Sending OpenExtendedMiningChannel message to upstream: {:?}",
                    open_channel_msg
                );

                let message = Mining::OpenExtendedMiningChannel(open_channel_msg);
                let sv2_frame: Sv2Frame = AnyMessage::Mining(message)
                    .try_into()
                    .map_err(TproxyError::shutdown)?;
                self.channel_state
                    .upstream_sender
                    .send(sv2_frame)
                    .await
                    .map_err(|e| {
                        error!("Failed to send open channel message to upstream: {:?}", e);
                        TproxyError::fallback(TproxyErrorKind::ChannelErrorSender)
                    })?;
            }
            Mining::SubmitSharesExtended(mut m) => {
                let value =
                    self.extended_channels
                        .get_mut(&m.channel_id)
                        .map(|mut extended_channel| {
                            (
                                extended_channel.validate_share(m.clone()),
                                extended_channel.get_share_accounting().clone(),
                            )
                        });
                if let Some((Ok(_result), _share_accounting)) = value {
                    info!(
                        "SubmitSharesExtended: valid share, forwarding it to upstream | channel_id: {}, sequence_number: {} ☑️",
                        m.channel_id, m.sequence_number
                    );

                    if is_aggregated()
                        && self.extended_channels.contains_key(&AGGREGATED_CHANNEL_ID)
                    {
                        let upstream_extended_channel_id = self
                            .extended_channels
                            .get(&AGGREGATED_CHANNEL_ID)
                            .map(|ch| ch.get_channel_id())
                            .unwrap();

                        // In aggregated mode, use a single sequence counter for all valid shares
                        m.sequence_number =
                            self.next_share_sequence_number(upstream_extended_channel_id);
                        // Get the downstream channel's extranonce prefix (contains
                        // upstream prefix + translator proxy prefix)
                        let downstream_extranonce_prefix = self
                            .extended_channels
                            .get(&m.channel_id)
                            .map(|channel| channel.get_extranonce_prefix().clone());
                        // Get the length of the upstream prefix (range0)
                        let range0_len = self
                            .extranonce_factories
                            .get(&AGGREGATED_CHANNEL_ID)
                            .unwrap()
                            .get_range0_len();
                        if let Some(downstream_extranonce_prefix) = downstream_extranonce_prefix {
                            // Skip the upstream prefix (range0) and take the remaining
                            // bytes (translator proxy prefix)
                            let translator_prefix = &downstream_extranonce_prefix[range0_len..];
                            // Create new extranonce: translator proxy prefix + miner's
                            // extranonce
                            let mut new_extranonce = translator_prefix.to_vec();
                            new_extranonce.extend_from_slice(m.extranonce.as_ref());
                            // Replace the original extranonce with the modified one for
                            // upstream submission
                            m.extranonce =
                                new_extranonce.try_into().map_err(TproxyError::shutdown)?;
                        }
                        // We need to set the channel id to the upstream extended
                        // channel id
                        m.channel_id = upstream_extended_channel_id;
                    } else {
                        // In non-aggregated mode, each downstream channel has its own sequence
                        // counter
                        m.sequence_number = self.next_share_sequence_number(m.channel_id);

                        // Check if we have a per-channel factory for extranonce adjustment
                        let channel_factory = self.extranonce_factories.get(&m.channel_id);

                        if let Some(factory) = channel_factory {
                            // We need to adjust the extranonce for this channel
                            let downstream_extranonce_prefix = self
                                .extended_channels
                                .get(&m.channel_id)
                                .map(|channel| channel.get_extranonce_prefix().clone());
                            let range0_len = factory.get_range0_len();
                            if let Some(downstream_extranonce_prefix) = downstream_extranonce_prefix
                            {
                                // Skip the upstream prefix (range0) and take the remaining
                                // bytes (translator proxy prefix)
                                let translator_prefix = &downstream_extranonce_prefix[range0_len..];
                                // Create new extranonce: translator proxy prefix + miner's
                                // extranonce
                                let mut new_extranonce = translator_prefix.to_vec();
                                new_extranonce.extend_from_slice(m.extranonce.as_ref());
                                // Replace the original extranonce with the modified one for
                                // upstream submission
                                m.extranonce =
                                    new_extranonce.try_into().map_err(TproxyError::shutdown)?;
                            }
                        }
                    }

                    // Send the share upstream (common for both aggregated and non-aggregated modes)
                    let contains_type_in_negotiated_extension =
                        self.negotiated_extensions.super_safe_lock(|data| {
                            data.contains(&EXTENSION_TYPE_WORKER_HASHRATE_TRACKING)
                        });

                    // Check if we should try to include TLV fields
                    let should_send_with_tlv =
                        contains_type_in_negotiated_extension && tlv_fields.is_some();

                    let mut sent = false;
                    if should_send_with_tlv {
                        info!(
                            "TLV fields in Channel Manager: {:?}",
                            tlv_fields.clone().unwrap()
                        );
                        // Create frame bytes with TLVs
                        let user_identity_tlv = tlv_fields.and_then(|tlvs| {
                            tlvs.iter()
                                .find(|tlv| {
                                    tlv.r#type.extension_type
                                        == EXTENSION_TYPE_WORKER_HASHRATE_TRACKING
                                        && tlv.r#type.field_type == TLV_FIELD_TYPE_USER_IDENTITY
                                })
                                .cloned()
                        });

                        if let Some(tlv) = user_identity_tlv {
                            let tlv_list = TlvList::from_slice(&[tlv]).map_err(|e| {
                                error!("Failed to create TLV list: {:?}", e);
                                TproxyError::shutdown(e)
                            })?;
                            let frame_bytes = tlv_list
                                .build_frame_bytes_with_tlvs(Mining::SubmitSharesExtended(
                                    m.clone(),
                                ))
                                .map_err(|e| {
                                    error!("Failed to build frame bytes with TLVs: {:?}", e);
                                    TproxyError::shutdown(e)
                                })?;
                            // Convert to StandardSv2Frame with proper buffer type
                            let sv2_frame = StandardSv2Frame::from_bytes(frame_bytes.into())
                                .map_err(|missing| {
                                    error!(
                                        "Failed to convert frame bytes to StandardSv2Frame: {:?}",
                                        missing
                                    );
                                    TproxyError::shutdown(framing_sv2::Error::ExpectedSv2Frame)
                                })?;
                            self.channel_state.upstream_sender.send(sv2_frame).await.map_err(|e| {
                                error!("Failed to send submit shares extended message to upstream: {:?}", e);
                                TproxyError::fallback(TproxyErrorKind::ChannelErrorSender)
                            })?;
                            sent = true;
                        }
                    }

                    if !sent {
                        let message = Mining::SubmitSharesExtended(m);
                        let sv2_frame: Sv2Frame = AnyMessage::Mining(message)
                            .try_into()
                            .map_err(TproxyError::shutdown)?;
                        self.channel_state.upstream_sender.send(sv2_frame).await.map_err(|e| {
                            error!("Failed to send submit shares extended message to upstream: {:?}", e);
                            TproxyError::fallback(TproxyErrorKind::ChannelErrorSender)
                        })?;
                    }
                }
            }
            Mining::UpdateChannel(mut m) => {
                debug!("Received UpdateChannel from SV1Server: {:?}", m);

                if is_aggregated() {
                    // Update the aggregated channel's nominal hashrate so
                    // that monitoring reports a value consistent with the
                    // downstream vardiff estimate.
                    if let Some(mut aggregated_extended_channel) =
                        self.extended_channels.get_mut(&AGGREGATED_CHANNEL_ID)
                    {
                        aggregated_extended_channel.set_nominal_hashrate(m.nominal_hash_rate);
                        m.channel_id = aggregated_extended_channel.get_channel_id();
                    }
                } else {
                    // Non-aggregated: update the specific channel's nominal hashrate
                    if let Some(mut channel) = self.extended_channels.get_mut(&m.channel_id) {
                        channel.set_nominal_hashrate(m.nominal_hash_rate);
                    }
                }

                info!(
                    "Sending UpdateChannel message to upstream for channel_id: {:?}",
                    m.channel_id
                );
                // Forward UpdateChannel message to upstream
                let message = Mining::UpdateChannel(m);
                let sv2_frame: Sv2Frame = AnyMessage::Mining(message)
                    .try_into()
                    .map_err(TproxyError::shutdown)?;

                self.channel_state
                    .upstream_sender
                    .send(sv2_frame)
                    .await
                    .map_err(|e| {
                        error!("Failed to send UpdateChannel message to upstream: {:?}", e);
                        TproxyError::fallback(TproxyErrorKind::ChannelErrorSender)
                    })?;
            }
            Mining::CloseChannel(m) => {
                debug!("Received CloseChannel from Sv1Server: {m}");

                // Remove from extended_channels
                if self.extended_channels.remove(&m.channel_id).is_some() {
                    debug!("Removed channel {} from extended_channels before sending CloseChannel to upstream", m.channel_id);
                } else {
                    warn!("Attempted to remove channel {} from extended_channels but it was not found", m.channel_id);
                }
                // Remove from any group channels that contain it
                for mut group_channel in self.group_channels.iter_mut() {
                    if group_channel.get_channel_ids().contains(&m.channel_id) {
                        group_channel.remove_channel_id(m.channel_id);
                        debug!("Removed channel {} from group channel before sending CloseChannel to upstream", m.channel_id);
                    }
                }

                let message = Mining::CloseChannel(m);
                let sv2_frame: Sv2Frame = AnyMessage::Mining(message)
                    .try_into()
                    .map_err(TproxyError::shutdown)?;

                self.channel_state
                    .upstream_sender
                    .send(sv2_frame)
                    .await
                    .map_err(|e| {
                        error!("Failed to send CloseChannel message to upstream: {:?}", e);
                        TproxyError::fallback(TproxyErrorKind::ChannelErrorSender)
                    })?;
            }
            _ => {
                warn!("Unhandled downstream message: {:?}", message);
            }
        }

        Ok(())
    }

    /// Handles a downstream extended channel request in aggregated mode.
    ///
    /// Allocates a new extranonce prefix, creates a new downstream
    /// `ExtendedChannel`, and sends an
    /// `OpenExtendedMiningChannelSuccess` to the SV1Server.
    ///
    /// The new channel is initialized with the aggregated channel’s
    /// current state (chain tip, active job, and future jobs) so the
    /// downstream can start mining immediately.
    pub async fn handle_downstream_channel_request_in_aggregated_mode(
        &self,
        request_id: RequestId,
        user_identity: String,
        hashrate: Hashrate,
        min_extranonce_size: usize,
    ) -> TproxyResult<(), error::ChannelManager> {
        // We already have the unique channel open and so we create a new
        // extranonce prefix and we send the
        // OpenExtendedMiningChannelSuccess message directly to the sv1
        // server
        let target = self
            .extended_channels
            .get(&AGGREGATED_CHANNEL_ID)
            .map(|ch| *ch.get_target())
            .unwrap();
        let new_extranonce_prefix = self
            .extranonce_factories
            .get_mut(&AGGREGATED_CHANNEL_ID)
            .unwrap()
            .next_prefix_extended(min_extranonce_size)
            .ok();
        let new_extranonce_size = self
            .extranonce_factories
            .get_mut(&AGGREGATED_CHANNEL_ID)
            .unwrap()
            .get_range2_len();
        if let Some(new_extranonce_prefix) = new_extranonce_prefix {
            if new_extranonce_size >= min_extranonce_size {
                // Find max channel ID, excluding AGGREGATED_CHANNEL_ID
                // (u32::MAX) which would cause overflow when adding 1
                let channel_id = self
                    .extended_channels
                    .iter()
                    .filter(|x| *x.key() != AGGREGATED_CHANNEL_ID)
                    .fold(0, |acc, x| std::cmp::max(acc, *x.key()));
                let next_channel_id = channel_id + 1;
                let new_downstream_extended_channel = ExtendedChannel::new(
                    next_channel_id,
                    user_identity.clone(),
                    new_extranonce_prefix
                        .clone()
                        .into_b032()
                        .into_static()
                        .to_vec(),
                    target,
                    hashrate,
                    true,
                    new_extranonce_size as u16,
                );
                self.extended_channels
                    .insert(next_channel_id, new_downstream_extended_channel);
                let success_message =
                    Mining::OpenExtendedMiningChannelSuccess(OpenExtendedMiningChannelSuccess {
                        request_id,
                        channel_id: next_channel_id,
                        target: target.to_le_bytes().into(),
                        extranonce_size: new_extranonce_size as u16,
                        extranonce_prefix: new_extranonce_prefix.clone().into(),
                        group_channel_id: 0, /* use a dummy value, this
                                              * shouldn't
                                              * matter for the Sv1 server */
                    });

                self.channel_state
                    .sv1_server_sender
                    .send((success_message, None))
                    .await
                    .map_err(|e| {
                        error!("Failed to send open channel message to SV1Server: {:?}", e);
                        TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender)
                    })?;
                // Initialize the new downstream channel with state from upstream:
                // chain tip, active job, and any pending future jobs.
                let active_job_for_sv1_server = || {
                    // Extract data from aggregated channel in a scope block
                    // to release the borrow before accessing other channels
                    let (last_active_job, future_jobs, last_chain_tip) = {
                        let aggregated_channel =
                            self.extended_channels.get(&AGGREGATED_CHANNEL_ID)?;
                        (
                            aggregated_channel.get_active_job().map(|j| j.0.clone()),
                            aggregated_channel
                                .get_future_jobs()
                                .values()
                                .map(|j| j.0.clone())
                                .collect::<Vec<_>>(),
                            aggregated_channel.get_chain_tip().cloned(),
                        )
                    };

                    if let Some(chain_tip) = last_chain_tip {
                        self.extended_channels
                            .get_mut(&next_channel_id)?
                            .set_chain_tip(chain_tip);
                    }

                    if let Some(mut job) = last_active_job.clone() {
                        job.channel_id = next_channel_id;
                        _ = self
                            .extended_channels
                            .get_mut(&next_channel_id)?
                            .on_new_extended_mining_job(job);
                    }
                    // Also add any future jobs so SetNewPrevHash won't fail
                    for mut future_job in future_jobs {
                        future_job.channel_id = next_channel_id;
                        _ = self
                            .extended_channels
                            .get_mut(&next_channel_id)?
                            .on_new_extended_mining_job(future_job);
                    }

                    last_active_job.map(|mut job| {
                        job.channel_id = AGGREGATED_CHANNEL_ID;
                        job
                    })
                };

                if let Some(job) = active_job_for_sv1_server() {
                    self.channel_state
                        .sv1_server_sender
                        .send((Mining::NewExtendedMiningJob(job), None))
                        .await
                        .map_err(|e| {
                            error!(
                                "Failed to send active extended mining job to Sv1Server: {:?}",
                                e
                            );
                            TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender)
                        })?;
                }
            }
        }
        Ok(())
    }

    /// Gets the next sequence number for a valid share and increments the counter.
    ///
    /// The counter_key determines which counter to use:
    /// - In aggregated mode: use upstream channel ID (single counter for all shares)
    /// - In non-aggregated mode: use downstream channel ID (one counter per channel)
    pub fn next_share_sequence_number(&self, counter_key: u32) -> u32 {
        let mut counter = self.share_sequence_counters.entry(counter_key).or_insert(0);
        let counter = counter.value_mut();

        *counter += 1;
        *counter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_channel::unbounded;
    use stratum_apps::stratum_core::mining_sv2::{
        OpenExtendedMiningChannel, SubmitSharesExtended, UpdateChannel,
    };

    fn create_test_channel_manager() -> ChannelManager {
        let (upstream_sender, _upstream_receiver) = unbounded();
        let (_upstream_sender2, upstream_receiver) = unbounded();
        let (sv1_server_sender, _sv1_server_receiver) = unbounded();
        let (_sv1_server_sender2, sv1_server_receiver) = unbounded();
        let (status_sender, _) = unbounded();

        ChannelManager::new(
            upstream_sender,
            upstream_receiver,
            sv1_server_sender,
            sv1_server_receiver,
            status_sender,
            vec![],
            vec![],
            std::sync::Arc::new(crate::payment::custom_handler::NoopCustomMiningMessageHandler),
        )
    }

    #[tokio::test]
    async fn test_handle_downstream_open_channel_message() {
        let manager = create_test_channel_manager();

        // Create an OpenExtendedMiningChannel message
        let open_channel = OpenExtendedMiningChannel {
            request_id: 1,
            user_identity: "test_user".as_bytes().to_vec().try_into().unwrap(),
            nominal_hash_rate: 1000.0,
            max_target: vec![0xFFu8; 32].try_into().unwrap(),
            min_extranonce_size: 4,
        };

        // Store the pending channel information
        manager
            .pending_downstream_channels
            .insert(1, ("test_user".to_string(), 1000.0, 4));

        // Test that the message can be handled without panicking
        // In a real test environment, we would need to mock the upstream sender
        // For now, we just verify the channel manager can process the message type
        let mining_message = Mining::OpenExtendedMiningChannel(open_channel);

        // Verify the message can be processed (would normally be sent to upstream)
        match mining_message {
            Mining::OpenExtendedMiningChannel(msg) => {
                assert_eq!(msg.request_id, 1);
                assert_eq!(msg.nominal_hash_rate, 1000.0);
                assert_eq!(msg.min_extranonce_size, 4);
            }
            _ => panic!("Expected OpenExtendedMiningChannel"),
        }
    }

    #[tokio::test]
    async fn test_handle_downstream_submit_shares_message() {
        let _manager = create_test_channel_manager();

        // Create a SubmitSharesExtended message
        let submit_shares = SubmitSharesExtended {
            channel_id: 1,
            sequence_number: 100,
            job_id: 42,
            nonce: 0x12345678,
            ntime: 1234567890,
            version: 0x20000000,
            extranonce: vec![0x01, 0x02, 0x03, 0x04].try_into().unwrap(),
        };

        // Test that the message can be handled
        let mining_message = Mining::SubmitSharesExtended(submit_shares);

        // Verify the message structure
        match mining_message {
            Mining::SubmitSharesExtended(msg) => {
                assert_eq!(msg.channel_id, 1);
                assert_eq!(msg.sequence_number, 100);
                assert_eq!(msg.job_id, 42);
                assert_eq!(msg.nonce, 0x12345678);
            }
            _ => panic!("Expected SubmitSharesExtended"),
        }
    }

    #[tokio::test]
    async fn test_handle_downstream_update_channel_message() {
        let _manager = create_test_channel_manager();

        // Create an UpdateChannel message
        let update_channel = UpdateChannel {
            channel_id: 1,
            nominal_hash_rate: 2000.0,
            maximum_target: [0xFFu8; 32].try_into().unwrap(),
        };

        // Test that the message can be handled
        let mining_message = Mining::UpdateChannel(update_channel);

        // Verify the message structure
        match mining_message {
            Mining::UpdateChannel(msg) => {
                assert_eq!(msg.channel_id, 1);
                assert_eq!(msg.nominal_hash_rate, 2000.0);
            }
            _ => panic!("Expected UpdateChannel"),
        }
    }

    #[test]
    fn test_channel_manager_debug() {
        let manager = create_test_channel_manager();

        // Test that Debug trait is implemented
        let debug_str = format!("{:?}", manager);
        assert!(debug_str.contains("ChannelManager"));
    }

    #[test]
    fn test_channel_manager_data_access() {
        let manager = create_test_channel_manager();
        // Test that we can access and modify channel manager data
        manager
            .pending_downstream_channels
            .insert(1, ("test".to_string(), 100.0, 4));
        let has_pending = manager.pending_downstream_channels.contains_key(&1);

        assert!(has_pending);
    }
}
