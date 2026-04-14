//! ## Translator Sv2
//!
//! Provides the core logic and main struct (`TranslatorSv2`) for running a
//! Stratum V1 to Stratum V2 translation proxy.
//!
//! This module orchestrates the interaction between downstream SV1 miners and upstream SV2
//! applications (proxies or pool servers).
//!
//! The central component is the `TranslatorSv2` struct, which encapsulates the state and
//! provides the `start` method as the main entry point for running the translator service.
//! It relies on several sub-modules (`config`, `downstream_sv1`, `upstream_sv2`, `proxy`, `status`,
//! etc.) for specialized functionalities.
#![allow(clippy::module_inception)]
use async_channel::{unbounded, Receiver, Sender};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use stratum_apps::{
    fallback_coordinator::FallbackCoordinator,
    task_manager::TaskManager,
    utils::types::{Sv2Frame, GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub use stratum_apps::stratum_core::sv1_api::server_to_client;

use config::TranslatorConfig;

use crate::{
    error::TproxyErrorKind,
    payment::{
        cdk_handler::CdkQuoteNotificationHandler,
        custom_handler::{CustomMiningMessageHandler, NoopCustomMiningMessageHandler},
        wallet::create_wallet,
    },
    status::{State, Status},
    sv1::sv1_server::sv1_server::Sv1Server,
    sv2::{ChannelManager, Upstream},
    utils::UpstreamEntry,
};

pub mod config;
pub mod error;
pub mod faucet_api;
mod io_task;
#[cfg(feature = "monitoring")]
mod monitoring;
pub mod payment;
pub mod status;
pub mod sv1;
#[cfg(feature = "monitoring")]
mod sv1_monitoring;
pub mod sv2;
pub mod utils;

/// The main struct that manages the SV1/SV2 translator.
#[derive(Clone)]
pub struct TranslatorSv2 {
    config: TranslatorConfig,
    cancellation_token: CancellationToken,
    shutdown_notify: Arc<Notify>,
    is_alive: Arc<AtomicBool>,
}

#[cfg_attr(not(test), hotpath::measure_all)]
impl TranslatorSv2 {
    /// Creates a new `TranslatorSv2`.
    ///
    /// Initializes the translator with the given configuration and sets up
    /// the reconnect wait time.
    pub fn new(config: TranslatorConfig) -> Self {
        Self {
            config,
            cancellation_token: CancellationToken::new(),
            shutdown_notify: Arc::new(Notify::new()),
            is_alive: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Starts the translator.
    ///
    /// This method starts the main event loop, which handles connections,
    /// protocol translation, job management, and status reporting.
    pub async fn start(self) {
        info!("Starting Translator Proxy...");
        // only initialized once
        TPROXY_MODE
            .set(self.config.aggregate_channels.into())
            .expect("TPROXY_MODE initialized more than once");
        VARDIFF_ENABLED
            .set(self.config.downstream_difficulty_config.enable_vardiff)
            .expect("VARDIFF_ENABLED initialized more than once");

        let cancellation_token = self.cancellation_token.clone();
        let mut fallback_coordinator = FallbackCoordinator::new();

        let task_manager = Arc::new(TaskManager::new());
        let (status_sender, status_receiver) = async_channel::unbounded::<Status>();

        let (channel_manager_to_upstream_sender, channel_manager_to_upstream_receiver) =
            unbounded();
        let (upstream_to_channel_manager_sender, upstream_to_channel_manager_receiver) =
            unbounded();
        let (channel_manager_to_sv1_server_sender, channel_manager_to_sv1_server_receiver) =
            unbounded();
        let (sv1_server_to_channel_manager_sender, sv1_server_to_channel_manager_receiver) =
            unbounded();

        debug!("All inter-subsystem channels initialized");

        // Initialize CDK wallet and payment handler
        let (wallet_opt, custom_handler): (
            Option<Arc<cdk::wallet::Wallet>>,
            Arc<dyn CustomMiningMessageHandler>,
        ) = if let Some(mint_config) = &self.config.mint {
            match create_wallet(
                &mint_config.url,
                &self.config.wallet.mnemonic,
                &self.config.wallet.db_path,
            )
            .await
            {
                Ok(w) => {
                    let handler: Arc<dyn CustomMiningMessageHandler> =
                        Arc::new(CdkQuoteNotificationHandler::new(w.clone()));
                    (Some(w), handler)
                }
                Err(e) => {
                    error!(
                        "Failed to initialize CDK wallet: {e}. Proceeding without payment support."
                    );
                    (None, Arc::new(NoopCustomMiningMessageHandler))
                }
            }
        } else {
            (None, Arc::new(NoopCustomMiningMessageHandler))
        };

        if let Some(ref w) = wallet_opt {
            payment::quote_sweeper::spawn_quote_sweeper(
                w.clone(),
                self.config.wallet.locking_privkey.clone(),
            );
            let faucet_wallet = w.clone();
            let faucet_port = self.config.faucet_port;
            let faucet_timeout = self.config.faucet_timeout;
            let faucet_privkey = self.config.wallet.locking_privkey.clone();
            tokio::spawn(async move {
                faucet_api::run_faucet_api(faucet_port, faucet_wallet, faucet_timeout, faucet_privkey).await;
            });
        }

        let mut upstream_addresses = self
            .config
            .upstreams
            .iter()
            .map(|u| UpstreamEntry {
                host: u.address.clone(),
                port: u.port,
                authority_pubkey: u.authority_pubkey,
                tried_or_flagged: false,
            })
            .collect::<Vec<_>>();

        let downstream_addr: SocketAddr = SocketAddr::new(
            self.config.downstream_address.parse().unwrap(),
            self.config.downstream_port,
        );

        let mut sv1_server = Arc::new(Sv1Server::new(
            downstream_addr,
            channel_manager_to_sv1_server_receiver,
            sv1_server_to_channel_manager_sender,
            self.config.clone(),
        ));

        info!("Initializing upstream connection...");

        if let Err(e) = self
            .initialize_upstream(
                &mut upstream_addresses,
                channel_manager_to_upstream_receiver.clone(),
                upstream_to_channel_manager_sender.clone(),
                cancellation_token.clone(),
                fallback_coordinator.clone(),
                status_sender.clone(),
                task_manager.clone(),
                sv1_server.clone(),
                self.config.required_extensions.clone(),
            )
            .await
        {
            error!("Failed to initialize any upstream connection: {e:?}");
            self.shutdown_notify.notify_waiters();
            self.is_alive.store(false, Ordering::Relaxed);
            return;
        }

        let mut channel_manager: Arc<ChannelManager> = Arc::new(ChannelManager::new(
            channel_manager_to_upstream_sender,
            upstream_to_channel_manager_receiver,
            channel_manager_to_sv1_server_sender.clone(),
            sv1_server_to_channel_manager_receiver,
            status_sender.clone(),
            self.config.supported_extensions.clone(),
            self.config.required_extensions.clone(),
            custom_handler.clone(),
        ));

        info!("Launching ChannelManager tasks...");
        ChannelManager::run_channel_manager_tasks(
            channel_manager.clone(),
            cancellation_token.clone(),
            fallback_coordinator.clone(),
            status_sender.clone(),
            task_manager.clone(),
        )
        .await;

        // Start monitoring server if configured
        #[cfg(feature = "monitoring")]
        if let Some(monitoring_addr) = self.config.monitoring_address() {
            info!(
                "Initializing monitoring server on http://{}",
                monitoring_addr
            );

            let monitoring_server = stratum_apps::monitoring::MonitoringServer::new(
                monitoring_addr,
                Some(channel_manager.clone()), // SV2 channels opened with servers
                None,                          /* no SV2 channels opened with clients (SV1
                                                * handled separately) */
                std::time::Duration::from_secs(self.config.monitoring_cache_refresh_secs().unwrap_or(15)),
            )
            .expect("Failed to initialize monitoring server")
            .with_sv1_monitoring(sv1_server.clone()) // SV1 client connections
            .expect("Failed to add SV1 monitoring");

            // Fetch network from pool once per connection before run() consumes monitoring_server
            let network_handle = monitoring_server.network_handle();
            if let Some(pool_mon_url) = self.config.pool_monitoring_url().map(|s| s.to_string()) {
                tokio::spawn(async move {
                    fetch_pool_network(pool_mon_url, network_handle).await;
                });
            }

            // Create shutdown signal using cancellation token
            let cancellation_token_clone = cancellation_token.clone();
            let fallback_coordinator_token = fallback_coordinator.token();
            let shutdown_signal = async move {
                tokio::select! {
                    _ = cancellation_token_clone.cancelled() => {
                        info!("Monitoring server: received shutdown signal.");
                    }
                    _ = fallback_coordinator_token.cancelled() => {
                        info!("Monitoring server: fallback triggered.");
                    }
                }
            };

            let fallback_coordinator_clone = fallback_coordinator.clone();
            task_manager.spawn(async move {
                // we just spawned a new task that's relevant to fallback coordination
                // so register it with the fallback coordinator
                let fallback_handler = fallback_coordinator_clone.register();

                if let Err(e) = monitoring_server.run(shutdown_signal).await {
                    error!("Monitoring server error: {:?}", e);
                }

                // signal fallback coordinator that this task has completed its cleanup
                fallback_handler.done();
                info!("Monitoring server task exited and signaled fallback coordinator");
            });
        }

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Ctrl+C received — initiating graceful shutdown...");
                    cancellation_token.cancel();
                    break;
                }
                _ = cancellation_token.cancelled() => {
                    break;
                }
                message = status_receiver.recv() => {
                    if let Ok(status) = message {
                        match status.state {
                            State::DownstreamShutdown{downstream_id,..} => {
                                warn!("Downstream {downstream_id:?} disconnected — cleaning up sv1_server state.");
                                // clean up sv1_server state
                                sv1_server.handle_downstream_disconnect(downstream_id).await;
                            }
                            State::Sv1ServerShutdown(_) => {
                                warn!("SV1 Server shutdown requested — initiating full shutdown.");
                                cancellation_token.cancel();
                                break;
                            }
                            State::ChannelManagerShutdown(_) => {
                                warn!("Channel Manager shutdown requested — initiating full shutdown.");
                                cancellation_token.cancel();
                                break;
                            }
                            State::UpstreamShutdown(msg) => {
                                warn!("Upstream connection dropped: {msg:?} — attempting reconnection...");

                                // Trigger fallback and wait for all components to finish cleanup
                                fallback_coordinator.trigger_fallback_and_wait().await;
                                info!("All components finished fallback cleanup");

                                // Drain any buffered status messages from old components
                                while let Ok(old_status) = status_receiver.try_recv() {
                                    debug!("Draining buffered status message: {:?}", old_status.state);
                                }

                                // Create a fresh FallbackCoordinator for the reconnection attempt
                                fallback_coordinator = FallbackCoordinator::new();

                                // Recreate channels and components (old ones were closed during fallback)
                                let (channel_manager_to_upstream_sender, channel_manager_to_upstream_receiver) =
                                    unbounded();
                                let (upstream_to_channel_manager_sender, upstream_to_channel_manager_receiver) =
                                    unbounded();
                                let (channel_manager_to_sv1_server_sender, channel_manager_to_sv1_server_receiver) =
                                    unbounded();
                                let (sv1_server_to_channel_manager_sender, sv1_server_to_channel_manager_receiver) =
                                    unbounded();

                                sv1_server = Arc::new(Sv1Server::new(
                                    downstream_addr,
                                    channel_manager_to_sv1_server_receiver,
                                    sv1_server_to_channel_manager_sender,
                                    self.config.clone(),
                                ));

                                if let Err(e) = self.initialize_upstream(
                                    &mut upstream_addresses,
                                    channel_manager_to_upstream_receiver,
                                    upstream_to_channel_manager_sender,
                                    cancellation_token.clone(),
                                    fallback_coordinator.clone(),
                                    status_sender.clone(),
                                    task_manager.clone(),
                                    sv1_server.clone(),
                                    self.config.required_extensions.clone(),
                                ).await {
                                    error!("Couldn't perform fallback, shutting system down: {e:?}");
                                    cancellation_token.cancel();
                                    break;
                                }

                                channel_manager = Arc::new(ChannelManager::new(
                                    channel_manager_to_upstream_sender,
                                    upstream_to_channel_manager_receiver,
                                    channel_manager_to_sv1_server_sender,
                                    sv1_server_to_channel_manager_receiver,
                                    status_sender.clone(),
                                    self.config.supported_extensions.clone(),
                                    self.config.required_extensions.clone(),
                                    custom_handler.clone(),
                                ));

                                info!("Launching ChannelManager tasks...");
                                ChannelManager::run_channel_manager_tasks(
                                    channel_manager.clone(),
                                    cancellation_token.clone(),
                                    fallback_coordinator.clone(),
                                    status_sender.clone(),
                                    task_manager.clone(),
                                )
                                .await;

                                // Recreate monitoring server with new components
                                #[cfg(feature = "monitoring")]
                                if let Some(monitoring_addr) = self.config.monitoring_address() {
                                    info!(
                                        "Reinitializing monitoring server on http://{}",
                                        monitoring_addr
                                    );

                                    let monitoring_server = stratum_apps::monitoring::MonitoringServer::new(
                                        monitoring_addr,
                                        Some(channel_manager.clone()),
                                        None,
                                        std::time::Duration::from_secs(self.config.monitoring_cache_refresh_secs().unwrap_or(15)),
                                    )
                                    .expect("Failed to initialize monitoring server")
                                    .with_sv1_monitoring(sv1_server.clone())
                                    .expect("Failed to add SV1 monitoring");

                                    // Fetch network from pool once per connection before run() consumes monitoring_server
                                    let network_handle = monitoring_server.network_handle();
                                    if let Some(pool_mon_url) = self.config.pool_monitoring_url().map(|s| s.to_string()) {
                                        tokio::spawn(async move {
                                            fetch_pool_network(pool_mon_url, network_handle).await;
                                        });
                                    }

                                    let cancellation_token_clone = cancellation_token.clone();
                                    let fallback_coordinator_token = fallback_coordinator.token();
                                    let shutdown_signal = async move {
                                        tokio::select! {
                                            _ = cancellation_token_clone.cancelled() => {
                                                info!("Monitoring server: received shutdown signal.");
                                            }
                                            _ = fallback_coordinator_token.cancelled() => {
                                                info!("Monitoring server: fallback triggered.");
                                            }
                                        }
                                    };

                                    let monitoring_fallback = fallback_coordinator.clone();
                                    task_manager.spawn(async move {
                                        let fallback_handler = monitoring_fallback.register();

                                        if let Err(e) = monitoring_server.run(shutdown_signal).await {
                                            error!("Monitoring server error: {:?}", e);
                                        }

                                        // signal fallback coordinator that this task has completed its cleanup
                                        fallback_handler.done();
                                        info!("Monitoring server task exited and signaled fallback coordinator");
                                    });
                                }

                                info!("Upstream and ChannelManager restarted successfully.");
                            }
                        }
                    }
                }
            }
        }

        warn!(
            "Graceful shutdown: waiting {} seconds for tasks to finish",
            GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS
        );
        match tokio::time::timeout(
            std::time::Duration::from_secs(GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS),
            task_manager.join_all(),
        )
        .await
        {
            Ok(_) => {
                info!("All tasks joined cleanly");
            }
            Err(_) => {
                warn!(
                    "Tasks did not finish within {} seconds, aborting",
                    GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS
                );
                task_manager.abort_all().await;
                info!("Joining aborted tasks...");
                task_manager.join_all().await;
                warn!("Forced shutdown complete");
            }
        }
        self.shutdown_notify.notify_waiters();
        self.is_alive.store(false, Ordering::Relaxed);
        info!("TranslatorSv2 shutdown complete.");
    }

    pub async fn shutdown(&self) {
        if !self.is_alive.load(Ordering::Relaxed) {
            return;
        }
        // The Notified future is guaranteed to receive wakeups from notify_waiters()
        // as soon as it has been created, even if it has not yet been polled.
        let notified = self.shutdown_notify.notified();
        self.cancellation_token.cancel();
        notified.await;
    }

    /// Initializes the upstream connection list, handling retries, fallbacks, and flagging.
    ///
    /// Upstreams are tried sequentially, each receiving a fixed number of retries before we
    /// advance to the next entry. This ensures we exhaust every healthy upstream before shutting
    /// the translator down.
    ///
    /// The `tried_or_flagged` flag in the `UpstreamEntry` acts as the upstream's state machine:
    ///  `false` means "never tried", while `true` means "already connected or marked as
    /// malicious". Once an upstream is flagged we skip it on future loops
    /// to avoid hammering known-bad endpoints during failover.
    #[allow(clippy::too_many_arguments)]
    pub async fn initialize_upstream(
        &self,
        upstreams: &mut [UpstreamEntry],
        channel_manager_to_upstream_receiver: Receiver<Sv2Frame>,
        upstream_to_channel_manager_sender: Sender<Sv2Frame>,
        cancellation_token: CancellationToken,
        fallback_coordinator: FallbackCoordinator,
        status_sender: Sender<Status>,
        task_manager: Arc<TaskManager>,
        sv1_server_instance: Arc<Sv1Server>,
        required_extensions: Vec<u16>,
    ) -> Result<(), TproxyErrorKind> {
        const MAX_RETRIES: usize = 3;
        let upstream_len = upstreams.len();
        for (i, upstream_entry) in upstreams.iter_mut().enumerate() {
            // Skip upstreams already marked as malicious. We’ve previously failed or
            // blacklisted them, so no need to warn or attempt reconnecting again.
            if upstream_entry.tried_or_flagged {
                debug!(
                    "Upstream previously marked as malicious, skipping initial attempt warnings."
                );
                continue;
            }

            info!(
                "Trying upstream {} of {}: {}:{}",
                i + 1,
                upstream_len,
                upstream_entry.host,
                upstream_entry.port
            );
            for attempt in 1..=MAX_RETRIES {
                info!("Connection attempt {}/{}...", attempt, MAX_RETRIES);
                tokio::time::sleep(Duration::from_secs(1)).await;

                match try_initialize_upstream(
                    upstream_entry,
                    upstream_to_channel_manager_sender.clone(),
                    channel_manager_to_upstream_receiver.clone(),
                    cancellation_token.clone(),
                    fallback_coordinator.clone(),
                    status_sender.clone(),
                    task_manager.clone(),
                    required_extensions.clone(),
                )
                .await
                {
                    Ok(()) => {
                        // starting sv1 server instance
                        if let Err(e) = sv1_server_instance
                            .clone()
                            .start(
                                cancellation_token.clone(),
                                fallback_coordinator.clone(),
                                status_sender.clone(),
                                task_manager.clone(),
                            )
                            .await
                        {
                            error!("SV1 server startup failed: {e:?}");
                            return Err(e.kind);
                        }

                        upstream_entry.tried_or_flagged = true;
                        return Ok(());
                    }
                    Err(e) => {
                        warn!(
                            "Attempt {}/{} failed for {}:{}: {:?}",
                            attempt, MAX_RETRIES, upstream_entry.host, upstream_entry.port, e
                        );
                        if attempt == MAX_RETRIES {
                            warn!(
                                "Max retries reached for {}:{}, moving to next upstream",
                                upstream_entry.host, upstream_entry.port
                            );
                        }
                    }
                }
            }
            upstream_entry.tried_or_flagged = true;
        }

        tracing::error!("All upstreams failed after {} retries each", MAX_RETRIES);
        Err(TproxyErrorKind::CouldNotInitiateSystem)
    }
}

// Attempts to initialize a single upstream.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), hotpath::measure)]
async fn try_initialize_upstream(
    upstream_addr: &UpstreamEntry,
    upstream_to_channel_manager_sender: Sender<Sv2Frame>,
    channel_manager_to_upstream_receiver: Receiver<Sv2Frame>,
    cancellation_token: CancellationToken,
    fallback_coordinator: FallbackCoordinator,
    status_sender: Sender<Status>,
    task_manager: Arc<TaskManager>,
    required_extensions: Vec<u16>,
) -> Result<(), TproxyErrorKind> {
    let upstream = Upstream::new(
        upstream_addr,
        upstream_to_channel_manager_sender,
        channel_manager_to_upstream_receiver,
        cancellation_token.clone(),
        fallback_coordinator.clone(),
        task_manager.clone(),
        required_extensions,
    )
    .await?;

    upstream
        .start(
            cancellation_token,
            fallback_coordinator,
            status_sender,
            task_manager,
        )
        .await?;
    Ok(())
}

/// Defines the operational mode for Translator Proxy.
///
/// It can operate in two different modes that affect how Sv1
/// downstream connections are mapped to the upstream Sv2 channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TproxyMode {
    /// All Sv1 downstream connections share a single extended Sv2 channel.
    /// This mode uses extranonce_prefix allocation to distinguish between
    /// different downstream miners while presenting them as a single entity
    /// to the upstream server. This is more efficient for pools with many
    /// miners.
    Aggregated,
    /// Each Sv1 downstream connection gets its own dedicated extended Sv2 channel.
    /// This mode provides complete isolation between downstream connections
    /// but may be less efficient for large numbers of miners.
    NonAggregated,
}

