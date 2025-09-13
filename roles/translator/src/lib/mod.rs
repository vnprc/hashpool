use async_channel::{bounded, unbounded};
use cdk::wallet::Wallet;
use cdk::amount::SplitTarget;
use cdk_sqlite::WalletSqliteDatabase;
use cdk::nuts::CurrencyUnit;
use cdk::{HttpClient, mint_url::MintUrl};
use bip39::Mnemonic;

use futures::FutureExt;
use rand::Rng;
pub use roles_logic_sv2::utils::Mutex;
use status::Status;
use std::path::{Path, PathBuf};
use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
};

use tokio::{
    sync::broadcast,
    task::{self, AbortHandle},
};
use tracing::{debug, error, info, warn};
pub use v1::server_to_client;

use proxy_config::ProxyConfig;

use crate::status::State;

pub mod downstream_sv1;
pub mod error;
pub mod proxy;
pub mod proxy_config;
pub mod status;
pub mod upstream_sv2;
pub mod utils;

// TODO add to config
pub const HASH_CURRENCY_UNIT: &str = "HASH";

use std::{time::Duration, env};
use anyhow::{Result, Context};

#[derive(Clone, Debug)]
pub struct TranslatorSv2 {
    config: ProxyConfig,
    reconnect_wait_time: u64,
    wallet: Option<Arc<Wallet>>,
    mint_client: HttpClient,
}

fn resolve_and_prepare_db_path(config_path: &str) -> PathBuf {
    let path = Path::new(config_path);
    let full_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .expect("Failed to get current working directory")
            .join(path)
    };

    if let Some(parent) = full_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).expect("Failed to create parent directory for DB path");
        }
    }

    full_path
}

pub async fn create_wallet(
    mint_url: String,
    mnemonic: String,
    db_path: String,
) -> Result<Arc<Wallet>> {
    tracing::debug!("Parsing mnemonic...");
    let seed = Mnemonic::from_str(&mnemonic)
        .with_context(|| format!("Invalid mnemonic: '{}'", mnemonic))?
        .to_seed_normalized("");
    let seed: [u8; 64] = seed.try_into()
        .map_err(|_| anyhow::anyhow!("Seed must be exactly 64 bytes"))?;
    tracing::debug!("Seed derived.");

    let db_path = resolve_and_prepare_db_path(&db_path);
    tracing::debug!("Resolved db_path: {}", db_path.display());

    tracing::debug!("Creating localstore...");
    let localstore = WalletSqliteDatabase::new(db_path)
        .await
        .context("WalletSqliteDatabase::new failed")?;

    tracing::debug!("Creating wallet...");
    let wallet = Wallet::new(
        &mint_url,
        CurrencyUnit::Hash,
        Arc::new(localstore),
        seed,
        None,
    )
    .context("Failed to create wallet")?;
    tracing::debug!("Wallet created.");

    let balance = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(wallet.total_balance())
    });
    tracing::debug!("Wallet constructed: {:?}", balance);

    Ok(Arc::new(wallet))
}

fn extract_mint_url(config: &ProxyConfig) -> String {
    config
        .mint
        .as_ref()
        .map(|m| m.url.clone())
        .unwrap_or_else(|| panic!("No Mint URL configured; cannot create wallet."))
}

impl TranslatorSv2 {
    pub fn new(config: ProxyConfig) -> Self {
        let mut rng = rand::thread_rng();
        let mint_url = extract_mint_url(&config);
        let wait_time = rng.gen_range(0..=3000);
        let mint_client = HttpClient::new(MintUrl::from_str(&mint_url).unwrap(), None);

        Self {
            config: config.clone(),
            reconnect_wait_time: wait_time,
            wallet: None,
            mint_client: mint_client,
        }
    }

