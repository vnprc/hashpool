use crate::{
    downstream_sv1,
    error::ProxyResult,
    proxy_config::{DownstreamDifficultyConfig, UpstreamDifficultyConfig},
    status,
};
use super::super::miner_stats;
use async_channel::{bounded, Receiver, Sender};
use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream},
    prelude::*,
    task,
};
use error_handling::handle_result;
use futures::FutureExt;
use tokio::{sync::broadcast, task::AbortHandle};

use super::{kill, DownstreamMessages, SubmitShareWithChannelId, SUBSCRIBE_TIMEOUT_SECS};

use roles_logic_sv2::{
    common_properties::{IsDownstream, IsMiningDownstream},
    utils::Mutex,
};

use crate::error::Error;
use futures::select;
use tokio_util::codec::{FramedRead, LinesCodec};

use std::{net::SocketAddr, sync::Arc};
use tracing::{debug, info, warn};
use v1::{
    client_to_server::{self, Submit},
    json_rpc, server_to_client,
    utils::{Extranonce, HexU32Be},
    IsServer,
};

const MAX_LINE_LENGTH: usize = 2_usize.pow(16);

/// Handles the sending and receiving of messages to and from an SV2 Upstream role (most typically
/// a SV2 Pool server).
#[derive(Debug)]
pub struct Downstream {
    /// List of authorized Downstream Mining Devices.
    pub(super) connection_id: u32,
    authorized_names: Vec<String>,
    extranonce1: Vec<u8>,
    /// `extranonce1` to be sent to the Downstream in the SV1 `mining.subscribe` message response.
    //extranonce1: Vec<u8>,
    //extranonce2_size: usize,
    /// Version rolling mask bits
    version_rolling_mask: Option<HexU32Be>,
    /// Minimum version rolling mask bits size
    version_rolling_min_bit: Option<HexU32Be>,
    /// Sends a SV1 `mining.submit` message received from the Downstream role to the `Bridge` for
    /// translation into a SV2 `SubmitSharesExtended`.
    tx_sv1_bridge: Sender<DownstreamMessages>,
    /// Sends message to the SV1 Downstream role.
    tx_outgoing: Sender<json_rpc::Message>,
    /// True if this is the first job received from `Upstream`.
    first_job_received: bool,
    extranonce2_len: usize,
    pub(super) difficulty_mgmt: DownstreamDifficultyConfig,
    pub(super) upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
    last_job_id: String, // we usually receive a String on SV1 messages, no need to cast to u32
    miner_tracker: Arc<miner_stats::MinerTracker>,
    miner_id: u32,
    stats_handle: Option<super::super::stats_client::StatsHandle>,
}

