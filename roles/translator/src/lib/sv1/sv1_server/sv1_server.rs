use crate::{
    config::TranslatorConfig,
    error::{self, TproxyError, TproxyErrorKind, TproxyResult},
    is_aggregated, is_non_aggregated,
    status::{handle_error, Status, StatusSender},
    sv1::{
        downstream::downstream::Downstream,
        sv1_server::{channel::Sv1ServerChannelState, KEEPALIVE_JOB_ID_DELIMITER},
    },
    utils::AGGREGATED_CHANNEL_ID,
};
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use stratum_apps::{
    custom_mutex::Mutex,
    fallback_coordinator::FallbackCoordinator,
    network_helpers::sv1_connection::ConnectionSV1,
    stratum_core::{
        binary_sv2::Str0255,
        bitcoin::Target,
        channels_sv2::{
            target::{hash_rate_from_target, hash_rate_to_target},
            Vardiff, VardiffState,
        },
        extensions_sv2::UserIdentity,
        mining_sv2::{CloseChannel, SetNewPrevHash, SetTarget},
        parsers_sv2::{Mining, Tlv, TlvField},
        stratum_translation::{
            sv1_to_sv2::{
                build_sv2_open_extended_mining_channel,
                build_sv2_submit_shares_extended_from_sv1_submit,
            },
            sv2_to_sv1::{build_sv1_notify_from_sv2, build_sv1_set_difficulty_from_sv2_target},
        },
        sv1_api::{json_rpc, server_to_client, utils::HexU32Be, IsServer},
    },
    task_manager::TaskManager,
    utils::types::{ChannelId, DownstreamId, Hashrate, RequestId, SharesPerMinute},
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

/// SV1 server that handles connections from SV1 miners.
///
/// This struct manages the SV1 server component of the translator, which:
/// - Accepts connections from SV1 miners
/// - Manages difficulty adjustment for connected miners
/// - Coordinates with the SV2 channel manager for upstream communication
/// - Tracks mining jobs and share submissions
///
/// The server maintains state for multiple downstream connections and implements
/// variable difficulty adjustment based on share submission rates.
#[derive(Clone)]
pub struct Sv1Server {
    pub(crate) sv1_server_channel_state: Sv1ServerChannelState,
    pub(crate) shares_per_minute: SharesPerMinute,
    pub(crate) listener_addr: SocketAddr,
    pub(crate) config: TranslatorConfig,
    pub(crate) sequence_counter: Arc<AtomicU32>,
    pub(crate) miner_counter: Arc<AtomicU32>,
    pub(crate) keepalive_job_id_counter: Arc<AtomicU32>,
    pub(crate) downstream_id_factory: Arc<AtomicUsize>,
    pub(crate) request_id_factory: Arc<AtomicU32>,
    pub(crate) downstreams: Arc<DashMap<DownstreamId, Downstream>>,
    pub(crate) request_id_to_downstream_id: Arc<DashMap<RequestId, DownstreamId>>,
    pub(crate) vardiff: Arc<DashMap<DownstreamId, Arc<Mutex<VardiffState>>>>,
    /// HashMap to store the SetNewPrevHash for each channel
    /// Used in both aggregated and non-aggregated mode
    pub(crate) prevhashes: Arc<DashMap<ChannelId, SetNewPrevHash<'static>>>,
    /// Tracks pending target updates that are waiting for SetTarget response from upstream
    pub(crate) pending_target_updates: Arc<Mutex<Vec<PendingTargetUpdate>>>,
    /// Valid Sv1 jobs storage, containing only a single shared entry (AGGREGATED_CHANNEL_ID) in
    /// case of channels aggregation (aggregated mode)
    pub(crate) valid_sv1_jobs: Arc<DashMap<ChannelId, Vec<server_to_client::Notify<'static>>>>,
}

#[cfg_attr(not(test), hotpath::measure_all)]
impl Sv1Server {
    /// Cleans up server state and closes communication channels.
    pub fn cleanup(&self) {
        self.prevhashes.clear();
        self.valid_sv1_jobs.clear();
        if self.config.downstream_difficulty_config.enable_vardiff {
            self.vardiff.clear();
        }
        self.downstreams.clear();
        self.request_id_to_downstream_id.clear();
        self.pending_target_updates
            .safe_lock(|updates| updates.clear())
            .ok();
        self.sv1_server_channel_state.drop();
    }

    /// Creates a new SV1 server instance.
    ///
    /// # Arguments
    /// * `listener_addr` - The socket address to bind the server to
    /// * `channel_manager_receiver` - Channel to receive messages from the channel manager
    /// * `channel_manager_sender` - Channel to send messages to the channel manager
    /// * `config` - Configuration settings for the translator
    ///
    /// # Returns
    /// A new Sv1Server instance ready to accept connections
    pub fn new(
        listener_addr: SocketAddr,
        channel_manager_receiver: Receiver<(Mining<'static>, Option<Vec<Tlv>>)>,
        channel_manager_sender: Sender<(Mining<'static>, Option<Vec<Tlv>>)>,
        config: TranslatorConfig,
    ) -> Self {
        let shares_per_minute = config.downstream_difficulty_config.shares_per_minute;
        let sv1_server_channel_state =
            Sv1ServerChannelState::new(channel_manager_receiver, channel_manager_sender);
        Self {
            sv1_server_channel_state,
            config,
            listener_addr,
            shares_per_minute,
            miner_counter: Arc::new(AtomicU32::new(0)),
            sequence_counter: Arc::new(AtomicU32::new(1)),
            keepalive_job_id_counter: Arc::new(AtomicU32::new(0)),
            downstream_id_factory: Arc::new(AtomicUsize::new(1)),
            request_id_factory: Arc::new(AtomicU32::new(1)),
            downstreams: Arc::new(DashMap::new()),
            request_id_to_downstream_id: Arc::new(DashMap::new()),
            vardiff: Arc::new(DashMap::new()),
            prevhashes: Arc::new(DashMap::new()),
            pending_target_updates: Arc::new(Mutex::new(Vec::new())),
            valid_sv1_jobs: Arc::new(DashMap::new()),
        }
    }

    /// Starts the SV1 server and begins accepting connections.
    ///
    /// This method:
    /// - Binds to the configured listening address
    /// - Spawns the variable difficulty adjustment loop
    /// - Enters the main event loop to handle:
    ///   - New miner connections
    ///   - Shutdown signals
    ///   - Messages from downstream miners (submit shares)
    ///   - Messages from upstream SV2 channel manager
    ///
    /// The server will continue running until a shutdown signal is received.
    ///
    /// # Arguments
    /// * `cancellation_token` - Global application cancellation token
    /// * `fallback_coordinator` - Fallback coordinator
    /// * `status_sender` - Channel for sending status updates
    /// * `task_manager` - Manager for spawned async tasks
    ///
    /// # Returns
    /// * `Ok(())` - Server shut down gracefully
    /// * `Err(TproxyError)` - Server encountered an error
    pub async fn start(
        self: Arc<Self>,
        cancellation_token: CancellationToken,
        fallback_coordinator: FallbackCoordinator,
        status_sender: Sender<Status>,
        task_manager: Arc<TaskManager>,
    ) -> TproxyResult<(), error::Sv1Server> {
        info!("Starting SV1 server on {}", self.listener_addr);

        // get the first target for the first set difficulty message
        let first_target: Target = hash_rate_to_target(
            self.config
                .downstream_difficulty_config
                .min_individual_miner_hashrate as f64,
            self.config.downstream_difficulty_config.shares_per_minute as f64,
        )
        .unwrap();

        let vardiff_future = self.clone().spawn_vardiff_loop();

        let keepalive_future = self.clone().spawn_job_keepalive_loop();

        let listener = TcpListener::bind(self.listener_addr).await.map_err(|e| {
            error!("Failed to bind to {}: {}", self.listener_addr, e);
            TproxyError::shutdown(e)
        })?;

        info!("Translator Proxy: listening on {}", self.listener_addr);

        let sv1_status_sender = StatusSender::Sv1Server(status_sender.clone());
        let task_manager_clone = task_manager.clone();
        let vardiff_enabled = self.config.downstream_difficulty_config.enable_vardiff;
        let keepalive_enabled = self
            .config
            .downstream_difficulty_config
            .job_keepalive_interval_secs
            > 0;
        task_manager_clone.spawn(async move {
            // we just spawned a new task that's relevant to fallback coordination
            // so register it with the fallback coordinator
            let fallback_handler = fallback_coordinator.register();

            // get the cancellation token that signals fallback
            let fallback_token = fallback_coordinator.token();

            tokio::pin!(vardiff_future);
            tokio::pin!(keepalive_future);
            loop {
                tokio::select! {
                    // Handle app shutdown signal
                    _ = cancellation_token.cancelled() => {
                        debug!("SV1 Server: received shutdown signal. Exiting.");
                        self.cleanup();
                        break;
                    }

                    // Handle fallback trigger
                    _ = fallback_token.cancelled() => {
                        info!("SV1 Server: fallback triggered, clearing state");
                        self.cleanup();
                        break;
                    }
                    result = listener.accept() => {
                        match result {
                            Ok((stream, addr)) => {
                                info!("New SV1 downstream connection from {}", addr);
                                let connection_token = cancellation_token.child_token();
                                let connection = ConnectionSV1::new(
                                    stream,
                                    connection_token.clone(),
                                ).await;
                                let downstream_id = self.downstream_id_factory.fetch_add(1, Ordering::Relaxed);
                                let downstream = Downstream::new(
                                    downstream_id,
                                    connection.sender().clone(),
                                    connection.receiver().clone(),
                                    self.sv1_server_channel_state.downstream_to_sv1_server_sender.clone(),
                                    self.sv1_server_channel_state.sv1_server_to_downstream_sender.clone(),
                                    first_target,
                                    Some(self.config.downstream_difficulty_config.min_individual_miner_hashrate),
                                    connection_token,
                                );
                                // vardiff initialization (only if enabled)
                                self.downstreams.insert(downstream_id, downstream.clone());
                                // Insert vardiff state for this downstream only if vardiff is enabled
                                if self.config.downstream_difficulty_config.enable_vardiff {
                                    let vardiff = VardiffState::new().expect("Failed to create vardiffstate");
                                    self.vardiff.insert(downstream_id, Arc::new(Mutex::new(vardiff)));
                                }
                                info!("Downstream {} registered successfully (channel will be opened after first message)", downstream_id);


                                // Start downstream tasks immediately, but defer channel opening until first message
                                let status_sender = StatusSender::Downstream {
                                    downstream_id,
                                    tx: status_sender.clone(),
                                };
                                Downstream::run_downstream_tasks(
                                    downstream,
                                    cancellation_token.clone(),
                                    fallback_coordinator.clone(),
                                    status_sender,
                                    task_manager.clone(),
                                );
                            }
                            Err(e) => {
                                warn!("Failed to accept new connection: {:?}", e);
                            }
                        }
                    }
                    res = self.handle_downstream_message() => {
                        if let Err(e) = res {
                            if handle_error(&sv1_status_sender, e).await {
                                self.cleanup();
                                break;
                            }
                        }
                    }
                    res = self.handle_upstream_message(
                        first_target,
                    ) => {
                        if let Err(e) = res {
                            if handle_error(&sv1_status_sender, e).await {
                                self.cleanup();
                                break;
                            }
                        }
                    }
                    _ = &mut vardiff_future, if vardiff_enabled => {}
                    _ = &mut keepalive_future, if keepalive_enabled => {}
                }
            }
            debug!("SV1 Server main listener loop exited.");

            // signal fallback coordinator that this task has completed its cleanup
            fallback_handler.done();
        });

        Ok(())
    }

    /// Handles messages received from downstream SV1 miners.
    ///
    /// This method processes share submissions from miners by:
    /// - Updating variable difficulty counters
    /// - Extracting and validating share data
    /// - Converting SV1 share format to SV2 SubmitSharesExtended
    /// - Forwarding the share to the channel manager for upstream submission
    ///
    /// # Returns
    /// * `Ok(())` - Message processed successfully
    /// * `Err(TproxyError)` - Error processing the message
    pub async fn handle_downstream_message(&self) -> TproxyResult<(), error::Sv1Server> {
        let (downstream_id, downstream_message) = self
            .sv1_server_channel_state
            .downstream_to_sv1_server_receiver
            .recv()
            .await
            .map_err(TproxyError::shutdown)?;

        let downstream = self.downstreams.get(&downstream_id);

        if let Some(downstream) = downstream {
            let channel_id = downstream
                .downstream_data
                .super_safe_lock(|data| data.channel_id);
            if channel_id.is_none() {
                let is_first_message = downstream
                    .downstream_data
                    .super_safe_lock(|d| d.queued_sv1_handshake_messages.is_empty());
                if is_first_message {
                    self.handle_open_channel_request(downstream_id).await?;
                    debug!(
                        "Down: Sent OpenChannel request for downstream {}",
                        downstream_id
                    );
                }
                debug!("Down: Queuing Sv1 message until channel is established");
                downstream.downstream_data.super_safe_lock(|data| {
                    data.queued_sv1_handshake_messages
                        .push(downstream_message.clone())
                });
                return Ok(());
            }

            let response = self
                .clone()
                .handle_message(Some(downstream_id), downstream_message.clone());

            match response {
                Ok(Some(response_msg)) => {
                    debug!(
                        "Down: Sending Sv1 message to downstream: {:?}",
                        response_msg
                    );
                    downstream
                        .downstream_channel_state
                        .downstream_sv1_sender
                        .send(response_msg.into())
                        .await
                        .map_err(|error| {
                            error!("Down: Failed to send message to downstream: {error:?}");
                            TproxyError::disconnect(
                                TproxyErrorKind::ChannelErrorSender,
                                downstream_id,
                            )
                        })?;

                    // Check if this was an authorize message and handle sv1 handshake completion
                    if let json_rpc::Message::StandardRequest(request) = &downstream_message {
                        if request.method == "mining.authorize" {
                            info!("Down: Handling mining.authorize after handshake completion");
                            if let Err(e) = downstream.handle_sv1_handshake_completion().await {
                                error!("Down: Failed to handle handshake completion: {:?}", e);
                                return Err(TproxyError::disconnect(e, downstream_id));
                            }
                        }
                    }
                }
                Ok(None) => {
                    // Message was handled but no response needed
                }
                Err(e) => {
                    error!("Down: Error handling downstream message: {:?}", e);
                    return Err(TproxyError::disconnect(e, downstream_id));
                }
            }

            // Check if there's a pending share to send to the Sv1Server
            let pending_share = downstream
                .downstream_data
                .super_safe_lock(|d| d.pending_share.take());
            if let Some(share) = pending_share {
                self.handle_submit_shares(share).await?;
            }
        }

        Ok(())
    }

    /// Handles share submission messages from downstream.
    async fn handle_submit_shares(
        &self,
        message: crate::sv1::downstream::SubmitShareWithChannelId,
    ) -> TproxyResult<(), error::Sv1Server> {
        // Increment vardiff counter for this downstream (only if vardiff is enabled)
        if self.config.downstream_difficulty_config.enable_vardiff {
            if let Some(vardiff_state) = self.vardiff.get(&message.downstream_id) {
                vardiff_state.super_safe_lock(|state| state.increment_shares_since_last_update());
            }
        }

        let job_version = match message.job_version {
            Some(version) => version,
            None => {
                warn!("Received share submission without valid job version, skipping");
                return Ok(());
            }
        };

        // If this is a keepalive job, extract the original upstream job_id from the job_id string
        let mut share = message.share;
        let job_id_str = share.job_id.clone();
        if Self::is_keepalive_job_id(&job_id_str) {
            if let Some(original_job_id) = Self::extract_original_job_id(&job_id_str) {
                debug!(
                    "Extracting original job_id {} from keepalive job_id {}",
                    original_job_id, job_id_str
                );
                share.job_id = original_job_id;
            } else {
                warn!(
                    "Failed to extract original job_id from keepalive job_id {}, rejecting share",
                    job_id_str
                );
                return Ok(());
            }
        }

        // Increment and return the value for this share
        let sequence_number = self.sequence_counter.fetch_add(1, Ordering::SeqCst);

        let submit_share_extended = build_sv2_submit_shares_extended_from_sv1_submit(
            &share,
            message.channel_id,
            sequence_number,
            job_version,
            message.version_rolling_mask,
        )
        .map_err(|_| TproxyError::shutdown(TproxyErrorKind::SV1Error))?;

        // Only add TLV fields with user identity in non-aggregated mode
        let tlv_fields = if is_non_aggregated() {
            let user_identity_string = self
                .downstreams
                .get(&message.downstream_id)
                .unwrap()
                .downstream_data
                .super_safe_lock(|d| d.user_identity.clone());
            UserIdentity::new(&user_identity_string)
                .unwrap()
                .to_tlv()
                .ok()
                .map(|tlv| vec![tlv])
        } else {
            None
        };

        self.sv1_server_channel_state
            .channel_manager_sender
            .send((
                Mining::SubmitSharesExtended(submit_share_extended),
                tlv_fields,
            ))
            .await
            .map_err(|_| TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender))?;

        Ok(())
    }

    /// Handles channel opening requests from downstream when they send their first message.
    async fn handle_open_channel_request(
        &self,
        downstream_id: DownstreamId,
    ) -> TproxyResult<(), error::Sv1Server> {
        info!(
            "SV1 server: opening extended mining channel for downstream {} after first message",
            downstream_id
        );

        let request_id = self.request_id_factory.fetch_add(1, Ordering::Relaxed);
        self.request_id_to_downstream_id
            .insert(request_id, downstream_id);

        if !self.downstreams.contains_key(&downstream_id) {
            error!(
                "Downstream {} not found when attempting to open channel",
                downstream_id
            );
            return Err(TproxyError::disconnect(
                TproxyErrorKind::DownstreamNotFound(downstream_id as u32),
                downstream_id,
            ));
        }

        self.open_extended_mining_channel(request_id, downstream_id)
            .await?;

        Ok(())
    }

    /// Handles messages received from the upstream SV2 server via the channel manager.
    ///
    /// This method processes various SV2 messages including:
    /// - OpenExtendedMiningChannelSuccess: Sets up downstream connections
    /// - NewExtendedMiningJob: Converts to SV1 notify messages
    /// - SetNewPrevHash: Updates block template information
    /// - Channel error messages (TODO: implement proper handling)
    ///
    /// # Arguments
    /// * `first_target` - Initial difficulty target for new connections
    ///
    /// # Returns
    /// * `Ok(())` - Message processed successfully
    /// * `Err(TproxyError)` - Error processing the message
    pub async fn handle_upstream_message(
        &self,
        first_target: Target,
    ) -> TproxyResult<(), error::Sv1Server> {
        let (message, _tlv_fields) = self
            .sv1_server_channel_state
            .channel_manager_receiver
            .recv()
            .await
            .map_err(TproxyError::shutdown)?;

        match message {
            Mining::OpenExtendedMiningChannelSuccess(m) => {
                debug!(
                    "Received OpenExtendedMiningChannelSuccess for channel id: {}",
                    m.channel_id
                );
                let downstream_id = self.request_id_to_downstream_id.remove(&m.request_id);

                let Some((_, downstream_id)) = downstream_id else {
                    return Err(TproxyError::log(TproxyErrorKind::DownstreamNotFound(
                        m.request_id,
                    )));
                };
                if let Some(downstream) = self.downstreams.get(&downstream_id) {
                    let initial_target =
                        Target::from_le_bytes(m.target.inner_as_ref().try_into().unwrap());
                    let extranonce1 = m
                        .extranonce_prefix
                        .to_vec()
                        .try_into()
                        .map_err(TproxyError::fallback)?;
                    downstream
                        .downstream_data
                        .safe_lock(|d| {
                            d.extranonce1 = extranonce1;
                            d.extranonce2_len = m.extranonce_size.into();
                            d.channel_id = Some(m.channel_id);
                            // Set the initial upstream target from OpenExtendedMiningChannelSuccess
                            d.set_upstream_target(initial_target, downstream_id);
                        })
                        .map_err(TproxyError::shutdown)?;

                    // Process all queued messages now that channel is established
                    if let Ok(queued_messages) = downstream.downstream_data.safe_lock(|d| {
                        let messages = d.queued_sv1_handshake_messages.clone();
                        d.queued_sv1_handshake_messages.clear();
                        messages
                    }) {
                        if !queued_messages.is_empty() {
                            info!(
                                "Processing {} queued Sv1 messages for downstream {}",
                                queued_messages.len(),
                                downstream_id
                            );

                            // Set flag to indicate we're processing queued responses
                            downstream
                                .processing_queued_sv1_handshake_responses
                                .store(true, Ordering::SeqCst);

                            for message in queued_messages {
                                if let Ok(Some(response_msg)) =
                                    self.clone().handle_message(Some(downstream_id), message)
                                {
                                    self.sv1_server_channel_state
                                        .sv1_server_to_downstream_sender
                                        .send((
                                            m.channel_id,
                                            Some(downstream_id),
                                            response_msg.into(),
                                        ))
                                        .map_err(|_| {
                                            TproxyError::shutdown(
                                                TproxyErrorKind::ChannelErrorSender,
                                            )
                                        })?;
                                }
                            }
                        }
                    }

                    let set_difficulty = build_sv1_set_difficulty_from_sv2_target(first_target)
                        .map_err(|_| {
                            TproxyError::shutdown(TproxyErrorKind::General(
                                "Failed to generate set_difficulty".into(),
                            ))
                        })?;
                    // send the set_difficulty message to the downstream
                    self.sv1_server_channel_state
                        .sv1_server_to_downstream_sender
                        .send((m.channel_id, None, set_difficulty))
                        .map_err(|_| TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender))?;
                } else {
                    error!("Downstream not found for downstream_id: {}", downstream_id);
                }
            }

            Mining::NewExtendedMiningJob(m) => {
                debug!(
                    "Received NewExtendedMiningJob for channel id: {}",
                    m.channel_id
                );
                if let Some(prevhash) = self.prevhashes.get(&m.channel_id) {
                    let prevhash = prevhash.as_static();
                    let clean_jobs = m.job_id == prevhash.job_id;
                    let notify =
                        build_sv1_notify_from_sv2(prevhash, m.clone().into_static(), clean_jobs)
                            .map_err(TproxyError::shutdown)?;

                    // Update job storage based on the configured mode
                    let notify_parsed = notify.clone();
                    let job_channel_id = if is_non_aggregated() {
                        m.channel_id
                    } else {
                        AGGREGATED_CHANNEL_ID
                    };

                    let mut channel_jobs = self.valid_sv1_jobs.entry(job_channel_id).or_default();
                    if clean_jobs {
                        channel_jobs.clear();
                    }
                    channel_jobs.push(notify_parsed);

                    let _ = self
                        .sv1_server_channel_state
                        .sv1_server_to_downstream_sender
                        .send((m.channel_id, None, notify.into()));
                }
            }

            Mining::SetNewPrevHash(m) => {
                debug!("Received SetNewPrevHash for channel id: {}", m.channel_id);
                self.prevhashes
                    .insert(m.channel_id, m.clone().into_static());
            }

            Mining::SetTarget(m) => {
                debug!("Received SetTarget for channel id: {}", m.channel_id);
                if self.config.downstream_difficulty_config.enable_vardiff {
                    // Vardiff enabled - use full difficulty management
                    self.handle_set_target_message(m).await;
                } else {
                    // Vardiff disabled - just forward the difficulty to downstreams
                    debug!("Vardiff disabled - forwarding SetTarget to downstreams");
                    self.handle_set_target_without_vardiff(m).await?;
                }
            }
            // Guaranteed unreachable: the channel manager only forwards valid,
            // pre-filtered messages, so no other variants can arrive here.
            _ => unreachable!("Invalid message: should have been filtered earlier"),
        }

        Ok(())
    }

    /// Opens an extended mining channel for a downstream connection.
    ///
    /// This method initiates the SV2 channel setup process by:
    /// - Calculating the initial target based on configuration
    /// - Generating a unique user identity for the miner
    /// - Creating an OpenExtendedMiningChannel message
    /// - Sending the request to the channel manager
    ///
    /// # Arguments
    /// * `downstream` - The downstream connection to set up a channel for
    ///
    /// # Returns
    /// * `Ok(())` - Channel setup request sent successfully
    /// * `Err(TproxyError)` - Error setting up the channel
    pub async fn open_extended_mining_channel(
        &self,
        request_id: RequestId,
        downstream_id: DownstreamId,
    ) -> TproxyResult<(), error::Sv1Server> {
        let config = &self.config.downstream_difficulty_config;
        let downstream = self.downstreams.get(&downstream_id).unwrap();

        let hashrate = config.min_individual_miner_hashrate as f64;
        let shares_per_min = config.shares_per_minute as f64;
        let min_extranonce_size = self.config.downstream_extranonce2_size;
        let vardiff_enabled = config.enable_vardiff;

        let max_target = if vardiff_enabled {
            hash_rate_to_target(hashrate, shares_per_min).unwrap()
        } else {
            // If translator doesn't manage vardiff, we rely on upstream to do that,
            // so we give it more freedom by setting max_target to maximum possible value
            Target::from_le_bytes([0xff; 32])
        };

        let miner_id = self.miner_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let user_identity = format!("{}.miner{}", self.config.user_identity, miner_id);

        downstream
            .downstream_data
            .safe_lock(|d| d.user_identity = user_identity.clone())
            .map_err(TproxyError::shutdown)?;

        if let Ok(open_channel_msg) = build_sv2_open_extended_mining_channel(
            request_id,
            user_identity.clone(),
            hashrate as Hashrate,
            max_target,
            min_extranonce_size,
        ) {
            self.sv1_server_channel_state
                .channel_manager_sender
                .send((Mining::OpenExtendedMiningChannel(open_channel_msg), None))
                .await
                .map_err(|_| TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender))?;
        } else {
            error!("Failed to build OpenExtendedMiningChannel message");
        }

        Ok(())
    }

    /// Retrieves a downstream connection by ID from the provided map.
    ///
    /// # Arguments
    /// * `downstream_id` - The ID of the downstream connection to find
    /// * `downstream` - HashMap containing downstream connections
    ///
    /// # Returns
    /// * `Some(Downstream)` - If a downstream with the given ID exists
    /// * `None` - If no downstream with the given ID is found
    pub fn get_downstream(
        downstream_id: DownstreamId,
        downstream: HashMap<DownstreamId, Downstream>,
    ) -> Option<Downstream> {
        downstream.get(&downstream_id).cloned()
    }

    /// Extracts the downstream ID from a Downstream instance.
    ///
    /// # Arguments
    /// * `downstream` - The downstream connection to get the ID from
    ///
    /// # Returns
    /// The downstream ID as a u32
    pub fn get_downstream_id(downstream: Downstream) -> DownstreamId {
        downstream.downstream_id
    }

    /// Handles cleanup when a downstream connection disconnects.
    ///
    /// This method should be called from the main loop when a `State::DownstreamShutdown`
    /// status message is received. It:
    /// - Removes the downstream from the downstreams map
    /// - Removes vardiff state (if enabled)
    /// - Sends UpdateChannel if needed (aggregated mode with vardiff)
    /// - Sends CloseChannel message to ChannelManager (non-aggregated mode)
    ///
    /// # Arguments
    /// * `downstream_id` - The ID of the downstream that disconnected
    pub async fn handle_downstream_disconnect(&self, downstream_id: DownstreamId) {
        if self.config.downstream_difficulty_config.enable_vardiff {
            // Only remove from vardiff map if vardiff is enabled
            self.vardiff.remove(&downstream_id);
        }
        let current_downstream = self.downstreams.remove(&downstream_id);

        if let Some((downstream_id, downstream)) = current_downstream {
            info!("🔌 Downstream: {downstream_id} disconnected and removed from sv1 server downstreams");
            // In aggregated mode, send UpdateChannel to reflect the new state (only if vardiff
            // enabled)
            if self.config.downstream_difficulty_config.enable_vardiff {
                self.send_update_channel_on_downstream_state_change().await;
            }

            let channel_id = downstream.downstream_data.super_safe_lock(|d| d.channel_id);
            if let Some(channel_id) = channel_id {
                if !self.config.aggregate_channels {
                    info!("Sending CloseChannel message: {channel_id} for downstream: {downstream_id}");
                    let reason_code =
                        Str0255::try_from("downstream disconnected".to_string()).unwrap();
                    _ = self
                        .sv1_server_channel_state
                        .channel_manager_sender
                        .send((
                            Mining::CloseChannel(CloseChannel {
                                channel_id,
                                reason_code,
                            }),
                            None,
                        ))
                        .await;
                }
            }
        }
    }

    /// Handles SetTarget messages when vardiff is disabled.
    ///
    /// This method forwards difficulty changes from upstream directly to downstream miners
    /// without any variable difficulty logic. It respects the aggregated/non-aggregated
    /// channel configuration.
    ///
    /// When vardiff is disabled, the upstream (Pool or JDC) controls difficulty via SetTarget
    /// messages. We derive the hashrate from the received target so that monitoring can report
    /// meaningful SV1 downstream hashrate values.
    async fn handle_set_target_without_vardiff(
        &self,
        set_target: SetTarget<'_>,
    ) -> TproxyResult<(), error::Sv1Server> {
        let new_target =
            Target::from_le_bytes(set_target.maximum_target.inner_as_ref().try_into().unwrap());
        debug!(
            "Forwarding SetTarget to downstreams: channel_id={}, target={:?}",
            set_target.channel_id, new_target
        );

        // Derive hashrate from the upstream target so monitoring can report it
        let derived_hashrate = match hash_rate_from_target(
            set_target.maximum_target.clone().into_static(),
            self.shares_per_minute as f64,
        ) {
            Ok(hr) => {
                debug!(
                    "Derived hashrate from SetTarget: {} H/s (channel_id={})",
                    hr, set_target.channel_id
                );
                Some(hr)
            }
            Err(e) => {
                warn!(
                    "Failed to derive hashrate from SetTarget target: {:?} (channel_id={})",
                    e, set_target.channel_id
                );
                None
            }
        };

        if is_aggregated() {
            // Aggregated mode: send set_difficulty to ALL downstreams and update hashrate
            return self
                .send_set_difficulty_to_all_downstreams(new_target, derived_hashrate)
                .await;
        }

        // Non-aggregated mode: send set_difficulty to specific downstream for this channel
        self.send_set_difficulty_to_specific_downstream(
            set_target.channel_id,
            new_target,
            derived_hashrate,
        )
        .await
    }

    /// Sends set_difficulty to all downstreams (aggregated mode).
    /// Used only when vardiff is disabled.
    async fn send_set_difficulty_to_all_downstreams(
        &self,
        target: Target,
        derived_hashrate: Option<f64>,
    ) -> TproxyResult<(), error::Sv1Server> {
        for downstream in self.downstreams.iter() {
            let downstream_id = downstream.key();
            let downstream = downstream.value();
            let channel_id = downstream.downstream_data.super_safe_lock(|d| {
                let channel_id = d.channel_id?;

                d.set_upstream_target(target, *downstream_id);
                d.set_pending_target(target, *downstream_id);

                // Update pending hashrate derived from the upstream target
                if let Some(hr) = derived_hashrate {
                    d.set_pending_hashrate(Some(hr as f32), *downstream_id);
                }

                Some(channel_id)
            });

            let Some(channel_id) = channel_id else {
                trace!(
                    "Skipping downstream {}: no channel_id set (vardiff disabled)",
                    downstream_id
                );
                continue;
            };

            let set_difficulty_msg = match build_sv1_set_difficulty_from_sv2_target(target) {
                Ok(msg) => msg,
                Err(e) => {
                    error!(
                        "Failed to build SetDifficulty for downstream {}: {:?}",
                        downstream_id, e
                    );
                    return Err(TproxyError::shutdown(e));
                }
            };

            if let Err(e) = self
                .sv1_server_channel_state
                .sv1_server_to_downstream_sender
                .send((channel_id, Some(*downstream_id), set_difficulty_msg))
            {
                error!(
                    "Failed to send SetDifficulty to downstream {}: {:?}",
                    downstream_id, e
                );
                return Err(TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender));
            } else {
                debug!(
                    "Sent SetDifficulty to downstream {} (vardiff disabled)",
                    downstream_id
                );
            }
        }
        Ok(())
    }

    /// Sends set_difficulty to the specific downstream associated with a channel (non-aggregated
    /// mode).
    /// Used only when vardiff is disabled.
    async fn send_set_difficulty_to_specific_downstream(
        &self,
        channel_id: ChannelId,
        target: Target,
        derived_hashrate: Option<f64>,
    ) -> TproxyResult<(), error::Sv1Server> {
        let affected = self.downstreams.iter().find(|downstream| {
            downstream
                .downstream_data
                .super_safe_lock(|d| d.channel_id == Some(channel_id))
        });

        let Some(downstream) = affected else {
            warn!(
                "No downstream found for channel {} when vardiff is disabled",
                channel_id
            );
            info!("Sending CloseChannel message: Channel id {channel_id}");
            let reason_code = Str0255::try_from("downstream disconnected".to_string()).unwrap();
            self.sv1_server_channel_state
                .channel_manager_sender
                .send((
                    Mining::CloseChannel(CloseChannel {
                        channel_id,
                        reason_code,
                    }),
                    None,
                ))
                .await
                .map_err(|_| TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender))?;
            return Err(TproxyError::log(
                TproxyErrorKind::DownstreamNotFoundWithChannelId(channel_id),
            ));
        };

        let downstream_id = downstream.key();
        let downstream = downstream.value();

        downstream.downstream_data.super_safe_lock(|d| {
            d.set_upstream_target(target, *downstream_id);
            d.set_pending_target(target, *downstream_id);

            // Update pending hashrate derived from the upstream target
            if let Some(hr) = derived_hashrate {
                d.set_pending_hashrate(Some(hr as f32), *downstream_id);
            }
        });

        let set_difficulty_msg = match build_sv1_set_difficulty_from_sv2_target(target) {
            Ok(msg) => msg,
            Err(e) => {
                error!(
                    "Failed to build SetDifficulty for downstream {}: {:?}",
                    downstream_id, e
                );
                return Err(TproxyError::shutdown(e));
            }
        };

        if let Err(e) = self
            .sv1_server_channel_state
            .sv1_server_to_downstream_sender
            .send((channel_id, Some(*downstream_id), set_difficulty_msg))
        {
            error!(
                "Failed to send SetDifficulty to downstream {}: {:?}",
                downstream_id, e
            );
            return Err(TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender));
        } else {
            debug!(
                "Sent SetDifficulty to downstream {} for channel {} (vardiff disabled)",
                downstream_id, channel_id
            );
        }
        Ok(())
    }

    /// Spawns the job keepalive loop that sends periodic mining.notify messages.
    ///
    /// This prevents SV1 miners from timing out when there are no new jobs received from the
    /// upstream for a while.
    pub async fn spawn_job_keepalive_loop(self: Arc<Self>) {
        let keepalive_interval_secs = self
            .config
            .downstream_difficulty_config
            .job_keepalive_interval_secs;

        let interval = Duration::from_secs(keepalive_interval_secs as u64);
        let check_interval =
            Duration::from_secs(keepalive_interval_secs as u64 / 2).max(Duration::from_secs(5));
        info!(
            "Starting job keepalive loop with interval of {} seconds",
            keepalive_interval_secs
        );

        loop {
            tokio::time::sleep(check_interval).await;
            let keepalive_targets: Vec<(DownstreamId, Option<ChannelId>)> = self
                .downstreams
                .iter()
                .filter_map(|downstream| {
                    let downstream_id = downstream.key();
                    let downstream = downstream.value();
                    downstream.downstream_data.super_safe_lock(|d| {
                        // Only send keepalive if:
                        // 1. Handshake is complete
                        // 2. Enough time has passed since last job
                        let handshake_complete =
                            downstream.sv1_handshake_complete.load(Ordering::SeqCst);

                        if !handshake_complete {
                            return None;
                        }

                        let needs_keepalive = match d.last_job_received_time {
                            Some(last_time) => last_time.elapsed() >= interval,
                            None => false, // No job received yet, don't send keepalive
                        };

                        if needs_keepalive {
                            Some((*downstream_id, d.channel_id))
                        } else {
                            None
                        }
                    })
                })
                .collect();

            // Send keepalive to each downstream that needs one
            for (downstream_id, channel_id) in keepalive_targets {
                // Get the appropriate job for this downstream's channel and create keepalive
                let keepalive_job = self.get_last_job(channel_id).and_then(|last_job| {
                    // Extract the original upstream job_id from the last job
                    // If it's already a keepalive job, extract its original; otherwise use
                    // as-is
                    let original_job_id = Self::extract_original_job_id(&last_job.job_id)
                        .unwrap_or_else(|| last_job.job_id.clone());

                    // Find the original upstream job to get its base time
                    let original_job = self.get_original_job(&original_job_id, channel_id);
                    let base_time = original_job
                        .as_ref()
                        .map(|j| j.time.0)
                        .unwrap_or(last_job.time.0);

                    // Increment the time by the keepalive interval, but cap at
                    // MAX_FUTURE_BLOCK_TIME from the original job's time to maintain consensus
                    // validity (see https://github.com/bitcoin/bitcoin/blob/cd6e4c9235f763b8077cece69c2e3b2025cc8d0f/src/chain.h#L29)
                    const MAX_FUTURE_BLOCK_TIME: u32 = 2 * 60 * 60;
                    let new_time = last_job
                        .time
                        .0
                        .saturating_add(keepalive_interval_secs as u32)
                        .min(base_time.saturating_add(MAX_FUTURE_BLOCK_TIME));

                    // If we've hit the cap, don't send another keepalive for this job
                    if new_time == last_job.time.0 {
                        return None;
                    }

                    // Generate new keepalive job_id: {original_job_id}#{counter}
                    let new_job_id = self.next_keepalive_job_id(&original_job_id);

                    let mut keepalive_notify = last_job;
                    keepalive_notify.job_id = new_job_id.clone();
                    keepalive_notify.time = HexU32Be(new_time);

                    // Add the keepalive job to valid jobs so shares can be validated
                    let job_channel_id = if is_aggregated() {
                        Some(AGGREGATED_CHANNEL_ID)
                    } else {
                        channel_id
                    };

                    _ = job_channel_id
                        .and_then(|ch_id| self.valid_sv1_jobs.get_mut(&ch_id))
                        .map(|mut jobs| jobs.push(keepalive_notify.clone()));

                    Some(keepalive_notify)
                });

                if let Some(notify) = keepalive_job {
                    debug!(
                        "Sending keepalive job to downstream {} with job_id: {}, time: {}",
                        downstream_id, notify.job_id, notify.time.0
                    );

                    if let Err(e) = self
                        .sv1_server_channel_state
                        .sv1_server_to_downstream_sender
                        .send((channel_id.unwrap_or(0), Some(downstream_id), notify.into()))
                    {
                        warn!(
                            "Failed to send keepalive job to downstream {}: {:?}",
                            downstream_id, e
                        );
                    } else if let Some(downstream) = self.downstreams.get(&downstream_id) {
                        downstream.downstream_data.super_safe_lock(|d| {
                            d.last_job_received_time = Some(Instant::now());
                        });
                    }
                }
            }
        }
    }

    /// Generates a keepalive job ID by appending a mutation counter to the original job ID.
    /// Format: `{original_job_id}#{counter}` where `#` is the delimiter.
    /// When receiving a share, split on `#` to extract the original job ID.
    fn next_keepalive_job_id(&self, original_job_id: &str) -> String {
        let counter = self
            .keepalive_job_id_counter
            .fetch_add(1, Ordering::Relaxed);
        format!("{}#{}", original_job_id, counter)
    }

    /// Extracts the original upstream job ID from a keepalive job ID.
    /// Returns None if the job_id doesn't contain the keepalive delimiter.
    fn extract_original_job_id(job_id: &str) -> Option<String> {
        job_id
            .split_once(KEEPALIVE_JOB_ID_DELIMITER)
            .map(|(original, _)| original.to_string())
    }

    /// Returns true if the job_id is a keepalive job (contains the delimiter).
    #[inline]
    fn is_keepalive_job_id(job_id: &str) -> bool {
        job_id.contains(KEEPALIVE_JOB_ID_DELIMITER)
    }

    /// Gets the last job from the jobs storage.
    /// In aggregated mode, returns the last job from the shared job list.
    /// In non-aggregated mode, returns the last job for the specified channel.
    pub fn get_last_job(
        &self,
        channel_id: Option<u32>,
    ) -> Option<server_to_client::Notify<'static>> {
        let channel_id = if is_aggregated() {
            AGGREGATED_CHANNEL_ID
        } else {
            channel_id?
        };

        self.valid_sv1_jobs
            .get(&channel_id)
            .and_then(|jobs| jobs.last().cloned())
    }

    /// Gets the original upstream job by its job_id.
    /// This is used to find the base time for keepalive time capping.
    pub fn get_original_job(
        &self,
        job_id: &str,
        channel_id: Option<u32>,
    ) -> Option<server_to_client::Notify<'static>> {
        let channel_id = if is_aggregated() {
            AGGREGATED_CHANNEL_ID
        } else {
            channel_id?
        };

        self.valid_sv1_jobs
            .get(&channel_id)?
            .iter()
            .find(|j| j.job_id == job_id)
            .cloned()
    }
}