    pub async fn start(mut self) {
        // Initialize and validate wallet config
        self.config.wallet.initialize()
            .expect("Failed to initialize wallet config");
        
        let config = &self.config;

        let wallet = create_wallet(
            extract_mint_url(&self.config),
            config.wallet.mnemonic.clone(),
            config.wallet.db_path.clone(),
        )
        .await
        .expect("Failed to create wallet");

        if let Some(mint_cfg) = &config.mint {
            let mint_url = MintUrl::from_str(&mint_cfg.url)
                .expect("Invalid mint URL");

            wallet
                .localstore
                .add_mint(mint_url, None)
                .await
                .expect("Failed to add mint to localstore");
        }

        self.wallet = Some(wallet);

        let (tx_status, rx_status) = unbounded();

        let target = Arc::new(Mutex::new(vec![0; 32]));

        // Sender/Receiver to send SV1 `mining.notify` message from the `Bridge` to the `Downstream`
        let (tx_sv1_notify, _rx_sv1_notify): (
            broadcast::Sender<server_to_client::Notify>,
            broadcast::Receiver<server_to_client::Notify>,
        ) = broadcast::channel(10);

        let task_collector: Arc<Mutex<Vec<(AbortHandle, String)>>> =
            Arc::new(Mutex::new(Vec::new()));

        self.internal_start(
            tx_sv1_notify.clone(),
            target.clone(),
            tx_status.clone(),
            task_collector.clone(),
        )
        .await;

        debug!("Starting up signal listener");
        let task_collector_ = task_collector.clone();

        debug!("Starting up status listener");
        let wait_time = self.reconnect_wait_time;


        // Check all tasks if is_finished() is true, if so exit
        loop {
            let task_status = tokio::select! {
                task_status = rx_status.recv().fuse() => task_status,
                interrupt_signal = tokio::signal::ctrl_c().fuse() => {
                    match interrupt_signal {
                        Ok(()) => {
                            info!("Interrupt received");
                        },
                        Err(err) => {
                            error!("Unable to listen for interrupt signal: {}", err);
                            // we also shut down in case of error
                        },
                    }
                    break;
                }
            };
            let task_status: Status = task_status.unwrap();

            match task_status.state {
                // Should only be sent by the downstream listener
                State::DownstreamShutdown(err) => {
                    error!("SHUTDOWN from: {}", err);
                    break;
                }
                State::BridgeShutdown(err) => {
                    error!("SHUTDOWN from: {}", err);
                    break;
                }
                State::UpstreamShutdown(err) => {
                    error!("SHUTDOWN from: {}", err);
                    break;
                }
                State::UpstreamTryReconnect(err) => {
                    error!("SHUTDOWN from: {}", err);

                    // wait a random amount of time between 0 and 3000ms
                    // if all the downstreams try to reconnect at the same time, the upstream may
                    // fail
                    tokio::time::sleep(std::time::Duration::from_millis(wait_time)).await;

                    // kill al the tasks
                    let task_collector_aborting = task_collector_.clone();
                    kill_tasks(task_collector_aborting.clone());

                    warn!("Trying reconnecting to upstream");
                    self.internal_start(
                        tx_sv1_notify.clone(),
                        target.clone(),
                        tx_status.clone(),
                        task_collector_.clone(),
                    )
                    .await;
                }
                State::Healthy(msg) => {
                    info!("HEALTHY message: {}", msg);
                }
            }
        }
    }

