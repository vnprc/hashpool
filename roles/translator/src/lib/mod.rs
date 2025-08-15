use async_channel::{bounded, unbounded};
use cdk::wallet::{MintConnector, MintQuote, Wallet};
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
    collections::HashMap,
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
use tokio::runtime::Handle;
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
        .to_seed_normalized("")
        .to_vec();
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
        CurrencyUnit::Custom(HASH_CURRENCY_UNIT.to_string()),
        Arc::new(localstore),
        &seed,
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
                    error!("Trying to reconnect the Upstream because of: {}", err);

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
        )
        .await
        {
            Ok(upstream) => upstream,
            Err(e) => {
                error!("Failed to create upstream: {}", e);
                return;
            }
        };
        let task_collector_init_task = task_collector.clone();
        
        // Create shared list for tracking share hashes for quote sweeping
        let share_hashes = Arc::new(Mutex::new(Vec::<String>::new()));
        let share_hashes_for_task = share_hashes.clone();
        
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
                share_hashes_for_task.clone(),
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
        
        // Only spawn proof sweeper if we have a private key for signing
        if self.config.wallet.locking_privkey.is_some() {
            info!("Spawning proof sweeper");
            self.spawn_proof_sweeper(share_hashes.clone());
        }
    }

    fn spawn_proof_sweeper(&self, _share_hashes: Arc<Mutex<Vec<String>>>) {
        let wallet = self.wallet.as_ref().unwrap().clone();
        let mint_client = self.mint_client.clone();
        let locking_pubkey = self.config.wallet.locking_pubkey.as_ref().unwrap().clone();
        let locking_privkey = self.config.wallet.locking_privkey.clone();

        task::spawn(async move {
            loop {
                // Process quotes using locking key approach
                Self::process_quotes_by_locking_key_async(&wallet, &mint_client, &locking_pubkey, locking_privkey.as_deref()).await;
                
                // the people need ehash, let's give it to them
                Self::generate_single_ehash_token_async(&wallet).await;

                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    async fn generate_single_ehash_token_async(wallet: &Arc<Wallet>) {
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
                match wallet.send(send, None).await {
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

    fn generate_single_ehash_token(wallet: &Arc<Wallet>, rt: &Handle) {
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
        
        match rt.block_on(wallet.prepare_send(cdk::Amount::from(1), options)) {
            Ok(send) => {
                match rt.block_on(wallet.send(send, None)) {
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

    fn lookup_uuids_batch(rt: &Handle, mint_client: &HttpClient, share_hashes: &[String]) -> std::collections::HashMap<String, String> {
        if share_hashes.is_empty() {
            return HashMap::new();
        }

        let quotes_shares_future = mint_client.get_quotes_shares(share_hashes.to_vec());

        match rt.block_on(quotes_shares_future) {
            Ok(response) => {
                response.quote_ids.iter().map(|(id, uuid)| (id.clone(), uuid.clone())).collect::<HashMap<String, String>>()
            },
            Err(e) => {
                tracing::error!("Failed to lookup batch UUIDs from mint: {}", e);
                HashMap::new()
            }
        }
    }

    async fn process_quotes_by_locking_key_async(wallet: &Arc<Wallet>, _mint_client: &HttpClient, locking_pubkey: &str, locking_privkey: Option<&str>) {
        // Parse the hex locking pubkey into a PublicKey
        let pubkey_bytes: Vec<u8> = match hex::decode(locking_pubkey) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("Failed to decode locking pubkey {}: {:?}", locking_pubkey, e);
                return;
            }
        };
        
        let pubkey = match cdk::nuts::PublicKey::from_slice(&pubkey_bytes) {
            Ok(pk) => pk,
            Err(e) => {
                tracing::warn!("Failed to parse locking pubkey: {:?}", e);
                return;
            }
        };

        // First, let's debug the lookup step to see if we find any quotes
        tracing::debug!("Looking up quotes for pubkey: {}", locking_pubkey);
        
        let quote_lookup_items = match wallet.lookup_mint_quotes_by_pubkeys(&[pubkey]).await {
            Ok(items) => {
                tracing::info!("Found {} quote lookup items for pubkey {}", items.len(), locking_pubkey);
                for item in &items {
                    tracing::debug!("Quote lookup item: quote={}, method={}, pubkey={:?}", 
                                  item.quote, item.method, item.pubkey);
                }
                items
            }
            Err(e) => {
                tracing::error!("Failed to lookup quotes for pubkey {}: {}", locking_pubkey, e);
                return;
            }
        };

        if quote_lookup_items.is_empty() {
            tracing::warn!("No quotes found for pubkey {} - nothing to mint", locking_pubkey);
            return;
        }

        // Parse the secret key if provided
        let secret_key = match locking_privkey {
            Some(privkey_hex) => {
                match hex::decode(privkey_hex) {
                    Ok(privkey_bytes) => {
                        match cdk::nuts::SecretKey::from_slice(&privkey_bytes) {
                            Ok(sk) => Some(sk),
                            Err(e) => {
                                tracing::warn!("Failed to parse locking privkey: {:?}", e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to decode locking privkey hex: {:?}", e);
                        None
                    }
                }
            }
            None => None,
        };

        // Check if we have any keysets available before attempting to mint
        match wallet.get_mint_keysets().await {
            Ok(keysets) => {
                if keysets.is_empty() {
                    tracing::warn!("No keysets available in wallet - skipping mint attempt");
                    return;
                }
                tracing::debug!("Wallet has {} keysets available", keysets.len());
            }
            Err(e) => {
                tracing::error!("Failed to get keysets: {}", e);
                return;
            }
        }

        // Now use the convenience function that does everything for us:
        // 1. Query mint for quote IDs by pubkey (using our locking key lookup API)  
        // 2. Retrieve those quotes from the mint
        // 3. Mint them all
        tracing::debug!("Attempting to mint tokens for {} quotes", quote_lookup_items.len());
        match wallet.mint_tokens_for_pubkey(pubkey, secret_key).await {
            Ok(proofs) => {
                tracing::info!("Successfully minted {} ehash tokens for pubkey {}", proofs.len(), locking_pubkey);
                if proofs.is_empty() {
                    tracing::warn!("mint_tokens_for_pubkey returned 0 proofs despite {} quotes being found - this suggests the quotes may not be paid or mintable", quote_lookup_items.len());
                }
            }
            Err(e) => {
                tracing::error!("Failed to mint ehash tokens for pubkey {}: {}", locking_pubkey, e);
            }
        }
    }



    fn lookup_uuid_for_quote(_rt: &Handle, _mint_client: &HttpClient, quote_id: &str) -> Option<String> {
        // TODO: Implement proper UUID lookup via HTTP API
        // For now, use the quote_id directly as UUID since the new API should provide UUIDs
        Some(quote_id.to_string())
    }

    fn process_quotes_batch(wallet: &Arc<Wallet>, rt: &Handle, quotes: &[MintQuote], mint_client: &HttpClient) {
        if quotes.is_empty() {
            return;
        }

        let quote_ids: Vec<String> = quotes.iter().map(|q| q.id.clone()).collect();
        let uuid_mapping = Self::lookup_uuids_batch(rt, mint_client, &quote_ids);
        for quote in quotes {
            if let Some(uuid) = uuid_mapping.get(&quote.id) {
                // Update the quote's ID to use the mint UUID instead of header hash
                let mut updated_quote = quote.clone();
                updated_quote.id = uuid.clone();
                
                // TODO: use share hash as mint db mint_quote primary key, requires NUT-20 support
                // Risk: Temporary duplicate quotes/secrets if delete fails, but no data loss
                if let Err(e) = rt.block_on(wallet.localstore.add_mint_quote(updated_quote)) {
                    tracing::error!("Failed to add updated quote {} with mint UUID {}: {}", quote.id, uuid, e);
                    continue;
                }
                
                // Update premint secrets to use mint UUID key (same add-first pattern)
                // TODO update cdk to use share hash as the primary field for mining share
                if let Ok(Some(secrets)) = rt.block_on(wallet.localstore.get_premint_secrets(&quote.id)) {
                    if let Err(e) = rt.block_on(wallet.localstore.add_premint_secrets(uuid, secrets)) {
                        tracing::error!("Failed to add premint secrets for mint UUID {}: {}", uuid, e);
                        continue;
                    }
                }
                
                tracing::debug!("Successfully updated quote and secrets {} to use mint UUID {}", quote.id, uuid);
                
                // Remove old quote and secrets only after successful adds
                if let Err(e) = rt.block_on(wallet.localstore.remove_mint_quote(&quote.id)) {
                    tracing::warn!("Failed to remove old quote {} (new UUID {} exists): {}", quote.id, uuid, e);
                }
                if let Err(e) = rt.block_on(wallet.localstore.remove_premint_secrets(&quote.id)) {
                    tracing::warn!("Failed to remove old premint secrets {} (new UUID {} exists): {}", quote.id, uuid, e);
                }
                
                // TODO get latest keyset
                match rt.block_on(wallet.mint_mining_share(uuid)) {
                    Ok(_proofs) => {
                        Self::log_success(wallet, rt, quote);
                    }
                    Err(e) => {
                        tracing::error!("Failed to mint ehash tokens for share {} error: {}", quote.id, e);
                    }
                }
            }
        }
    }

    fn log_success(
        wallet: &Arc<Wallet>,
        rt: &Handle,
        quote: &MintQuote,
    ) {
        match rt.block_on(wallet.total_balance()) {
            Ok(balance) => info!(
                "Successfully minted ehash tokens for share {} with amount {}. Total wallet balance: {}",
                quote.id, quote.amount.map_or_else(|| "<missing>".to_string(), |a| a.to_string()), balance
            ),
            Err(e) => info!(
                "Minted ehash tokens for share {} with amount {}, but failed to get total balance: {:?}",
                quote.id, quote.amount.map_or_else(|| "<missing>".to_string(), |a| a.to_string()), e
            ),
        }
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
