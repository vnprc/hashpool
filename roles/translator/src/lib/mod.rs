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
use anyhow::{Context, Result};
use async_channel::unbounded;
use bip39::Mnemonic;
use cdk::{
    amount::SplitTarget,
    nuts::{CurrencyUnit, SecretKey, SpendingConditions},
    wallet::Wallet,
    Amount,
};
use cdk_sqlite::WalletSqliteDatabase;
use std::{net::SocketAddr, path::Path, str::FromStr, sync::Arc};
use tokio::{sync::mpsc, time::Duration};
use tracing::{debug, error, info, warn};

pub use v1::server_to_client;

use config::TranslatorConfig;

use crate::{
    status::{State, Status},
    sv1::sv1_server::sv1_server::Sv1Server,
    sv2::{channel_manager::ChannelMode, ChannelManager, Upstream},
    task_manager::TaskManager,
    utils::ShutdownMessage,
};

pub mod config;
pub mod error;
pub mod faucet_api;
pub mod miner_stats;
pub mod monitoring;
pub mod status;
pub mod sv1;
pub mod sv2;
mod task_manager;
pub mod utils;

/// The main struct that manages the SV1/SV2 translator.
#[derive(Clone)]
pub struct TranslatorSv2 {
    config: TranslatorConfig,
    wallet: Option<Arc<Wallet>>,
    miner_tracker: Arc<miner_stats::MinerTracker>,
}

impl std::fmt::Debug for TranslatorSv2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranslatorSv2")
            .field("config", &self.config)
            .field("wallet", &self.wallet.is_some())
            .field("miner_tracker", &"MinerTracker")
            .finish()
    }
}

impl TranslatorSv2 {
    /// Creates a new `TranslatorSv2`.
    ///
    /// Initializes the translator with the given configuration and sets up
    /// the reconnect wait time.
    pub fn new(config: TranslatorConfig) -> Self {
        let metrics_window_secs = config.metrics_window_secs;
        Self {
            config,
            wallet: None,
            miner_tracker: Arc::new(miner_stats::MinerTracker::new(
                metrics_window_secs,
            )),
        }
    }

    /// Helper function to resolve and prepare DB paths
    fn resolve_and_prepare_db_path(config_path: &str) -> std::path::PathBuf {
        let path = Path::new(config_path);
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .expect("Failed to get current working directory")
                .join(path)
        };