    async fn internal_start(
        &self,
        tx_sv1_notify: broadcast::Sender<server_to_client::Notify<'static>>,
        target: Arc<Mutex<Vec<u8>>>,
        tx_status: async_channel::Sender<Status<'static>>,
        task_collector: Arc<Mutex<Vec<(AbortHandle, String)>>>,
    ) {
        let wallet = self.wallet.as_ref().unwrap().clone();

        let proxy_config = self.config.clone();
        
        
        // Sender/Receiver to send a SV2 `SubmitSharesExtended` from the `Bridge` to the `Upstream`
        // (Sender<SubmitSharesExtended<'static>>, Receiver<SubmitSharesExtended<'static>>)
        let (tx_sv2_submit_shares_ext, rx_sv2_submit_shares_ext) = bounded(10);

        // `tx_sv1_bridge` sender is used by `Downstream` to send a `DownstreamMessages` message to
        // `Bridge` via the `rx_sv1_downstream` receiver
        // (Sender<downstream_sv1::DownstreamMessages>,
        // Receiver<downstream_sv1::DownstreamMessages>)
        let (tx_sv1_bridge, rx_sv1_downstream) = unbounded();

        // Sender/Receiver to send a SV2 `NewExtendedMiningJob` message from the `Upstream` to the
        // `Bridge`
        // (Sender<NewExtendedMiningJob<'static>>, Receiver<NewExtendedMiningJob<'static>>)
        let (tx_sv2_new_ext_mining_job, rx_sv2_new_ext_mining_job) = bounded(10);

        // Sender/Receiver to send a new extranonce from the `Upstream` to this `main` function to
        // be passed to the `Downstream` upon a Downstream role connection
        // (Sender<ExtendedExtranonce>, Receiver<ExtendedExtranonce>)
        let (tx_sv2_extranonce, rx_sv2_extranonce) = bounded(1);

        // Sender/Receiver to send a SV2 `SetNewPrevHash` message from the `Upstream` to the
        // `Bridge` (Sender<SetNewPrevHash<'static>>, Receiver<SetNewPrevHash<'static>>)
        let (tx_sv2_set_new_prev_hash, rx_sv2_set_new_prev_hash) = bounded(10);

        // Format `Upstream` connection address
        let upstream_addr = SocketAddr::new(
            IpAddr::from_str(&proxy_config.upstream_address)
                .expect("Failed to parse upstream address!"),
            proxy_config.upstream_port,
        );

        let diff_config = Arc::new(Mutex::new(proxy_config.upstream_difficulty_config.clone()));
        let task_collector_upstream = task_collector.clone();
        
        // Create broadcast channel for keyset updates
        let (keyset_sender, keyset_receiver) = broadcast::channel(16);
        
        // Instantiate a new `Upstream` (SV2 Pool)
        let upstream = match upstream_sv2::Upstream::new(
            upstream_addr,
            proxy_config.upstream_authority_pubkey,
            rx_sv2_submit_shares_ext,
            tx_sv2_set_new_prev_hash,
            tx_sv2_new_ext_mining_job,
            proxy_config.min_extranonce2_size,
            tx_sv2_extranonce,
            status::Sender::Upstream(tx_status.clone()),
            target.clone(),
            diff_config.clone(),
            task_collector_upstream,
            wallet.clone(),
            keyset_sender,
        )
        .await
        {
            Ok(upstream) => upstream,
            Err(e) => {
                error!("Failed to create upstream: {}", e);
                return;
            }
        };
        
        // Only spawn proof sweeper if we have a private key for signing
        if self.config.wallet.locking_privkey.is_some() {
            info!("Spawning proof sweeper");
            self.spawn_proof_sweeper(upstream.clone());
        }
        
        let task_collector_init_task = task_collector.clone();
        
        
        // Spawn a task to do all of this init work so that the main thread
        // can listen for signals and failures on the status channel. This
        // allows for the tproxy to fail gracefully if any of these init tasks
        //fail
        let task = task::spawn(async move {
            // Connect to the SV2 Upstream role
            match upstream_sv2::Upstream::connect(
                upstream.clone(),
                proxy_config.min_supported_version,
                proxy_config.max_supported_version,
            )
            .await
            {
                Ok(_) => info!("Connected to Upstream!"),
                Err(e) => {
                    error!("Failed to connect to Upstream EXITING! : {}", e);
                    return;
                }
            }

            // Start receiving messages from the SV2 Upstream role
            if let Err(e) = upstream_sv2::Upstream::parse_incoming(upstream.clone()) {
                error!("failed to create sv2 parser: {}", e);
                return;
            }

            debug!("Finished starting upstream listener");
            // Start task handler to receive submits from the SV1 Downstream role once it connects
            if let Err(e) = upstream_sv2::Upstream::handle_submit(upstream.clone()) {
                error!("Failed to create submit handler: {}", e);
                return;
            }

            // Receive the extranonce information from the Upstream role to send to the Downstream
            // role once it connects also used to initialize the bridge
            let (extended_extranonce, up_id) = rx_sv2_extranonce.recv().await.unwrap();
            loop {
                let target: [u8; 32] = target.safe_lock(|t| t.clone()).unwrap().try_into().unwrap();
                if target != [0; 32] {
                    break;
                };
                async_std::task::sleep(std::time::Duration::from_millis(100)).await;
            }

            let task_collector_bridge = task_collector_init_task.clone();
            
            // Instantiate a new `Bridge` and begins handling incoming messages
            let b = proxy::Bridge::new(
                rx_sv1_downstream,
                tx_sv2_submit_shares_ext,
                rx_sv2_set_new_prev_hash,
                rx_sv2_new_ext_mining_job,
                tx_sv1_notify.clone(),
                status::Sender::Bridge(tx_status.clone()),
                extended_extranonce,
                target,
                up_id,
                task_collector_bridge,
                wallet,
                // Safe to unwrap: initialize() ensures locking_pubkey is set
                proxy_config.wallet.locking_pubkey.as_ref().unwrap().clone(),
                keyset_receiver,
            );
            proxy::Bridge::start(b.clone());

            // Format `Downstream` connection address
            let downstream_addr = SocketAddr::new(
                IpAddr::from_str(&proxy_config.downstream_address).unwrap(),
                proxy_config.downstream_port,
            );

            let task_collector_downstream = task_collector_init_task.clone();
            // Accept connections from one or more SV1 Downstream roles (SV1 Mining Devices)
            downstream_sv1::Downstream::accept_connections(
                downstream_addr,
                tx_sv1_bridge,
                tx_sv1_notify,
                status::Sender::DownstreamListener(tx_status.clone()),
                b,
                proxy_config.downstream_difficulty_config,
                diff_config,
                task_collector_downstream,
            );
            
        }); // End of init task
        let _ =
            task_collector.safe_lock(|t| t.push((task.abort_handle(), "init task".to_string())));
        
        // Note: spawn_proof_sweeper moved to after upstream is created
    }

