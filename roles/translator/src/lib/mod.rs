use async_channel::{bounded, unbounded};
use cdk::wallet::{MintConnector, MintQuote, SendKind, SendOptions, Wallet};
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

use std::{thread, time::Duration, env};
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

        self.spawn_proof_sweeper();

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
    }

    fn spawn_proof_sweeper(&self) {
        let wallet = self.wallet.as_ref().unwrap().clone();
        let mint_client = self.mint_client.clone();

        task::spawn_blocking(move || {
            let rt = Handle::current();

            loop {
                let quotes = match rt.block_on(wallet.localstore.get_mint_quotes()) {
                    Ok(q) => q,
                    Err(e) => {
                        tracing::warn!("Failed to get mint quotes: {:?}", e);
                        thread::sleep(Duration::from_secs(10));
                        continue;
                    }
                };

                Self::process_quotes_batch(&wallet, &rt, &quotes, &mint_client);

                thread::sleep(Duration::from_secs(60));
            }
        });
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

    fn process_quotes_batch(wallet: &Arc<Wallet>, rt: &Handle, quotes: &[MintQuote], mint_client: &HttpClient) {
        if quotes.is_empty() {
            return;
        }

        let quote_ids: Vec<String> = quotes.iter().map(|q| q.id.clone()).collect();
        let uuid_mapping = Self::lookup_uuids_batch(rt, mint_client, &quote_ids);

        for quote in quotes {
            if let Some(uuid) = uuid_mapping.get(&quote.id) {
                // TODO get latest keyset
                match rt.block_on(wallet.get_mining_share_proofs(uuid, &quote.id)) {
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
                quote.id, quote.amount, balance
            ),
            Err(e) => info!(
                "Minted ehash tokens for share {} with amount {}, but failed to get total balance: {:?}",
                quote.id, quote.amount, e
            ),
        }

        let options = SendOptions{
            memo: None,
            conditions: None,
            amount_split_target: SplitTarget::None,
            send_kind: SendKind::OnlineExact,
            include_fee: false,
            metadata: HashMap::new(),
            max_proofs: None,
        };
        let send = tokio::runtime::Handle::current()
            .block_on(wallet.prepare_send(cdk::Amount::from(1), options)).unwrap();

        // the people need ehash, let's give it to them
        match rt.block_on(
    wallet.send(
                send,
                None,
            )) {
            Ok(token) => info!(
                "eHash token: {}",
                token,
            ),
            Err(e) => info!(
                "Error sending ehash token {}",
                e,
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
