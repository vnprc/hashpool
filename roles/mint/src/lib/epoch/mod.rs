//! Mining epoch mechanics: per-epoch currency units named by block height,
//! opened and closed by the mint. See docs/EPOCH_DESIGN.md.

pub mod naming;
pub mod store;

use anyhow::{anyhow, Context, Result};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use cdk::{
    cdk_payment::MintPayment,
    mint::{Mint, MintMeltLimits},
    nuts::{CurrencyUnit, PaymentMethod},
};
use cdk_ehash::EhashPaymentProcessor;
use rpc_sv2::mini_rpc_client::{Auth, MiniRpcClient};
use std::sync::{Arc, RwLock};
use store::{EpochRecord, EpochSource, EpochState, EpochStore};
use tracing::{info, warn};

/// Number of keys per epoch keyset (amounts 2^0 .. 2^(NUM_KEYS-1)).
const NUM_KEYS: u32 = 64;

fn ehash_method() -> PaymentMethod {
    PaymentMethod::Custom("ehash".to_string())
}

#[derive(Debug, Clone)]
pub struct EpochSettings {
    /// Pool identity: compressed secp256k1 pubkey, lowercase hex. Namespaces
    /// every epoch unit (`hash_<pool>_<height>`).
    pub pool_pubkey: String,
    pub store_path: std::path::PathBuf,
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_pass: String,
    /// Loopback listener for the manual rotation lever.
    pub admin_listen: String,
}

pub struct EpochManager {
    mint: Arc<Mint>,
    /// Serializes rotations; also guards the store file.
    store: tokio::sync::Mutex<EpochStore>,
    current: RwLock<EpochRecord>,
    pool_pubkey: String,
    amounts: Vec<u64>,
    rpc: MiniRpcClient,
}

impl EpochManager {
    /// Load the persisted current epoch, or open the genesis epoch at the
    /// current chain height. Blocks (with retry) until bitcoind answers.
    pub async fn load_or_genesis(mint: Arc<Mint>, settings: EpochSettings) -> Result<Arc<Self>> {
        let pool_pubkey = naming::validate_pool_pubkey(&settings.pool_pubkey)?;
        let uri: hyper::Uri = settings
            .rpc_url
            .parse()
            .with_context(|| format!("invalid bitcoin_rpc url {}", settings.rpc_url))?;
        let rpc = MiniRpcClient::new(uri, Auth::new(settings.rpc_user, settings.rpc_pass));
        let store = EpochStore::load(&settings.store_path)?;
        let amounts: Vec<u64> = (0..NUM_KEYS).map(|i| 2_u64.pow(i)).collect();

        if let Some(current) = store.current().cloned() {
            let manager = Arc::new(Self {
                mint,
                store: tokio::sync::Mutex::new(store),
                current: RwLock::new(current.clone()),
                pool_pubkey,
                amounts,
                rpc,
            });
            // The processor map is in-memory only: a restart must re-register
            // the resumed epoch's quote-creation entry or every share is
            // rejected until the next rotation.
            let unit = CurrencyUnit::Custom(current.unit.clone().into());
            manager.register_unit(&unit).await?;
            info!(
                unit = %current.unit,
                height = current.height,
                "resuming persisted epoch"
            );
            return Ok(manager);
        }

        let height = block_count_with_retry(&rpc, 30, std::time::Duration::from_secs(2))
            .await
            .context("genesis needs the chain height; is bitcoind reachable?")?;
        // Placeholder current; replaced by open_epoch below.
        let placeholder = EpochRecord {
            height: 0,
            unit: String::new(),
            keyset_id: String::new(),
            block_hash: None,
            reward_sats: None,
            state: EpochState::Final,
            source: EpochSource::Genesis,
            opened_at: 0,
        };
        let manager = Arc::new(Self {
            mint,
            store: tokio::sync::Mutex::new(store),
            current: RwLock::new(placeholder),
            pool_pubkey,
            amounts,
            rpc,
        });
        let record = manager
            .open_epoch(height, None, None, EpochSource::Genesis)
            .await?;
        info!(unit = %record.unit, height, "genesis epoch opened");
        Ok(manager)
    }

    /// Single-read snapshot for the quote path: the unit new quotes are
    /// stamped with, and whether that epoch's boundary is final (final →
    /// quotes pay at creation; provisional → they wait for finality). One
    /// read, so a rotation between "which unit" and "pay now?" cannot tear.
    pub fn current_snapshot(&self) -> (CurrencyUnit, bool) {
        let current = self.current.read().expect("epoch lock poisoned");
        (
            CurrencyUnit::Custom(current.unit.clone().into()),
            current.state == EpochState::Final,
        )
    }

    pub async fn chain_height(&self) -> Result<u64> {
        rpc_block_count(&self.rpc).await
    }