    fn spawn_proof_sweeper(&self, upstream: Arc<roles_logic_sv2::utils::Mutex<upstream_sv2::Upstream>>) {
        let wallet = self.wallet.as_ref().unwrap().clone();
        let locking_privkey = self.config.wallet.locking_privkey.clone();

        task::spawn(async move {
            let mut loop_count = 0;
            loop {
                loop_count += 1;
                tracing::info!("🕐 Proof sweeper loop #{} starting", loop_count);
                
                // Process quotes using stored quotes from extension messages
                tracing::debug!("📞 About to call process_stored_quotes");
                match Self::process_stored_quotes(&wallet, upstream.clone(), locking_privkey.as_deref()).await {
                    Ok(minted_amount) => {
                        tracing::info!("✅ process_stored_quotes returned: minted_amount = {}", minted_amount);
                        
                        // the people need ehash, let's give it to them (only if we minted some tokens)
                        if minted_amount > 0 {
                            tracing::info!("🎁 Generating single ehash token since we minted {} tokens", minted_amount);
                            Self::generate_single_ehash_token(&wallet).await;
                        } else {
                            tracing::debug!("⏭️ Skipping ehash token generation - no tokens were minted");
                        }
                    }
                    Err(e) => {
                        tracing::error!("❌ Quote processing failed: {}", e);
                        // Continue the loop - don't generate tokens on error
                    }
                }

                tracing::debug!("😴 Proof sweeper sleeping for 60 seconds...");
                tokio::time::sleep(Duration::from_secs(60)).await;
                tracing::debug!("⏰ Proof sweeper woke up from sleep");
            }
        });
    }

    async fn generate_single_ehash_token(wallet: &Arc<Wallet>) {
        tracing::debug!("Creating single ehash token for distribution");
        
        let options = cdk::wallet::SendOptions {
            memo: None,
            conditions: None,
            amount_split_target: SplitTarget::None,
            send_kind: cdk::wallet::SendKind::OnlineExact,
            include_fee: false,
            metadata: std::collections::HashMap::new(),
            max_proofs: None,
        };
        
        match wallet.prepare_send(cdk::Amount::from(1), options).await {
            Ok(send) => {
                match send.confirm(None).await {
                    Ok(token) => {
                        tracing::info!("Generated ehash token: {}", token);
                    },
                    Err(e) => {
                        tracing::error!("Failed to generate ehash token: {}", e);
                    }
                }
            },
            Err(e) => {
                tracing::error!("Failed to prepare send for ehash token: {}", e);
            }
        }
    }