impl From<bool> for TproxyMode {
    fn from(aggregate: bool) -> Self {
        if aggregate {
            return TproxyMode::Aggregated;
        }

        TproxyMode::NonAggregated
    }
}

static TPROXY_MODE: OnceLock<TproxyMode> = OnceLock::new();
static VARDIFF_ENABLED: OnceLock<bool> = OnceLock::new();

#[cfg(not(test))]
pub fn tproxy_mode() -> TproxyMode {
    *TPROXY_MODE.get().expect("TPROXY_MODE has to exist")
}

// We don’t initialize `TPROXY_MODE` in tests, so any test that
// depends on it will panic if the mode is undefined.
// This `cfg` wrapper ensures `tproxy_mode` does not panic in
// an undefined state by providing a default value when needed.
#[cfg(test)]
pub fn tproxy_mode() -> TproxyMode {
    *TPROXY_MODE.get_or_init(|| TproxyMode::Aggregated)
}

#[inline]
pub fn is_aggregated() -> bool {
    matches!(tproxy_mode(), TproxyMode::Aggregated)
}

#[inline]
pub fn is_non_aggregated() -> bool {
    matches!(tproxy_mode(), TproxyMode::NonAggregated)
}

#[cfg(not(test))]
pub fn vardiff_enabled() -> bool {
    *VARDIFF_ENABLED.get().expect("VARDIFF_ENABLED has to exist")
}

#[cfg(test)]
pub fn vardiff_enabled() -> bool {
    *VARDIFF_ENABLED.get_or_init(|| true)
}

impl Drop for TranslatorSv2 {
    fn drop(&mut self) {
        info!("TranslatorSv2 dropped");
        self.cancellation_token.cancel();
    }
}

/// Fetches the pool's monitoring API once per upstream connection and writes the reported
/// Bitcoin network name into `network_handle`. Retries are handled by the translator's
/// existing reconnect logic — no polling loop needed.
#[cfg(feature = "monitoring")]
async fn fetch_pool_network(
    pool_mon_url: String,
    network_handle: std::sync::Arc<std::sync::RwLock<Option<String>>>,
) {
    use stratum_apps::monitoring::GlobalInfo;
    let url = format!("{}/api/v1/global", pool_mon_url.trim_end_matches('/'));
    match reqwest::get(url).await {
        Ok(resp) => match resp.json::<GlobalInfo>().await {
            Ok(info) => {
                if let Ok(mut guard) = network_handle.write() {
                    *guard = info.network;
                }
            }
            Err(e) => warn!("Failed to parse pool global info: {e}"),
        },
        Err(e) => warn!("Failed to fetch pool network info from {pool_mon_url}: {e}"),
    }
}