#[derive(Debug, Clone)]
pub struct PendingTargetUpdate {
    pub downstream_id: DownstreamId,
    pub new_target: Target,
    pub new_hashrate: Hashrate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DownstreamDifficultyConfig, TranslatorConfig, Upstream};
    use async_channel::unbounded;
    use std::{collections::HashMap, str::FromStr};
    use stratum_apps::key_utils::Secp256k1PublicKey;

    fn create_test_config() -> TranslatorConfig {
        let pubkey_str = "9bDuixKmZqAJnrmP746n8zU1wyAQRrus7th9dxnkPg6RzQvCnan";
        let pubkey = Secp256k1PublicKey::from_str(pubkey_str).unwrap();

        let upstream = Upstream::new("127.0.0.1".to_string(), 4444, pubkey);
        let difficulty_config = DownstreamDifficultyConfig::new(100.0, 5.0, true, 60);

        TranslatorConfig::new(
            vec![upstream],
            "0.0.0.0".to_string(), // downstream_address
            3333,                  // downstream_port
            difficulty_config,     // downstream_difficulty_config
            2,                     // max_supported_version
            1,                     // min_supported_version
            4,                     // downstream_extranonce2_size
            "test_user".to_string(),
            true,   // aggregate_channels
            vec![], // supported_extensions
            vec![], // required_extensions
        )
    }

    fn create_test_sv1_server() -> Sv1Server {
        let (cm_sender, _cm_receiver) = unbounded();
        let (_downstream_sender, cm_receiver) = unbounded();
        let config = create_test_config();
        let addr = "127.0.0.1:3333".parse().unwrap();

        Sv1Server::new(addr, cm_receiver, cm_sender, config)
    }

    #[test]
    fn test_sv1_server_creation() {
        let server = create_test_sv1_server();

        assert_eq!(server.shares_per_minute, 5.0);
        assert_eq!(server.listener_addr.ip().to_string(), "127.0.0.1");
        assert_eq!(server.listener_addr.port(), 3333);
        assert_eq!(server.config.user_identity, "test_user");
    }

    #[test]
    fn test_sv1_server_config() {
        let mut config = create_test_config();
        config.downstream_difficulty_config.enable_vardiff = true;

        let (cm_sender, _cm_receiver) = unbounded();
        let (_downstream_sender, cm_receiver) = unbounded();
        let addr = "127.0.0.1:3333".parse().unwrap();

        let server = Sv1Server::new(addr, cm_receiver, cm_sender, config);

        assert!(server.config.downstream_difficulty_config.enable_vardiff);
    }

    #[test]
    fn test_get_downstream_basic() {
        let downstreams = HashMap::new();

        // Test non-existing downstream
        let not_found = Sv1Server::get_downstream(999, downstreams);
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_send_set_difficulty_to_all_downstreams_empty() {
        let server = create_test_sv1_server();
        let target: Target = hash_rate_to_target(200.0, 5.0).unwrap();

        // Test with empty downstreams
        _ = server
            .send_set_difficulty_to_all_downstreams(target, None)
            .await;

        // Should not crash with empty downstreams
    }

    #[tokio::test]
    async fn test_send_set_difficulty_to_specific_downstream_not_found() {
        let server = create_test_sv1_server();
        let target: Target = hash_rate_to_target(200.0, 5.0).unwrap();
        let channel_id = 1u32;

        // Test with no downstreams
        _ = server
            .send_set_difficulty_to_specific_downstream(channel_id, target, None)
            .await;

        // Should not crash when no downstreams are found
    }

    #[tokio::test]
    async fn test_handle_set_target_without_vardiff_aggregated() {
        let mut config = create_test_config();
        config.downstream_difficulty_config.enable_vardiff = false;

        let (cm_sender, _cm_receiver) = unbounded();
        let (_downstream_sender, cm_receiver) = unbounded();
        let addr = "127.0.0.1:3333".parse().unwrap();

        let server = Sv1Server::new(addr, cm_receiver, cm_sender, config);
        let target: Target = hash_rate_to_target(200.0, 5.0).unwrap();

        let set_target = SetTarget {
            channel_id: 1,
            maximum_target: target.to_le_bytes().into(),
        };

        // Test should not panic and should handle the message
        _ = server.handle_set_target_without_vardiff(set_target).await;
    }

    #[tokio::test]
    async fn test_handle_set_target_without_vardiff_non_aggregated() {
        let mut config = create_test_config();
        config.downstream_difficulty_config.enable_vardiff = false;

        let (cm_sender, _cm_receiver) = unbounded();
        let (_downstream_sender, cm_receiver) = unbounded();
        let addr = "127.0.0.1:3333".parse().unwrap();

        let server = Sv1Server::new(addr, cm_receiver, cm_sender, config);
        let target: Target = hash_rate_to_target(200.0, 5.0).unwrap();

        let set_target = SetTarget {
            channel_id: 1,
            maximum_target: target.to_le_bytes().into(),
        };

        // Test should not panic and should handle the message
        _ = server.handle_set_target_without_vardiff(set_target).await;
    }

    #[test]
    fn test_sv1_server_counters() {
        let server = create_test_sv1_server();

        // Test initial values
        assert_eq!(server.miner_counter.load(Ordering::SeqCst), 0);
        assert_eq!(server.sequence_counter.load(Ordering::SeqCst), 1);

        // Test incrementing
        let miner_id = server.miner_counter.fetch_add(1, Ordering::SeqCst);
        assert_eq!(miner_id, 0);
        assert_eq!(server.miner_counter.load(Ordering::SeqCst), 1);

        // sequence_counter starts at 1, so first share gets sequence 1
        let seq_id = server.sequence_counter.fetch_add(1, Ordering::SeqCst);
        assert_eq!(seq_id, 1);
        assert_eq!(server.sequence_counter.load(Ordering::SeqCst), 2);
    }
}