    async fn process_stored_quotes(
        wallet: &Arc<Wallet>, 
        upstream: Arc<roles_logic_sv2::utils::Mutex<upstream_sv2::Upstream>>,
        locking_privkey: Option<&str>
    ) -> Result<u64> {
        tracing::info!("🔄 Starting process_stored_quotes sweep");
        
        // Get the quote tracker from the upstream
        tracing::debug!("📡 Attempting to access quote tracker from upstream");
        let quote_tracker = match upstream.safe_lock(|u| u.quote_tracker.clone()) {
            Ok(tracker) => {
                tracing::debug!("✅ Successfully got quote tracker from upstream");
                tracker
            },
            Err(e) => {
                tracing::error!("❌ Failed to access quote tracker: {}", e);
                return Ok(0);
            }
        };

        // Get all stored quotes from the tracker
        tracing::debug!("🔒 Acquiring lock on quotes HashMap");
        let quotes = quote_tracker.quotes.lock().await;
        let quote_count = quotes.len();
        let quote_ids: Vec<String> = quotes.values().cloned().collect();
        tracing::info!("📊 Found {} quotes in tracker HashMap", quote_count);
        
        // Release the lock early to avoid holding it during minting
        drop(quotes);

        if quote_ids.is_empty() {
            tracing::debug!("📭 No quotes found in tracker, returning 0");
            return Ok(0);
        }

        let mut total_minted = 0u64;
        
        for (index, quote_id) in quote_ids.iter().enumerate() {
            tracing::debug!("🎫 Processing quote {}/{}: {}", index + 1, quote_ids.len(), quote_id);
            
            // First, fetch quote details from the mint and add to wallet
            match Self::fetch_and_add_quote_to_wallet(wallet, quote_id).await {
                Ok(_) => {
                    tracing::debug!("📥 Successfully added quote {} to wallet", quote_id);
                    
                    // Get the quote details we just fetched
                    match wallet.mint_quote_state_mining_share(quote_id).await {
                        Ok(quote_response) => {
                            let amount = quote_response.amount.unwrap_or(cdk::Amount::ZERO);
                            let keyset_id = quote_response.keyset_id;
                            
                            // Parse the secret key from config for NUT-20 signing
                            let secret_key = match locking_privkey {
                                Some(privkey_hex) => {
                                    match hex::decode(privkey_hex) {
                                        Ok(privkey_bytes) => {
                                            match cdk::nuts::SecretKey::from_slice(&privkey_bytes) {
                                                Ok(sk) => sk,
                                                Err(e) => {
                                                    tracing::error!("Invalid secret key format: {}", e);
                                                    continue; // Skip this quote
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!("Failed to decode secret key hex: {}", e);
                                            continue; // Skip this quote
                                        }
                                    }
                                }
                                None => {
                                    tracing::error!("Secret key is required for mining share minting (NUT-20)");
                                    continue; // Skip this quote
                                }
                            };
                            
                            // Now attempt to mint the quote with correct parameters
                            match wallet.mint_mining_share(quote_id, amount, keyset_id, secret_key).await {
                                Ok(proofs) => {
                                    let amount: u64 = proofs.iter().map(|p| u64::from(p.amount)).sum();
                                    total_minted += amount;
                                    tracing::info!("✅ Successfully minted {} ehash from quote {}", amount, quote_id);
                                    
                                    // Remove the successfully minted quote from the tracker
                                    let mut quotes = quote_tracker.quotes.lock().await;
                                    // Find and remove the key that corresponds to this quote_id
                                    let key_to_remove = quotes.iter()
                                        .find(|(_, v)| **v == *quote_id)
                                        .map(|(k, _)| k.clone());
                                    
                                    if let Some(key) = key_to_remove {
                                        quotes.remove(&key);
                                        tracing::debug!("🗑️ Removed successfully minted quote {} from tracker", quote_id);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("⚠️ Failed to mint quote {}: {}", quote_id, e);
                                    // Continue processing other quotes
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("⚠️ Failed to get quote details for {}: {}", quote_id, e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("⚠️ Failed to fetch quote {} details: {}", quote_id, e);
                    // Continue processing other quotes
                }
            }
        }

        if total_minted > 0 {
            tracing::info!("🎉 Total minted from {} quotes: {} ehash", quote_ids.len(), total_minted);
        } else {
            tracing::warn!("😞 No tokens were minted from any quotes");
        }

        tracing::info!("🏁 process_stored_quotes finished");
        Ok(total_minted)
    }

    /// Fetches quote from mint and adds to wallet's local store
    async fn fetch_and_add_quote_to_wallet(wallet: &Arc<Wallet>, quote_id: &str) -> Result<()> {
        tracing::debug!("🔍 Fetching quote {} from mint", quote_id);
        
        // Use wallet's mining share specific quote state function
        let quote = wallet.mint_quote_state_mining_share(quote_id).await
            .with_context(|| format!("Failed to fetch quote {} from mint", quote_id))?;
            
        tracing::debug!("💾 Quote {} fetched and added to wallet (state: {:?})", quote_id, quote.state);
        Ok(())
    }

}

fn kill_tasks(task_collector: Arc<Mutex<Vec<(AbortHandle, String)>>>) {
    let _ = task_collector.safe_lock(|t| {
        while let Some(handle) = t.pop() {
            handle.0.abort();
            warn!("Killed task: {:?}", handle.1);
        }
    });
}