        if let Some(parent) = full_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .expect("Failed to create parent directory for DB path");
            }
        }

        full_path
    }

    /// Creates and initializes a wallet for the translator
    async fn create_wallet(
        mint_url: String,
        mnemonic: String,
        db_path: String,
    ) -> Result<Arc<Wallet>> {
        debug!("Parsing mnemonic...");
        let seed = Mnemonic::from_str(&mnemonic)
            .with_context(|| format!("Invalid mnemonic: '{}'", mnemonic))?
            .to_seed_normalized("");
        let seed: [u8; 64] = seed
            .try_into()
            .map_err(|_| anyhow::anyhow!("Seed must be exactly 64 bytes"))?;
        debug!("Seed derived.");

        let db_path = Self::resolve_and_prepare_db_path(&db_path);
        debug!("Resolved db_path: {}", db_path.display());

        debug!("Creating localstore...");
        let localstore = WalletSqliteDatabase::new(db_path)
            .await
            .context("WalletSqliteDatabase::new failed")?;

        debug!("Creating wallet...");
        // TODO: Move "hash" currency unit to configuration (Phase 2)
        let wallet = Wallet::new(
            &mint_url,
            CurrencyUnit::Custom("hash".to_string()),
            Arc::new(localstore),
            seed,
            None,
        )
        .context("Failed to create wallet")?;
        debug!("Wallet created.");

        let balance = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(wallet.total_balance())
        });
        debug!("Wallet balance: {:?}", balance);

        Ok(Arc::new(wallet))
    }

    /// Starts the translator.
    ///
    /// This method starts the main event loop, which handles connections,
    /// protocol translation, job management, and status reporting.
    pub async fn start(mut self) {
        info!("Starting Translator Proxy...");

        // Initialize and validate wallet config if mint is configured
        if self.config.mint.is_some() {
            self.config
                .wallet
                .initialize()
                .expect("Failed to initialize wallet config");

            let mint_url = self
                .config
                .mint
                .as_ref()
                .map(|m| m.url.clone())
                .expect("Mint URL required for wallet");

            let db_path = std::env::var("CDK_WALLET_DB_PATH")
                .unwrap_or_else(|_| self.config.wallet.db_path.clone());

            match Self::create_wallet(
                mint_url,
                self.config.wallet.mnemonic.clone(),
                db_path,
            )
            .await
            {
                Ok(wallet) => {
                    info!("Wallet initialized successfully");
                    self.wallet = Some(wallet);
                }
                Err(e) => {
                    error!("Failed to create wallet: {}", e);
                    // Continue without wallet - quote functionality won't work but translator can
                    // still function
                }
            }
        }

        let (notify_shutdown, _) = tokio::sync::broadcast::channel::<ShutdownMessage>(1);
        let (shutdown_complete_tx, mut shutdown_complete_rx) = mpsc::channel::<()>(1);
        let task_manager = Arc::new(TaskManager::new());

        if let Some(wallet) = self.wallet.clone() {
            self.spawn_quote_sweeper(&task_manager, wallet.clone());

            // Start faucet API for ehash minting
            let faucet_port = self.config.faucet_port;
            let faucet_timeout = self.config.faucet_timeout;
            let faucet_locking_privkey = self.config.wallet.locking_privkey.clone();
            task_manager.spawn(faucet_api::run_faucet_api(faucet_port, wallet, faucet_timeout, faucet_locking_privkey));
        } else {
            debug!("Quote sweeper and faucet disabled: wallet not configured");
        }

        if let Some(monitoring_address) = self.config.monitoring_address.clone() {
            let translator_for_metrics = self.clone();
            task_manager.spawn(async move {
                if let Err(err) = monitoring::run_monitoring_server(
                    monitoring_address,
                    translator_for_metrics,
                )
                .await
                {
                    error!("Failed to start translator monitoring server: {}", err);
                }
            });
        }

        let (status_sender, status_receiver) = async_channel::unbounded::<Status>();

        let (channel_manager_to_upstream_sender, channel_manager_to_upstream_receiver) =
            unbounded();
        let (upstream_to_channel_manager_sender, upstream_to_channel_manager_receiver) =
            unbounded();
        let (channel_manager_to_sv1_server_sender, channel_manager_to_sv1_server_receiver) =
            unbounded();
        let (sv1_server_to_channel_manager_sender, sv1_server_to_channel_manager_receiver) =
            unbounded();

        debug!("Channels initialized.");

        let upstream_addresses = self
            .config
            .upstreams
            .iter()
            .map(|upstream| {
                let upstream_addr =
                    SocketAddr::new(upstream.address.parse().unwrap(), upstream.port);
                (upstream_addr, upstream.authority_pubkey)
            })
            .collect::<Vec<_>>();

        let upstream = match Upstream::new(
            &upstream_addresses,
            upstream_to_channel_manager_sender.clone(),
            channel_manager_to_upstream_receiver.clone(),
            notify_shutdown.clone(),
            shutdown_complete_tx.clone(),
        )
        .await
        {
            Ok(upstream) => {
                debug!("Upstream initialized successfully.");
                upstream
            }
            Err(e) => {
                error!("Failed to initialize upstream connection: {e:?}");
                return;
            }
        };

        let channel_manager = Arc::new(ChannelManager::new(
            channel_manager_to_upstream_sender,
            upstream_to_channel_manager_receiver,
            channel_manager_to_sv1_server_sender.clone(),
            sv1_server_to_channel_manager_receiver,
            if self.config.aggregate_channels {
                ChannelMode::Aggregated
            } else {
                ChannelMode::NonAggregated
            },
            self.wallet.clone(),
        ));

        let downstream_addr = SocketAddr::new(
            self.config.downstream_address.parse().unwrap(),
            self.config.downstream_port,
        );

        let sv1_server = Arc::new(Sv1Server::new(
            downstream_addr,
            channel_manager_to_sv1_server_receiver,
            sv1_server_to_channel_manager_sender,
            self.config.clone(),
            self.miner_tracker.clone(),
        ));

        ChannelManager::run_channel_manager_tasks(
            channel_manager.clone(),
            notify_shutdown.clone(),
            shutdown_complete_tx.clone(),
            status_sender.clone(),
            task_manager.clone(),
        )
        .await;

        if let Err(e) = upstream
            .start(
                notify_shutdown.clone(),
                shutdown_complete_tx.clone(),
                status_sender.clone(),
                task_manager.clone(),
            )
            .await
        {
            error!("Failed to start upstream listener: {e:?}");
            return;
        }

        let notify_shutdown_clone = notify_shutdown.clone();
        let shutdown_complete_tx_clone = shutdown_complete_tx.clone();
        let status_sender_clone = status_sender.clone();
        let task_manager_clone = task_manager.clone();
        task_manager.spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        info!("Ctrl+C received - initiating graceful shutdown...");
                        let _ = notify_shutdown_clone.send(ShutdownMessage::ShutdownAll);
                        break;
                    }
                    message = status_receiver.recv() => {
                        if let Ok(status) = message {
                            match status.state {
                                State::DownstreamShutdown{downstream_id,..} => {
                                    warn!("Downstream {downstream_id:?} disconnected - notifying SV1 server.");
                                    let _ = notify_shutdown_clone.send(ShutdownMessage::DownstreamShutdown(downstream_id));
                                }
                                State::Sv1ServerShutdown(_) => {
                                    warn!("SV1 Server shutdown requested - initiating full shutdown.");
                                    let _ = notify_shutdown_clone.send(ShutdownMessage::ShutdownAll);
                                    break;
                                }
                                State::ChannelManagerShutdown(_) => {
                                    warn!("Channel Manager shutdown requested - initiating full shutdown.");
                                    let _ = notify_shutdown_clone.send(ShutdownMessage::ShutdownAll);
                                    break;
                                }
                                State::UpstreamShutdown(msg) => {
                                    warn!("Upstream connection dropped: {msg:?} - attempting reconnection...");

                                    match Upstream::new(
                                        &upstream_addresses,
                                        upstream_to_channel_manager_sender.clone(),
                                        channel_manager_to_upstream_receiver.clone(),
                                        notify_shutdown_clone.clone(),
                                        shutdown_complete_tx_clone.clone(),
                                    ).await {
                                        Ok(upstream) => {
                                            if let Err(e) = upstream
                                                .start(
                                                    notify_shutdown_clone.clone(),
                                                    shutdown_complete_tx_clone.clone(),
                                                    status_sender_clone.clone(),
                                                    task_manager_clone.clone()
                                                )
                                                .await
                                            {
                                                error!("Restarted upstream failed to start: {e:?}");
                                                let _ = notify_shutdown_clone.send(ShutdownMessage::ShutdownAll);
                                                break;
                                            } else {
                                                info!("Upstream restarted successfully.");
                                                // Reset channel manager state and shutdown downstreams in one message
                                                let _ = notify_shutdown_clone.send(ShutdownMessage::UpstreamReconnectedResetAndShutdownDownstreams);
                                            }
                                        }
                                        Err(e) => {
                                            error!("Failed to reinitialize upstream after disconnect: {e:?}");
                                            let _ = notify_shutdown_clone.send(ShutdownMessage::ShutdownAll);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        if let Err(e) = Sv1Server::start(
            sv1_server,
            notify_shutdown.clone(),
            shutdown_complete_tx.clone(),
            status_sender.clone(),
            task_manager.clone(),
        )
        .await
        {
            error!("SV1 server startup failed: {e:?}");
            notify_shutdown.send(ShutdownMessage::ShutdownAll).unwrap();
        }

        drop(shutdown_complete_tx);
        info!("Waiting for shutdown completion signals from subsystems...");
        let shutdown_timeout = tokio::time::Duration::from_secs(5);
        tokio::select! {
            _ = shutdown_complete_rx.recv() => {
                info!("All subsystems reported shutdown complete.");
            }
            _ = tokio::time::sleep(shutdown_timeout) => {
                warn!("Graceful shutdown timed out after {shutdown_timeout:?} - forcing shutdown.");
                task_manager.abort_all().await;
            }
        }
        info!("Joining remaining tasks...");
        task_manager.join_all().await;
        info!("TranslatorSv2 shutdown complete.");
    }

    fn spawn_quote_sweeper(&self, task_manager: &Arc<TaskManager>, wallet: Arc<Wallet>) {
        let locking_privkey = self.config.wallet.locking_privkey.clone();

        if locking_privkey.is_none() {
            warn!("Quote sweeper running without locking_privkey; minted tokens cannot be signed");
        }

        task_manager.spawn(async move {
            let mut loop_count: u64 = 0;
            loop {
                loop_count += 1;
                info!("🕐 Quote sweeper loop #{} starting", loop_count);

                debug!("📞 About to call process_stored_quotes");
                match Self::process_stored_quotes(&wallet, locking_privkey.as_deref()).await {
                    Ok(_minted_amount) => {
                        if let Ok(balance) = wallet.total_balance().await {
                            info!("💰 Wallet balance after sweep: {} ehash", balance);
                        }
                    }
                    Err(e) => {
                        error!("❌ Quote processing failed: {}", e);
                    }
                }

                debug!("😴 Quote sweeper sleeping for 15 seconds...");
                tokio::time::sleep(Duration::from_secs(15)).await;
                debug!("⏰ Quote sweeper woke up from sleep");
            }
        });
    }

    async fn process_stored_quotes(
        wallet: &Arc<Wallet>,
        locking_privkey: Option<&str>,
    ) -> Result<u64> {
        let pending_quotes = match wallet.get_unissued_mint_quotes().await {
            Ok(quotes) => quotes,
            Err(e) => {
                error!("Failed to fetch pending quotes from wallet: {}", e);
                return Ok(0);
            }
        };

        match wallet.total_balance().await {
            Ok(balance) => {
                info!("💰 Current wallet balance: {} ehash", balance);
            }
            Err(e) => {
                error!("Failed to get wallet balance: {}", e);
            }
        }

        info!(
            "📋 Found {} pending quotes with mintable amount",
            pending_quotes.len()
        );

        if pending_quotes.is_empty() {
            return Ok(0);
        }

        let secret_key = match locking_privkey {
            Some(privkey_hex) => match hex::decode(privkey_hex) {
                Ok(privkey_bytes) => match SecretKey::from_slice(&privkey_bytes) {
                    Ok(sk) => sk,
                    Err(e) => {
                        error!("Invalid secret key format: {}", e);
                        return Ok(0);
                    }
                },
                Err(e) => {
                    error!("Failed to decode secret key hex: {}", e);
                    return Ok(0);
                }
            },
            None => {
                debug!("Skipping mint: no locking_privkey configured");
                return Ok(0);
            }
        };

        let pubkey = secret_key.public_key();
        let spending_conditions = SpendingConditions::new_p2pk(pubkey, None);

        // Store signing key in each quote's local DB record so batch_mint includes
        // NUT-20 signatures (the mint requires them because quotes are created with pubkey set).
        for mut quote in pending_quotes.iter().cloned() {
            quote.secret_key = Some(secret_key.clone());
            if let Err(e) = wallet.localstore.add_mint_quote(quote).await {
                error!("Failed to store signing key for quote: {}", e);
                return Ok(0);
            }
        }

        // Batch check quote status (1 HTTP call instead of N)
        let quote_id_strings: Vec<String> = pending_quotes.iter().map(|q| q.id.clone()).collect();
        let quote_ids: Vec<&str> = quote_id_strings.iter().map(|s| s.as_str()).collect();

        let updated_quotes = match wallet.batch_check_mint_quote_status(&quote_ids).await {
            Ok(quotes) => quotes,
            Err(e) => {
                error!("Failed to batch check quote status: {}", e);
                return Ok(0);
            }
        };

        let mintable_id_strings: Vec<String> = updated_quotes
            .iter()
            .filter(|q| q.amount_mintable() != Amount::ZERO)
            .map(|q| q.id.clone())
            .collect();

        if mintable_id_strings.is_empty() {
            warn!("😞 No mintable quotes after batch status check");
            return Ok(0);
        }

        let mintable_ids: Vec<&str> = mintable_id_strings.iter().map(|s| s.as_str()).collect();

        // Batch mint (1 HTTP call instead of N)
        let proofs = match wallet
            .batch_mint(&mintable_ids, SplitTarget::default(), Some(spending_conditions), None)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                error!("Batch mint failed: {}", e);
                return Ok(0);
            }
        };

        let total_minted: u64 = proofs.iter().map(|p| u64::from(p.amount)).sum();
        info!("Minted {} ehash from {} quotes", total_minted, mintable_ids.len());

        Ok(total_minted)
    }
}
