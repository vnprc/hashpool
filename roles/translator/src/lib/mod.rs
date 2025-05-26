use async_channel::{bounded, unbounded};
use cdk::wallet::{Wallet, MintQuote};
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::CurrencyUnit;
use futures::FutureExt;
use rand::Rng;
pub use roles_logic_sv2::utils::Mutex;
use status::Status;
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

use redis::{Commands, Connection};
use std::{thread, time::Duration};
use tokio::runtime::Handle;

#[derive(Clone, Debug)]
pub struct TranslatorSv2 {
    config: ProxyConfig,
    reconnect_wait_time: u64,
    wallet: Arc<Wallet>,
}

fn create_wallet(mint_url: String) -> Arc<Wallet> {
    // TODO add to config
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    let localstore = WalletMemoryDatabase::default();
    Arc::new(
        Wallet::new(
            &mint_url,
            CurrencyUnit::Custom(HASH_CURRENCY_UNIT.to_string()),
            Arc::new(localstore),
            &seed,
            None
        )
        .unwrap(),
    )
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
        let mint_url = extract_mint_url(&config);

        let mut rng = rand::thread_rng();
        let wait_time = rng.gen_range(0..=3000);
        Self {
            config,
            reconnect_wait_time: wait_time,
            wallet: create_wallet(mint_url),
        }
    }

    pub async fn start(self) {
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
            self.wallet.clone(),
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
        let wallet = self.wallet.clone();
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
        let wallet = self.wallet.clone();
        let redis_url = match self.redis_url() {
            Some(url) => url.to_string(),
            None => {
                tracing::warn!("No Redis URL configured; skipping proof sweeper.");
                return;
            }
        };

        task::spawn_blocking(move || {

            let mut conn = match Self::connect_to_redis(&redis_url) {
                Some(c) => c,
                None => return,
            };

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

                Self::process_quotes_batch(&wallet, &mut conn, &rt, &quotes);

                thread::sleep(Duration::from_secs(60));
            }
        });
    }

    fn redis_url(&self) -> Option<&str> {
        self.config.redis.as_ref().map(|r| r.url.as_str())
    }

    fn connect_to_redis(redis_url: &str) -> Option<Connection> {
        match redis::Client::open(redis_url).and_then(|c| c.get_connection()) {
            Ok(conn) => Some(conn),
            Err(e) => {
                tracing::error!("Redis connection error: {:?}", e);
                None
            }
        }
    }

    fn lookup_uuids_batch(quote_ids: &[String]) -> std::collections::HashMap<String, String> {
        use std::collections::HashMap;

        if quote_ids.is_empty() {
            return HashMap::new();
        }

        let share_hashes = quote_ids.join(",");
        let url = format!("http://localhost:3338/v1/mint/quote-ids/share?share_hashes={}", share_hashes);

        match ureq::get(&url).call() {
            Ok(response) => {
                match response.into_string() {
                    Ok(body) => {
                        match serde_json::from_str::<HashMap<String, String>>(&body) {
                            Ok(mapping) => mapping,
                            Err(e) => {
                                tracing::warn!("Failed to parse batch UUID response: {}", e);
                                HashMap::new()
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to read batch UUID response body: {}", e);
                        HashMap::new()
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to make batch UUID lookup request: {}", e);
                HashMap::new()
            }
        }
    }

    fn process_quotes_batch(wallet: &Arc<Wallet>, conn: &mut Connection, rt: &Handle, quotes: &[MintQuote]) {
        if quotes.is_empty() {
            return;
        }

        let quote_ids: Vec<String> = quotes.iter().map(|q| q.id.clone()).collect();
        let uuid_mapping = Self::lookup_uuids_batch(&quote_ids);

        for quote in quotes {
            if let Some(uuid) = uuid_mapping.get(&quote.id) {
                // TODO get latest keyset
                match rt.block_on(wallet.get_mining_share_proofs(uuid, &quote.id)) {
                    Ok(_proofs) => {
                        let redis_key = format!("mint:quotes:hash:{}", quote.id);
                        Self::log_success_and_cleanup(wallet, conn, rt, quote, &redis_key);
                    }
                    Err(e) => {
                        tracing::info!("Failed to mint ehash tokens for share {} error: {}", quote.id, e);
                    }
                }
            }
        }
    }

    fn log_success_and_cleanup(
        wallet: &Arc<Wallet>,
        conn: &mut Connection,
        rt: &Handle,
        quote: &MintQuote,
        redis_key: &str,
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

        if let Err(e) = conn.del::<_, ()>(redis_key) {
            tracing::warn!("Failed to delete Redis key {}: {:?}", redis_key, e);
        }

        // the people need ehash, let's give it to them
        match rt.block_on(wallet.send(cdk::Amount::from(1),None,None,&cdk::amount::SplitTarget::None, &cdk::wallet::SendKind::OnlineExact, false)) {
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