impl Downstream {
    #[cfg(test)]
    pub fn new(
        connection_id: u32,
        authorized_names: Vec<String>,
        extranonce1: Vec<u8>,
        version_rolling_mask: Option<HexU32Be>,
        version_rolling_min_bit: Option<HexU32Be>,
        tx_sv1_bridge: Sender<DownstreamMessages>,
        tx_outgoing: Sender<json_rpc::Message>,
        first_job_received: bool,
        extranonce2_len: usize,
        difficulty_mgmt: DownstreamDifficultyConfig,
        upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
        last_job_id: String,
        miner_tracker: Arc<miner_stats::MinerTracker>,
        miner_id: u32,
        stats_handle: Option<super::super::stats_client::StatsHandle>,
    ) -> Self {
        Downstream {
            connection_id,
            authorized_names,
            extranonce1,
            version_rolling_mask,
            version_rolling_min_bit,
            tx_sv1_bridge,
            tx_outgoing,
            first_job_received,
            extranonce2_len,
            difficulty_mgmt,
            upstream_difficulty_config,
            last_job_id,
            miner_tracker,
            miner_id,
            stats_handle,
        }
    }
    /// Instantiate a new `Downstream`.
    #[allow(clippy::too_many_arguments)]
    pub async fn new_downstream(
        stream: TcpStream,
        connection_id: u32,
        tx_sv1_bridge: Sender<DownstreamMessages>,
        mut rx_sv1_notify: broadcast::Receiver<server_to_client::Notify<'static>>,
        tx_status: status::Sender,
        extranonce1: Vec<u8>,
        last_notify: Option<server_to_client::Notify<'static>>,
        extranonce2_len: usize,
        host: String,
        difficulty_config: DownstreamDifficultyConfig,
        upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
        task_collector: Arc<Mutex<Vec<(AbortHandle, String)>>>,
        miner_tracker: Arc<miner_stats::MinerTracker>,
        stats_handle: Option<super::super::stats_client::StatsHandle>,
    ) {
        // Get peer address before moving stream into Arc
        let peer_addr = stream.peer_addr().unwrap();
        let stream = std::sync::Arc::new(stream);

        // Reads and writes from Downstream SV1 Mining Device Client
        let (socket_reader, socket_writer) = (stream.clone(), stream);
        let (tx_outgoing, receiver_outgoing) = bounded(10);

        let socket_writer_clone = socket_writer.clone();
        // Used to send SV1 `mining.notify` messages to the Downstreams
        let _socket_writer_notify = socket_writer;

        // Register miner with tracker first
        let miner_name = format!("miner-{}", connection_id);
        let miner_id = miner_tracker.add_miner(peer_addr, miner_name.clone()).await;

        // Send stats for new miner connection
        if let Some(ref handle) = stats_handle {
            use super::super::stats_client::StatsMessage;
            handle.send_stats(StatsMessage::DownstreamConnected {
                downstream_id: miner_id,
                flags: 0,
                name: miner_name,
            });
        }

        let downstream = Arc::new(Mutex::new(Downstream {
            connection_id,
            authorized_names: vec![],
            extranonce1,
            //extranonce1: extranonce1.to_vec(),
            version_rolling_mask: None,
            version_rolling_min_bit: None,
            tx_sv1_bridge,
            tx_outgoing,
            first_job_received: false,
            extranonce2_len,
            difficulty_mgmt: difficulty_config,
            upstream_difficulty_config,
            last_job_id: "".to_string(),
            miner_tracker: miner_tracker.clone(),
            miner_id,
            stats_handle: stats_handle.clone(),
        }));
        let self_ = downstream.clone();
        info!("📋 Registered miner {} from {} with ID {}", connection_id, peer_addr, miner_id);

        let host_ = host.clone();
        // The shutdown channel is used local to the `Downstream::new_downstream()` function.
        // Each task is set broadcast a shutdown message at the end of their lifecycle with
        // `kill()`, and each task has a receiver to listen for the shutdown message. When a
        // shutdown message is received the task should `break` its loop. For any errors that should
        // shut a task down, we should `break` out of the loop, so that the `kill` function
        // can send the shutdown broadcast. EXTRA: The since all downstream tasks rely on
        // receiving messages with a future (either TCP recv or Receiver<_>) we use the
        // futures::select! macro to merge the receiving end of a task channels into a single loop
        // within the task
        let (tx_shutdown, rx_shutdown): (Sender<bool>, Receiver<bool>) = async_channel::bounded(3);

        let rx_shutdown_clone = rx_shutdown.clone();
        let tx_shutdown_clone = tx_shutdown.clone();
        let tx_status_reader = tx_status.clone();
        let task_collector_mining_device = task_collector.clone();
        let miner_tracker_reader = miner_tracker.clone();
        // Task to read from SV1 Mining Device Client socket via `socket_reader`. Depending on the
        // SV1 message received, a message response is sent directly back to the SV1 Downstream
        // role, or the message is sent upwards to the Bridge for translation into a SV2 message
        // and then sent to the SV2 Upstream role.
        let socket_reader_task = tokio::task::spawn(async move {
            let reader = BufReader::new(&*socket_reader);
            let mut messages = FramedRead::new(
                async_compat::Compat::new(reader),
                LinesCodec::new_with_max_length(MAX_LINE_LENGTH),
            );
            loop {
                // Read message from SV1 Mining Device Client socket
                // On message receive, parse to `json_rpc:Message` and send to Upstream
                // `Translator.receive_downstream` via `sender_upstream` done in
                // `send_message_upstream`.
                select! {
                    res = messages.next().fuse() => {
                        match res {
                            Some(Ok(incoming)) => {
                                debug!("Receiving from Mining Device {}: {:?}", &host_, &incoming);
                                let incoming: json_rpc::Message = handle_result!(tx_status_reader, serde_json::from_str(&incoming));
                                // Handle what to do with message
                                // if let json_rpc::Message

                                // if message is Submit Shares update difficulty management
                                if let v1::Message::StandardRequest(standard_req) = incoming.clone() {
                                    if let Ok(Submit{..}) = standard_req.try_into() {
                                        handle_result!(tx_status_reader, Self::save_share(self_.clone()));
                                        // Track share submission for this miner with current hashrate from difficulty mgmt
                                        let current_hashrate = self_.safe_lock(|s| s.difficulty_mgmt.min_individual_miner_hashrate).unwrap();
                                        miner_tracker_reader.increment_shares(miner_id, current_hashrate).await;

                                        // Send stats for share submission
                                        let stats_handle = self_.safe_lock(|s| s.stats_handle.clone()).unwrap();
                                        if let Some(handle) = stats_handle {
                                            use super::super::stats_client::StatsMessage;
                                            let timestamp = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap()
                                                .as_millis() as u64;
                                            handle.send_stats(StatsMessage::ShareSubmitted {
                                                downstream_id: miner_id,
                                                timestamp,
                                            });
                                            // Send hashrate update - even if 0, so dashboard shows current state
                                            let hashrate = miner_tracker_reader.get_hashrate(miner_id).await.unwrap_or(0.0);
                                            handle.send_stats(StatsMessage::HashrateUpdate {
                                                downstream_id: miner_id,
                                                hashrate,
                                                timestamp,
                                            });
                                        }
                                    }
                                }

                                let res = Self::handle_incoming_sv1(self_.clone(), incoming).await;
                                handle_result!(tx_status_reader, res);
                            }
                            Some(Err(_)) => {
                                handle_result!(tx_status_reader, Err(Error::Sv1MessageTooLong));
                            }
                            None => {
                                handle_result!(tx_status_reader, Err(
                                    std::io::Error::new(
                                        std::io::ErrorKind::ConnectionAborted,
                                        "Connection closed by client"
                                    )
                                ));
                            }
                        }
                    },
                    _ = rx_shutdown_clone.recv().fuse() => {
                        break;
                    }
                };
            }
            // Remove miner from tracker when connection closes
            miner_tracker_reader.remove_miner(miner_id).await;
            info!("📤 Removed miner {} from tracker", miner_id);

            // Send stats for miner disconnection
            let stats_handle = self_.safe_lock(|s| s.stats_handle.clone()).unwrap();
            if let Some(handle) = stats_handle {
                use super::super::stats_client::StatsMessage;
                handle.send_stats(StatsMessage::DownstreamDisconnected {
                    downstream_id: miner_id,
                });
            }

            kill(&tx_shutdown_clone).await;
            warn!("Downstream: Shutting down sv1 downstream reader");
        });
        let _ = task_collector_mining_device.safe_lock(|a| {
            a.push((
                socket_reader_task.abort_handle(),
                "socket_reader_task".to_string(),
            ))
        });

        let rx_shutdown_clone = rx_shutdown.clone();
        let tx_shutdown_clone = tx_shutdown.clone();
        let tx_status_writer = tx_status.clone();
        let host_ = host.clone();

        let task_collector_new_sv1_message_no_transl = task_collector.clone();
        // Task to receive SV1 message responses to SV1 messages that do NOT need translation.
        // These response messages are sent directly to the SV1 Downstream role.
        let socket_writer_task = tokio::task::spawn(async move {
            loop {
                select! {
                    res = receiver_outgoing.recv().fuse() => {
                        let to_send = handle_result!(tx_status_writer, res);
                        let to_send = match serde_json::to_string(&to_send) {
                            Ok(string) => format!("{}\n", string),
                            Err(_e) => {
                                debug!("\nDownstream: Bad SV1 server message\n");
                                break;
                            }
                        };
                        debug!("Sending to Mining Device: {} - {:?}", &host_, &to_send);
                        let res = (&*socket_writer_clone)
                                    .write_all(to_send.as_bytes())
                                    .await;
                        handle_result!(tx_status_writer, res);
                    },
                    _ = rx_shutdown_clone.recv().fuse() => {
                            break;
                        }
                };
            }
            kill(&tx_shutdown_clone).await;
            warn!(
                "Downstream: Shutting down sv1 downstream writer: {}",
                &host_
            );
        });
        let _ = task_collector_new_sv1_message_no_transl.safe_lock(|a| {
            a.push((
                socket_writer_task.abort_handle(),
                "socket_writer_task".to_string(),
            ))
        });

        let tx_status_notify = tx_status;
        let self_ = downstream.clone();

        let task_collector_notify_task = task_collector.clone();
        let notify_task = tokio::task::spawn(async move {
            let timeout_timer = std::time::Instant::now();
            let mut first_sent = false;
            loop {
                let is_a = match downstream.safe_lock(|d| !d.authorized_names.is_empty()) {
                    Ok(is_a) => is_a,
                    Err(_e) => {
                        debug!("\nDownstream: Poison Lock - authorized_names\n");
                        break;
                    }
                };
                if is_a && !first_sent && last_notify.is_some() {
                    let target = handle_result!(
                        tx_status_notify,
                        Self::hash_rate_to_target(downstream.clone())
                    );
                    // make sure the mining start time is initialized and reset number of shares
                    // submitted
                    handle_result!(
                        tx_status_notify,
                        Self::init_difficulty_management(downstream.clone(), &target).await
                    );
                    let message =
                        handle_result!(tx_status_notify, Self::get_set_difficulty(target));
                    handle_result!(
                        tx_status_notify,
                        Downstream::send_message_downstream(downstream.clone(), message).await
                    );

                    let sv1_mining_notify_msg = last_notify.clone().unwrap();

                    self_
                        .safe_lock(|s| s.last_job_id = sv1_mining_notify_msg.clone().job_id)
                        .unwrap();

                    let message: json_rpc::Message = sv1_mining_notify_msg.into();
                    handle_result!(
                        tx_status_notify,
                        Downstream::send_message_downstream(downstream.clone(), message).await
                    );
                    if let Err(_e) = downstream.clone().safe_lock(|s| {
                        s.first_job_received = true;
                    }) {
                        debug!("\nDownstream: Poison Lock - first_job_received\n");
                        break;
                    }
                    first_sent = true;
                } else if is_a {
                    // if hashrate has changed, update difficulty management, and send new
                    // mining.set_difficulty
                    select! {
                        res = rx_sv1_notify.recv().fuse() => {
                            // if hashrate has changed, update difficulty management, and send new mining.set_difficulty
                            let new_hashrate = handle_result!(tx_status_notify, Self::try_update_difficulty_settings(downstream.clone()).await);

                            // Update miner tracker with real calculated hashrate
                            if let Some(hashrate) = new_hashrate {
                                let (miner_tracker, miner_id, stats_handle) = downstream.safe_lock(|d| (d.miner_tracker.clone(), d.miner_id, d.stats_handle.clone())).unwrap();
                                miner_tracker.update_hashrate(miner_id, hashrate as f64).await;

                                // Send stats update with new hashrate
                                if let Some(handle) = stats_handle {
                                    use super::super::stats_client::StatsMessage;
                                    let timestamp = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_millis() as u64;
                                    handle.send_stats(StatsMessage::HashrateUpdate {
                                        downstream_id: miner_id,
                                        hashrate: hashrate as f64,
                                        timestamp,
                                    });
                                }
                            }

                            let sv1_mining_notify_msg = handle_result!(tx_status_notify, res);
                            let message: json_rpc::Message = sv1_mining_notify_msg.clone().into();

                            self_.safe_lock(|s| s.last_job_id = sv1_mining_notify_msg.job_id).unwrap();

                            handle_result!(tx_status_notify, Downstream::send_message_downstream(downstream.clone(), message).await);
                        },
                        _ = rx_shutdown.recv().fuse() => {
                                break;
                            }
                    };
                } else {
                    // timeout connection if miner does not send the authorize message after sending
                    // a subscribe
                    if timeout_timer.elapsed().as_secs() > SUBSCRIBE_TIMEOUT_SECS {
                        debug!(
                            "Downstream: miner.subscribe/miner.authorize TIMOUT for {}",
                            &host
                        );
                        break;
                    }
                    task::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
            let _ = Self::remove_miner_hashrate_from_channel(self_);
            kill(&tx_shutdown).await;
            warn!(
                "Downstream: Shutting down sv1 downstream job notifier for {}",
                &host
            );
        });

        let _ = task_collector_notify_task
            .safe_lock(|a| a.push((notify_task.abort_handle(), "notify_task".to_string())));
    }

    /// Accept connections from one or more SV1 Downstream roles (SV1 Mining Devices) and create a
    /// new `Downstream` for each connection.
    #[allow(clippy::too_many_arguments)]
    pub fn accept_connections(
        downstream_addr: SocketAddr,
        tx_sv1_submit: Sender<DownstreamMessages>,
        tx_mining_notify: broadcast::Sender<server_to_client::Notify<'static>>,
        tx_status: status::Sender,
        bridge: Arc<Mutex<crate::proxy::Bridge>>,
        downstream_difficulty_config: DownstreamDifficultyConfig,
        upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
        task_collector: Arc<Mutex<Vec<(AbortHandle, String)>>>,
        miner_tracker: Arc<miner_stats::MinerTracker>,
        stats_handle: Option<super::super::stats_client::StatsHandle>,
    ) {
        let task_collector_downstream = task_collector.clone();

        let accept_connections = tokio::task::spawn(async move {
            let downstream_listener = TcpListener::bind(downstream_addr).await.unwrap();
            let mut downstream_incoming = downstream_listener.incoming();

            while let Some(stream) = downstream_incoming.next().await {
                let stream = stream.expect("Err on SV1 Downstream connection stream");
                let expected_hash_rate = downstream_difficulty_config.min_individual_miner_hashrate;
                let open_sv1_downstream = bridge
                    .safe_lock(|s| s.on_new_sv1_connection(expected_hash_rate))
                    .unwrap();

                let host = stream.peer_addr().unwrap().to_string();
                match open_sv1_downstream {
                    Ok(opened) => {
                        info!("PROXY SERVER - ACCEPTING FROM DOWNSTREAM: {}", host);
                        Downstream::new_downstream(
                            stream,
                            opened.channel_id,
                            tx_sv1_submit.clone(),
                            tx_mining_notify.subscribe(),
                            tx_status.listener_to_connection(),
                            opened.extranonce,
                            opened.last_notify,
                            opened.extranonce2_len as usize,
                            host,
                            downstream_difficulty_config.clone(),
                            upstream_difficulty_config.clone(),
                            task_collector_downstream.clone(),
                            miner_tracker.clone(),
                            stats_handle.clone(),
                        )
                        .await;
                    }
                    Err(e) => {
                        tracing::error!("Failed to create a new downstream connection: {:?}", e);
                    }
                }
            }
        });
        let _ = task_collector.safe_lock(|a| {
            a.push((
                accept_connections.abort_handle(),
                "accept_connections".to_string(),
            ))
        });
    }

    /// As SV1 messages come in, determines if the message response needs to be translated to SV2
    /// and sent to the `Upstream`, or if a direct response can be sent back by the `Translator`
    /// (SV1 and SV2 protocol messages are NOT 1-to-1).
    async fn handle_incoming_sv1(
        self_: Arc<Mutex<Self>>,
        message_sv1: json_rpc::Message,
    ) -> Result<(), super::super::error::Error<'static>> {
        // Check if this is an authorize message and extract the worker name
        let worker_name = if let json_rpc::Message::StandardRequest(ref req) = message_sv1 {
            if let Ok(auth) = client_to_server::Authorize::try_from(req.clone()) {
                Some(auth.name.clone())
            } else {
                None
            }
        } else {
            None
        };

        // `handle_message` in `IsServer` trait + calls `handle_request`
        // TODO: Map err from V1Error to Error::V1Error
        let response = self_.safe_lock(|s| s.handle_message(message_sv1)).unwrap();

        // If it was an authorize message and it succeeded, add to authorized names
        if let Some(name) = worker_name {
            if response.is_ok() {
                self_.safe_lock(|s| s.authorize(&name)).ok();
            }
        }

        match response {
            Ok(res) => {
                if let Some(r) = res {
                    // If some response is received, indicates no messages translation is needed
                    // and response should be sent directly to the SV1 Downstream. Otherwise,
                    // message will be sent to the upstream Translator to be translated to SV2 and
                    // forwarded to the `Upstream`
                    // let sender = self_.safe_lock(|s| s.connection.sender_upstream)
                    if let Err(e) = Self::send_message_downstream(self_, r.into()).await {
                        return Err(e.into());
                    }
                    Ok(())
                } else {
                    // If None response is received, indicates this SV1 message received from the
                    // Downstream MD is passed to the `Translator` for translation into SV2
                    Ok(())
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Send SV1 response message that is generated by `Downstream` (as opposed to being received
    /// by `Bridge`) to be written to the SV1 Downstream role.
    pub(super) async fn send_message_downstream(
        self_: Arc<Mutex<Self>>,
        response: json_rpc::Message,
    ) -> Result<(), async_channel::SendError<v1::Message>> {
        let sender = self_.safe_lock(|s| s.tx_outgoing.clone()).unwrap();
        debug!("To DOWN: {:?}", response);
        sender.send(response).await
    }

    /// Send SV1 response message that is generated by `Downstream` (as opposed to being received
    /// by `Bridge`) to be written to the SV1 Downstream role.
    pub(super) async fn send_message_upstream(
        self_: Arc<Mutex<Self>>,
        msg: DownstreamMessages,
    ) -> ProxyResult<'static, ()> {
        let sender = self_.safe_lock(|s| s.tx_sv1_bridge.clone()).unwrap();
        debug!("To Bridge: {:?}", msg);
        let _ = sender.send(msg).await;
        Ok(())
    }
}

/// Implements `IsServer` for `Downstream` to handle the SV1 messages.
impl IsServer<'static> for Downstream {
    /// Handle the incoming `mining.configure` message which is received after a Downstream role is
    /// subscribed and authorized. Contains the version rolling mask parameters.
    fn handle_configure(
        &mut self,
        request: &client_to_server::Configure,
    ) -> (Option<server_to_client::VersionRollingParams>, Option<bool>) {
        info!("Down: Configuring");
        debug!("Down: Handling mining.configure: {:?}", &request);

        // TODO 0x1FFFE000 should be configured
        // = 11111111111111110000000000000
        // this is a reasonable default as it allows all 16 version bits to be used
        // If the tproxy/pool needs to use some version bits this needs to be configurable
        // so upstreams can negotiate with downstreams. When that happens this should consider
        // the min_bit_count in the mining.configure message
        self.version_rolling_mask = request
            .version_rolling_mask()
            .map(|mask| HexU32Be(mask & 0x1FFFE000));
        self.version_rolling_min_bit = request.version_rolling_min_bit_count();

        debug!(
            "Negotiated version_rolling_mask is {:?}",
            self.version_rolling_mask
        );
        (
            Some(server_to_client::VersionRollingParams::new(
                self.version_rolling_mask.clone().unwrap_or(HexU32Be(0)),
                self.version_rolling_min_bit.clone().unwrap_or(HexU32Be(0)),
            ).expect("Version mask invalid, automatic version mask selection not supported, please change it in carte::downstream_sv1::mod.rs")),
            Some(false),
        )
    }

    /// Handle the response to a `mining.subscribe` message received from the client.
    /// The subscription messages are erroneous and just used to conform the SV1 protocol spec.
    /// Because no one unsubscribed in practice, they just unplug their machine.
    fn handle_subscribe(&self, request: &client_to_server::Subscribe) -> Vec<(String, String)> {
        info!("Down: Subscribing");
        debug!("Down: Handling mining.subscribe: {:?}", &request);

        let set_difficulty_sub = (
            "mining.set_difficulty".to_string(),
            downstream_sv1::new_subscription_id(),
        );
        let notify_sub = (
            "mining.notify".to_string(),
            "ae6812eb4cd7735a302a8a9dd95cf71f".to_string(),
        );

        vec![set_difficulty_sub, notify_sub]
    }

    /// Any numbers of workers may be authorized at any time during the session. In this way, a
    /// large number of independent Mining Devices can be handled with a single SV1 connection.
    /// https://bitcoin.stackexchange.com/questions/29416/how-do-pool-servers-handle-multiple-workers-sharing-one-connection-with-stratum
    fn handle_authorize(&self, request: &client_to_server::Authorize) -> bool {
        info!("Down: Authorizing worker: {}", request.name);
        debug!("Down: Handling mining.authorize: {:?}", &request);

        // Update miner name in tracker with the real worker name
        let miner_tracker = self.miner_tracker.clone();
        let miner_id = self.miner_id;
        let worker_name = request.name.clone();
        let stats_handle = self.stats_handle.clone();
        tokio::spawn(async move {
            miner_tracker.update_miner_name(miner_id, worker_name.clone()).await;

            // Send updated name to stats service
            if let Some(handle) = stats_handle {
                use super::super::stats_client::StatsMessage;
                handle.send_stats(StatsMessage::DownstreamConnected {
                    downstream_id: miner_id,
                    flags: 0,
                    name: worker_name,
                });
            }
        });

        true
    }

    /// When miner find the job which meets requested difficulty, it can submit share to the server.
    /// Only [Submit](client_to_server::Submit) requests for authorized user names can be submitted.
    fn handle_submit(&self, request: &client_to_server::Submit<'static>) -> bool {
        debug!("Down: Submitting Share {:?}", request);
        debug!("Down: Handling mining.submit: {:?}", &request);

        // TODO: Check if receiving valid shares by adding diff field to Downstream

        if request.job_id == self.last_job_id {
            let to_send = SubmitShareWithChannelId {
                channel_id: self.connection_id,
                share: request.clone(),
                extranonce: self.extranonce1.clone(),
                extranonce2_len: self.extranonce2_len,
                version_rolling_mask: self.version_rolling_mask.clone(),
                premint_secret: None,
            };

            self.tx_sv1_bridge
                .try_send(DownstreamMessages::SubmitShares(to_send))
                .unwrap();

            true
        } else {
            false
        }
    }

    /// Indicates to the server that the client supports the mining.set_extranonce method.
    fn handle_extranonce_subscribe(&self) {}

    /// Checks if a Downstream role is authorized.
    fn is_authorized(&self, name: &str) -> bool {
        self.authorized_names.contains(&name.to_string())
    }

    /// Authorizes a Downstream role.
    fn authorize(&mut self, name: &str) {
        self.authorized_names.push(name.to_string());
    }

    /// Sets the `extranonce1` field sent in the SV1 `mining.notify` message to the value specified
    /// by the SV2 `OpenExtendedMiningChannelSuccess` message sent from the Upstream role.
    fn set_extranonce1(
        &mut self,
        _extranonce1: Option<Extranonce<'static>>,
    ) -> Extranonce<'static> {
        self.extranonce1.clone().try_into().unwrap()
    }

    /// Returns the `Downstream`'s `extranonce1` value.
    fn extranonce1(&self) -> Extranonce<'static> {
        self.extranonce1.clone().try_into().unwrap()
    }

    /// Sets the `extranonce2_size` field sent in the SV1 `mining.notify` message to the value
    /// specified by the SV2 `OpenExtendedMiningChannelSuccess` message sent from the Upstream role.
    fn set_extranonce2_size(&mut self, _extra_nonce2_size: Option<usize>) -> usize {
        self.extranonce2_len
    }

    /// Returns the `Downstream`'s `extranonce2_size` value.
    fn extranonce2_size(&self) -> usize {
        self.extranonce2_len
    }

    /// Returns the version rolling mask.
    fn version_rolling_mask(&self) -> Option<HexU32Be> {
        self.version_rolling_mask.clone()
    }

    /// Sets the version rolling mask.
    fn set_version_rolling_mask(&mut self, mask: Option<HexU32Be>) {
        self.version_rolling_mask = mask;
    }

    /// Sets the minimum version rolling bit.
    fn set_version_rolling_min_bit(&mut self, mask: Option<HexU32Be>) {
        self.version_rolling_min_bit = mask
    }

    fn notify(&mut self) -> Result<json_rpc::Message, v1::error::Error> {
        unreachable!()
    }
}

impl IsMiningDownstream for Downstream {}

impl IsDownstream for Downstream {
    fn get_downstream_mining_data(
        &self,
    ) -> roles_logic_sv2::common_properties::CommonDownstreamData {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gets_difficulty_from_target() {
        let target = vec![
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 255, 127,
            0, 0, 0, 0, 0,
        ];
        let actual = Downstream::difficulty_from_target(target).unwrap();
        let expect = 512.0;
        assert_eq!(actual, expect);
    }
}
