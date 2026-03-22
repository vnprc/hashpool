use crate::{
    error::{self, TproxyError, TproxyErrorKind, TproxyResult},
    status::{handle_error, StatusSender},
    sv1::downstream::{channel::DownstreamChannelState, data::DownstreamData},
    utils::AGGREGATED_CHANNEL_ID,
};
use async_channel::{Receiver, Sender};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
use stratum_apps::{
    custom_mutex::Mutex,
    fallback_coordinator::FallbackCoordinator,
    stratum_core::{
        bitcoin::Target,
        sv1_api::{
            json_rpc::{self, Message},
            server_to_client,
        },
    },
    task_manager::TaskManager,
    utils::types::{ChannelId, DownstreamId, Hashrate},
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Represents a downstream SV1 miner connection.
///
/// This struct manages the state and communication for a single SV1 miner connected
/// to the translator. It handles:
/// - SV1 protocol message processing (subscribe, authorize, submit)
/// - Bidirectional message routing between miner and SV1 server
/// - Mining job tracking and share validation
/// - Difficulty adjustment coordination
/// - Connection lifecycle management
///
/// Each downstream connection runs in its own async task that processes messages
/// from both the miner and the server, ensuring proper message ordering and
/// handling connection-specific state.
#[derive(Clone, Debug)]
pub struct Downstream {
    pub downstream_id: DownstreamId,
    pub downstream_data: Arc<Mutex<DownstreamData>>,
    pub downstream_channel_state: DownstreamChannelState,
    // Flag to track if SV1 handshake is complete (subscribe + authorize)
    pub sv1_handshake_complete: Arc<AtomicBool>,
    // Flag to indicate we're processing queued Sv1 handshake message responses
    pub processing_queued_sv1_handshake_responses: Arc<AtomicBool>,
}

#[cfg_attr(not(test), hotpath::measure_all)]
impl Downstream {
    /// Creates a new downstream connection instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        downstream_id: DownstreamId,
        downstream_sv1_sender: Sender<json_rpc::Message>,
        downstream_sv1_receiver: Receiver<json_rpc::Message>,
        sv1_server_sender: Sender<(DownstreamId, json_rpc::Message)>,
        sv1_server_broadcast: broadcast::Sender<(
            ChannelId,
            Option<DownstreamId>,
            json_rpc::Message,
        )>,
        target: Target,
        hashrate: Option<Hashrate>,
        connection_token: CancellationToken,
    ) -> Self {
        let downstream_data = Arc::new(Mutex::new(DownstreamData::new(hashrate, target)));
        let downstream_channel_state = DownstreamChannelState::new(
            downstream_sv1_sender,
            downstream_sv1_receiver,
            sv1_server_sender,
            sv1_server_broadcast,
            connection_token,
        );
        Self {
            downstream_id,
            downstream_data,
            downstream_channel_state,
            sv1_handshake_complete: Arc::new(AtomicBool::new(false)),
            processing_queued_sv1_handshake_responses: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawns and runs the main task loop for this downstream connection.
    ///
    /// This method creates an async task that handles all communication for this
    /// downstream connection. The task runs a select loop that processes:
    /// - Cancellation signals (global via cancellation_token or fallback)
    /// - Messages from the miner (subscribe, authorize, submit)
    /// - Messages from the SV1 server (notify, set_difficulty, etc.)
    ///
    /// The task will continue running until a cancellation signal is received or
    /// an unrecoverable error occurs. It ensures graceful cleanup of resources
    /// and proper error reporting.
    pub fn run_downstream_tasks(
        self,
        cancellation_token: CancellationToken,
        fallback_coordinator: FallbackCoordinator,
        status_sender: StatusSender,
        task_manager: Arc<TaskManager>,
    ) {
        let mut sv1_server_receiver = self
            .downstream_channel_state
            .sv1_server_broadcast
            .subscribe();
        let downstream_id = self.downstream_id;
        task_manager.spawn(async move {
            // we just spawned a new task that's relevant to fallback coordination
            // so register it with the fallback coordinator
            let fallback_handler = fallback_coordinator.register();

            // get the cancellation token that signals fallback
            let fallback_token = fallback_coordinator.token();

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        info!("Downstream {downstream_id}: received app shutdown signal");
                        break;
                    }
                    _ = fallback_token.cancelled() => {
                        info!("Downstream {downstream_id}: fallback triggered");
                        break;
                    }

                    // Handle downstream -> server message
                    res = self.handle_downstream_message() => {
                        if let Err(e) = res {
                            error!("Downstream {downstream_id}: error in downstream message handler: {e:?}");
                            if handle_error(&status_sender, e).await {
                                break;
                            }
                        }
                    }

                    // Handle server -> downstream message
                    res = self.handle_sv1_server_message(&mut sv1_server_receiver) => {
                        if let Err(e) = res {
                            error!("Downstream {downstream_id}: error in server message handler: {e:?}");
                            if handle_error(&status_sender, e).await {
                                break;
                            }
                        }
                    }

                    else => {
                        warn!("Downstream {downstream_id}: all channels closed; exiting task");
                        break;
                    }
                }
            }

            warn!("Downstream {downstream_id}: unified task shutting down");
            self.downstream_channel_state.drop();

            // signal fallback coordinator that this task has completed its cleanup
            fallback_handler.done();
        });
    }

    /// Handles messages received from the SV1 server.
    ///
    /// This method processes messages broadcast from the SV1 server to downstream
    /// connections. Since `mining.notify` messages are guaranteed to never arrive
    /// before their corresponding `mining.set_difficulty` message, the logic is
    /// simplified to handle only handshake completion timing.
    ///
    /// Key behaviors:
    /// - Filters messages by channel ID and downstream ID
    /// - For `mining.set_difficulty`: Always caches the message (never sent immediately)
    /// - For `mining.notify`: Sends any pending set_difficulty first, then forwards the notify
    /// - For other messages: Forwards directly to the miner
    /// - Caches both `mining.set_difficulty` and `mining.notify` messages if handshake is not yet
    ///   complete
    /// - On handshake completion: sends cached messages in correct order (set_difficulty first,
    ///   then notify)
    pub async fn handle_sv1_server_message(
        &self,
        sv1_server_receiver: &mut broadcast::Receiver<(
            ChannelId,
            Option<DownstreamId>,
            json_rpc::Message,
        )>,
    ) -> TproxyResult<(), error::Downstream> {
        match sv1_server_receiver.recv().await {
            Ok((channel_id, downstream_id, message)) => {
                let my_channel_id = self.downstream_data.super_safe_lock(|d| d.channel_id);
                let my_downstream_id = self.downstream_id;
                let handshake_complete = self.sv1_handshake_complete.load(Ordering::SeqCst);
                let id_matches = (my_channel_id == Some(channel_id)
                    || channel_id == AGGREGATED_CHANNEL_ID)
                    && (downstream_id.is_none() || downstream_id == Some(my_downstream_id));
                if !id_matches {
                    return Ok(()); // Message not intended for this downstream
                }

                // Check if this is a queued message response
                let is_queued_sv1_handshake_response = self
                    .processing_queued_sv1_handshake_responses
                    .load(Ordering::SeqCst);

                // Handle messages based on message type and handshake state
                if let Message::Notification(notification) = &message {
                    // For notifications (mining.set_difficulty, mining.notify), only send if
                    // handshake is complete
                    if handshake_complete {
                        match notification.method.as_str() {
                            "mining.set_difficulty" => {
                                // Cache the Sv1 set_difficulty message to be sent before the next
                                // notify
                                debug!("Down: Caching mining.set_difficulty to send before next mining.notify");
                                self.downstream_data.super_safe_lock(|d| {
                                    d.cached_set_difficulty = Some(message);
                                });
                                return Ok(());
                            }
                            "mining.notify" => {
                                let (pending_set_difficulty, notify_opt) =
                                    self.downstream_data.super_safe_lock(|d| {
                                        let cached_set_difficulty = d.cached_set_difficulty.take();

                                        // Prepare the notify message and update state
                                        let notify_result = server_to_client::Notify::try_from(
                                            notification.clone(),
                                        );
                                        if let Ok(mut notify) = notify_result {
                                            if cached_set_difficulty.is_some() {
                                                notify.clean_jobs = true;
                                            }
                                            d.last_job_version_field = Some(notify.version.0);

                                            // Update target and hashrate if we're sending
                                            // set_difficulty
                                            if cached_set_difficulty.is_some() {
                                                if let Some(new_target) = d.pending_target.take() {
                                                    d.target = new_target;
                                                }
                                                if let Some(new_hashrate) =
                                                    d.pending_hashrate.take()
                                                {
                                                    d.hashrate = Some(new_hashrate);
                                                }
                                            }
                                            // Update last job received time for keepalive tracking
                                            d.last_job_received_time = Some(Instant::now());
                                            (cached_set_difficulty, Some(notify))
                                        } else {
                                            (cached_set_difficulty, None)
                                        }
                                    });

                                if let Some(set_difficulty_msg) = &pending_set_difficulty {
                                    debug!("Down: Sending pending mining.set_difficulty before mining.notify");
                                    self.downstream_channel_state
                                        .downstream_sv1_sender
                                        .send(set_difficulty_msg.clone())
                                        .await
                                        .map_err(|e| {
                                            error!(
                                                "Down: Failed to send mining.set_difficulty to downstream: {:?}",
                                                e
                                            );
                                            TproxyError::disconnect(TproxyErrorKind::ChannelErrorSender, downstream_id.unwrap_or(0))
                                        })?;
                                }

                                if let Some(notify) = notify_opt {
                                    debug!("Down: Sending mining.notify");
                                    self.downstream_channel_state
                                        .downstream_sv1_sender
                                        .send(notify.into())
                                        .await
                                        .map_err(|e| {
                                            error!("Down: Failed to send mining.notify to downstream: {:?}", e);
                                            TproxyError::disconnect(TproxyErrorKind::ChannelErrorSender, downstream_id.unwrap_or(0))
                                        })?;
                                }
                                return Ok(());
                            }
                            _ => {
                                // Other notifications - forward if handshake complete
                                self.downstream_channel_state
                                    .downstream_sv1_sender
                                    .send(message.clone())
                                    .await
                                    .map_err(|e| {
                                        error!(
                                            "Down: Failed to send notification to downstream: {:?}",
                                            e
                                        );
                                        TproxyError::disconnect(
                                            TproxyErrorKind::ChannelErrorSender,
                                            downstream_id.unwrap_or(0),
                                        )
                                    })?;
                            }
                        }
                    } else {
                        // Handshake not complete - cache mining notifications but skip others
                        match notification.method.as_str() {
                            "mining.set_difficulty" => {
                                debug!("Down: SV1 handshake not complete, caching mining.set_difficulty");
                                self.downstream_data.super_safe_lock(|d| {
                                    d.cached_set_difficulty = Some(message);
                                });
                            }
                            "mining.notify" => {
                                debug!("Down: SV1 handshake not complete, caching mining.notify");
                                self.downstream_data.super_safe_lock(|d| {
                                    d.cached_notify = Some(message.clone());
                                    let notify =
                                        server_to_client::Notify::try_from(notification.clone())
                                            .expect("this must be a mining.notify");
                                    d.last_job_version_field = Some(notify.version.0);
                                });
                            }
                            _ => {
                                debug!(
                                    "Down: SV1 handshake not complete, skipping other notification"
                                );
                            }
                        }
                    }
                } else if is_queued_sv1_handshake_response {
                    // For non-notification messages, send if processing queued handshake responses
                    self.downstream_channel_state
                        .downstream_sv1_sender
                        .send(message.clone())
                        .await
                        .map_err(|e| {
                            error!("Down: Failed to send queued message to downstream: {:?}", e);
                            TproxyError::disconnect(
                                TproxyErrorKind::ChannelErrorSender,
                                downstream_id.unwrap_or(0),
                            )
                        })?;
                } else {
                    // Neither handshake complete nor queued response - skip non-notification
                    // messages
                    debug!("Down: SV1 handshake not complete, skipping non-notification message");
                }
            }
            Err(e) => {
                let downstream_id = self.downstream_id;
                error!(
                    "Sv1 message handler error for downstream {}: {:?}",
                    downstream_id, e
                );
                return Err(TproxyError::disconnect(e, downstream_id));
            }
        }

        Ok(())
    }

    /// Handles messages received from the downstream SV1 miner.
    ///
    /// This method processes SV1 protocol messages sent by the miner, including:
    /// - `mining.subscribe` - Subscription requests
    /// - `mining.authorize` - Authorization requests
    /// - `mining.submit` - Share submissions
    /// - Other SV1 protocol messages
    ///
    /// The method delegates message processing to the downstream data handler,
    /// which implements the SV1 protocol logic and generates appropriate responses.
    /// Responses are sent back to the miner, while share submissions are forwarded
    /// to the SV1 server for upstream processing.
    pub async fn handle_downstream_message(&self) -> TproxyResult<(), error::Downstream> {
        let downstream_id = self.downstream_id;
        let message = match self
            .downstream_channel_state
            .downstream_sv1_receiver
            .recv()
            .await
        {
            Ok(msg) => msg,
            Err(e) => {
                error!("Error receiving downstream message: {:?}", e);
                return Err(TproxyError::disconnect(e, downstream_id));
            }
        };

        self.downstream_channel_state
            .sv1_server_sender
            .send((downstream_id, message))
            .await
            .map_err(|_| TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender))?;

        Ok(())
    }

    /// Handles SV1 handshake completion after mining.authorize.
    ///
    /// This method is called when the downstream completes the SV1 handshake
    /// (subscribe + authorize). It sends any cached messages in the correct order:
    /// set_difficulty first, then notify.
    pub async fn handle_sv1_handshake_completion(&self) -> TproxyResult<(), error::Downstream> {
        let (cached_set_difficulty, cached_notify, downstream_id) =
            self.downstream_data.super_safe_lock(|d| {
                self.sv1_handshake_complete
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                (
                    d.cached_set_difficulty.take(),
                    d.cached_notify.take(),
                    self.downstream_id,
                )
            });
        debug!("Down: SV1 handshake completed for downstream");

        // Send cached messages in correct order: set_difficulty first, then notify
        if let Some(set_difficulty_msg) = cached_set_difficulty {
            debug!("Down: Sending cached mining.set_difficulty after handshake completion");
            self.downstream_channel_state
                .downstream_sv1_sender
                .send(set_difficulty_msg)
                .await
                .map_err(|e| {
                    error!(
                        "Down: Failed to send cached mining.set_difficulty to downstream: {:?}",
                        e
                    );
                    TproxyError::disconnect(TproxyErrorKind::ChannelErrorSender, downstream_id)
                })?;

            // Update target and hashrate after sending set_difficulty
            self.downstream_data.super_safe_lock(|d| {
                if let Some(new_target) = d.pending_target.take() {
                    d.target = new_target;
                }
                if let Some(new_hashrate) = d.pending_hashrate.take() {
                    d.hashrate = Some(new_hashrate);
                }
            });
        }

        if let Some(notify_msg) = cached_notify {
            debug!("Down: Sending cached mining.notify after handshake completion");
            self.downstream_channel_state
                .downstream_sv1_sender
                .send(notify_msg)
                .await
                .map_err(|e| {
                    error!(
                        "Down: Failed to send cached mining.notify to downstream: {:?}",
                        e
                    );
                    TproxyError::disconnect(TproxyErrorKind::ChannelErrorSender, downstream_id)
                })?;
            // Update last job received time for keepalive tracking
            self.downstream_data.super_safe_lock(|d| {
                d.last_job_received_time = Some(Instant::now());
            });
        }

        Ok(())
    }
}