    /// Idempotent quote-creation registration for a unit: an already-present
    /// (unit, method) pair is success (restart resume and retry paths).
    async fn register_unit(&self, unit: &CurrencyUnit) -> Result<()> {
        if self
            .mint
            .get_payment_processor(unit.clone(), ehash_method())
            .is_ok()
        {
            return Ok(());
        }
        let processor = Arc::new(EhashPaymentProcessor::new(unit.clone()))
            as Arc<dyn MintPayment<Err = cdk::cdk_payment::Error> + Send + Sync>;
        self.mint
            .register_payment_processor(
                unit.clone(),
                ehash_method(),
                MintMeltLimits::new(1, u64::MAX),
                processor,
            )
            .await
            .map_err(|e| anyhow!("registering {unit} for quoting failed: {e}"))
    }

    /// Close the current epoch and open a new one. Genesis/manual epochs open
    /// Final; the previous epoch's quote-creation entry is retired once the
    /// successor is final (old quotes still mint; no new quotes).
    pub async fn open_epoch(
        &self,
        height: u64,
        block_hash: Option<String>,
        reward_sats: Option<u64>,
        source: EpochSource,
    ) -> Result<EpochRecord> {
        const MAX_NAME_ATTEMPTS: u32 = 32;

        let mut store = self.store.lock().await;
        let previous = store.current().cloned();
        let mut suffix = store.count_at_height(height);
        let mut attempts = 0u32;

        let (unit, keyset_id) = loop {
            attempts += 1;
            if attempts > MAX_NAME_ATTEMPTS {
                return Err(anyhow!(
                    "no usable unit name for height {height} after {MAX_NAME_ATTEMPTS} attempts"
                ));
            }
            let name = naming::unit_name(&self.pool_pubkey, height, suffix);
            if store.unit_taken(&name) {
                suffix += 1;
                continue;
            }
            let unit = CurrencyUnit::Custom(name.clone().into());
            match self
                .mint
                .rotate_keyset(unit.clone(), self.amounts.clone(), 0, true, None)
                .await
            {
                Ok(keyset_info) => break (unit, keyset_info.id.to_string()),
                Err(cdk::Error::UnitStringCollision(_)) => {
                    warn!(unit = %name, "unit derivation collision; trying suffix {}", suffix + 1);
                    suffix += 1;
                    continue;
                }
                Err(e) => return Err(anyhow!("keyset creation for {name} failed: {e}")),
            }
        };

        self.register_unit(&unit).await?;

        // Persist and swap the current epoch BEFORE retiring the previous one:
        // the quote path must never observe a deregistered unit as current, and
        // a persist failure must not leave the mint without a quotable unit.
        let record = EpochRecord {
            height,
            unit: unit.to_string(),
            keyset_id,
            block_hash,
            reward_sats,
            state: EpochState::Final,
            source,
            opened_at: store::unix_now(),
        };
        store.append(record.clone())?;
        *self.current.write().expect("epoch lock poisoned") = record.clone();

        // Retire the previous epoch's quote-creation entry last. Failure is
        // loud but non-fatal: a lingering entry is benign, a failed rotation
        // is not.
        if let Some(prev) = &previous {
            let prev_unit = CurrencyUnit::Custom(prev.unit.clone().into());
            if let Err(e) = self
                .mint
                .deregister_payment_processor(prev_unit, ehash_method())
                .await
            {
                warn!(unit = %prev.unit, "failed to retire previous epoch entry: {e}");
            }
        }

        info!(unit = %record.unit, height, ?source, "epoch opened");
        Ok(record)
    }
}

/// One getblockcount attempt with a hard timeout, so a hung (not refusing)
/// RPC endpoint cannot block mint startup forever.
async fn rpc_block_count(rpc: &MiniRpcClient) -> Result<u64> {
    tokio::time::timeout(std::time::Duration::from_secs(10), rpc.get_block_count())
        .await
        .map_err(|_| anyhow!("getblockcount timed out after 10s"))?
        .map_err(|e| anyhow!("getblockcount failed: {e:?}"))
}

async fn block_count_with_retry(
    rpc: &MiniRpcClient,
    attempts: u32,
    delay: std::time::Duration,
) -> Result<u64> {
    let mut last_err = None;
    for _ in 0..attempts {
        match rpc_block_count(rpc).await {
            Ok(h) => return Ok(h),
            Err(e) => {
                last_err = Some(e.to_string());
                tokio::time::sleep(delay).await;
            }
        }
    }
    Err(anyhow!(
        "bitcoind RPC unreachable after {attempts} attempts: {}",
        last_err.unwrap_or_default()
    ))
}

/// Loopback admin surface: the manual rotation lever (`just rotate-epoch`).
pub fn admin_router(manager: Arc<EpochManager>) -> Router {
    Router::new()
        .route("/rotate-epoch", post(rotate_epoch_handler))
        .with_state(manager)
}

async fn rotate_epoch_handler(State(manager): State<Arc<EpochManager>>) -> impl IntoResponse {
    let height = match manager.chain_height().await {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    };
    match manager
        .open_epoch(height, None, None, EpochSource::Manual)
        .await
    {
        Ok(record) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "unit": record.unit,
                "keyset_id": record.keyset_id,
                "height": record.height,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}
