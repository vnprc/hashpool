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
use std::collections::HashSet;
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
            let non_dissolved = store.non_dissolved();
            let outstanding = outstanding_quote_units(&mint).await?;

            // Must stay quotable across a restart (EPOCH_DESIGN.md, "Dissolve").
            let chain = resumable_chain(&non_dissolved);

            // EPOCH_DESIGN.md, `only_mintable`.
            let to_restore: Vec<EpochRecord> = non_dissolved
                .iter()
                .filter(|r| outstanding.contains(&r.unit) || chain.contains(&r.unit))
                .cloned()
                .collect();

            let manager = Arc::new(Self {
                mint,
                store: tokio::sync::Mutex::new(store),
                current: RwLock::new(current.clone()),
                pool_pubkey,
                amounts,
                rpc,
            });

            // Register before retire leaves a closed epoch briefly quotable; safe only
            // because this runs before the mint's listeners bind. Moving it later breaks that.
            for record in &to_restore {
                let unit = CurrencyUnit::Custom(record.unit.clone().into());
                manager.register_unit(&unit).await?;
            }
            // Every non-chain record, not just the register set: one left out may still
            // carry a quote-creation gate persisted by an earlier process.
            let retire_mint = manager.mint.clone();
            retire_all_except(&non_dissolved, &chain, move |unit| {
                let mint = retire_mint.clone();
                async move {
                    let currency_unit = CurrencyUnit::Custom(unit.into());
                    mint.retire_payment_processor(currency_unit, ehash_method())
                        .await
                        .map_err(|e| anyhow!("{e}"))
                }
            })
            .await?;

            info!(
                unit = %current.unit,
                height = current.height,
                restored_epochs = to_restore.len(),
                "resuming persisted epoch"
            );
            return Ok(manager);
        }

        let height = block_count_with_retry(&rpc, 30, std::time::Duration::from_secs(2))
            .await
            .context("genesis needs the chain height; is bitcoind reachable?")?;
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

        // Retire (not deregister) the previous epoch's quote-creation entry
        // last: deregister also drops the processor map entry, stranding its
        // paid-but-unissued quotes. Failure is loud but non-fatal.
        if let Some(prev) = &previous {
            let prev_unit = CurrencyUnit::Custom(prev.unit.clone().into());
            if let Err(e) = self
                .mint
                .retire_payment_processor(prev_unit, ehash_method())
                .await
            {
                warn!(unit = %prev.unit, "failed to retire previous epoch entry: {e}");
            }
        }

        info!(unit = %record.unit, height, ?source, "epoch opened");
        Ok(record)
    }
}

/// Retires every record whose unit is not in `keep`. All selected
/// retirements must succeed before startup continues.
async fn retire_all_except<F, Fut>(
    records: &[EpochRecord],
    keep: &HashSet<String>,
    retire: F,
) -> Result<()>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    for record in records {
        if keep.contains(&record.unit) {
            continue;
        }
        retire(record.unit.clone())
            .await
            .with_context(|| format!("failed to retire closed epoch {} on resume", record.unit))?;
    }
    Ok(())
}

/// Every epoch a cascade of dissolves could hand the current role to.
/// Requires `non_dissolved` oldest first, current last.
fn resumable_chain(non_dissolved: &[EpochRecord]) -> HashSet<String> {
    let mut chain = HashSet::new();
    for record in non_dissolved.iter().rev() {
        chain.insert(record.unit.clone());
        if record.state != EpochState::Provisional {
            break;
        }
    }
    chain
}

/// Units with a quote where `amount_paid > amount_issued` (EPOCH_DESIGN.md,
/// `only_mintable`): still owed ecash, so still needing a resolvable processor.
async fn outstanding_quote_units(mint: &Mint) -> Result<HashSet<String>> {
    let quotes = mint
        .mint_quotes()
        .await
        .map_err(|e| anyhow!("listing mint quotes for resume failed: {e}"))?;
    Ok(quotes
        .into_iter()
        .filter(|q| q.amount_issued() < q.amount_paid())
        .map(|q| q.unit.to_string())
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use cdk::mint::MintBuilder;

    // secp256k1 generator point: a well-known valid compressed pubkey.
    const POOL_PUBKEY: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    static TEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn temp_store_path(label: &str) -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "epoch-manager-test-{}-{label}-{n}.json",
            std::process::id()
        ))
    }

    fn record(height: u64, unit: &str, state: EpochState) -> EpochRecord {
        EpochRecord {
            height,
            unit: unit.into(),
            keyset_id: "00aa".into(),
            block_hash: None,
            reward_sats: None,
            state,
            source: EpochSource::Manual,
            opened_at: 1,
        }
    }

    async fn test_mint() -> Arc<Mint> {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mint = MintBuilder::new(db.clone())
            .build_with_seed(db, &[7u8; 64])
            .await
            .unwrap();
        let mint = Arc::new(mint);
        mint.start().await.unwrap();
        mint
    }

    fn test_settings(store_path: std::path::PathBuf) -> EpochSettings {
        EpochSettings {
            pool_pubkey: POOL_PUBKEY.to_string(),
            store_path,
            // Resume never calls bitcoind; this just needs to parse.
            rpc_url: "http://127.0.0.1:0".to_string(),
            rpc_user: "user".to_string(),
            rpc_pass: "pass".to_string(),
            admin_listen: "127.0.0.1:0".to_string(),
        }
    }

    async fn seed_paid_quote(mint: &Mint, unit: &CurrencyUnit, amount: u64) {
        let processor = Arc::new(EhashPaymentProcessor::new(unit.clone()));
        mint.register_payment_processor(
            unit.clone(),
            ehash_method(),
            MintMeltLimits::new(1, u64::MAX),
            processor.clone() as Arc<dyn MintPayment<Err = cdk::cdk_payment::Error> + Send + Sync>,
        )
        .await
        .unwrap();

        let header_hash = "11".repeat(32);
        let request = cdk::MintQuoteRequest::Custom {
            method: ehash_method(),
            request: cdk::nuts::MintQuoteCustomRequest {
                amount: Some(cdk::Amount::from(amount)),
                unit: unit.clone(),
                description: None,
                pubkey: None,
                extra: serde_json::json!({ "header_hash": header_hash }),
            },
        };
        let response = mint.get_mint_quote(request).await.unwrap();
        let quote_id = response.quote().clone();

        processor
            .pay_ehash_quote(&header_hash, cdk::Amount::new(amount, unit.clone()))
            .await
            .unwrap();

        for _ in 0..50 {
            let quotes = mint.mint_quotes().await.unwrap();
            if quotes
                .iter()
                .any(|q| q.id == quote_id && q.amount_paid().value() >= amount)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("seeded quote for {unit} never reached paid state");
    }

    #[tokio::test]
    async fn resume_restores_current_and_owing_but_not_settled_closed_epochs() {
        let store_path = temp_store_path("resume");
        let _ = std::fs::remove_file(&store_path);

        let mut store = EpochStore::load(&store_path).unwrap();
        store
            .append(record(100, "hash_test_100_dissolved", EpochState::Dissolved))
            .unwrap();
        store
            .append(record(150, "hash_test_150_settled", EpochState::Final))
            .unwrap();
        store
            .append(record(200, "hash_test_200_owing", EpochState::Final))
            .unwrap();
        store
            .append(record(300, "hash_test_300_current", EpochState::Final))
            .unwrap();
        drop(store);

        let mint = test_mint().await;
        let owing_unit = CurrencyUnit::Custom("hash_test_200_owing".to_string().into());
        seed_paid_quote(&mint, &owing_unit, 10).await;

        let settings = test_settings(store_path.clone());
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .expect("resume must succeed");

        let dissolved_unit = CurrencyUnit::Custom("hash_test_100_dissolved".to_string().into());
        let settled_unit = CurrencyUnit::Custom("hash_test_150_settled".to_string().into());
        let current_unit = CurrencyUnit::Custom("hash_test_300_current".to_string().into());
        let mint_info = mint.mint_info().await.unwrap();

        assert!(
            mint.get_payment_processor(current_unit.clone(), ehash_method()).is_ok(),
            "current epoch must have a processor map entry"
        );
        assert!(
            mint_info
                .nuts
                .nut04
                .get_settings(&current_unit, &ehash_method())
                .is_some(),
            "current epoch must still accept new quotes"
        );

        assert!(
            mint.get_payment_processor(owing_unit.clone(), ehash_method()).is_ok(),
            "a closed epoch with a paid-but-unissued quote must stay resolvable after resume"
        );
        assert!(
            mint_info
                .nuts
                .nut04
                .get_settings(&owing_unit, &ehash_method())
                .is_none(),
            "a closed epoch must not accept new quotes after resume"
        );

        assert!(
            mint.get_payment_processor(settled_unit, ehash_method()).is_err(),
            "a closed epoch with no outstanding quotes must stay unregistered after resume"
        );
        assert!(
            mint.get_payment_processor(dissolved_unit, ehash_method()).is_err(),
            "a dissolved epoch must never get a processor map entry"
        );

        let _ = std::fs::remove_file(&store_path);
        drop(manager);
    }

    #[tokio::test]
    async fn resume_keeps_the_epoch_before_a_provisional_current_quotable() {
        let store_path = temp_store_path("provisional");
        let _ = std::fs::remove_file(&store_path);

        let mut store = EpochStore::load(&store_path).unwrap();
        store
            .append(record(100, "hash_test_100_prev", EpochState::Final))
            .unwrap();
        store
            .append(record(200, "hash_test_200_provisional", EpochState::Provisional))
            .unwrap();
        drop(store);

        let mint = test_mint().await;
        let settings = test_settings(store_path.clone());
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .expect("resume must succeed");

        let prev_unit = CurrencyUnit::Custom("hash_test_100_prev".to_string().into());
        let current_unit = CurrencyUnit::Custom("hash_test_200_provisional".to_string().into());
        let mint_info = mint.mint_info().await.unwrap();

        assert!(
            mint_info
                .nuts
                .nut04
                .get_settings(&prev_unit, &ehash_method())
                .is_some(),
            "the epoch before a provisional current epoch must stay quotable"
        );
        assert!(
            mint_info
                .nuts
                .nut04
                .get_settings(&current_unit, &ehash_method())
                .is_some(),
            "the provisional current epoch must itself be quotable"
        );

        let _ = std::fs::remove_file(&store_path);
        drop(manager);
    }

    #[tokio::test]
    async fn resume_keeps_the_whole_resumable_chain_quotable() {
        let store_path = temp_store_path("chain");
        let _ = std::fs::remove_file(&store_path);

        let mut store = EpochStore::load(&store_path).unwrap();
        store
            .append(record(100, "hash_test_100_final", EpochState::Final))
            .unwrap();
        store
            .append(record(200, "hash_test_200_provisional", EpochState::Provisional))
            .unwrap();
        store
            .append(record(300, "hash_test_300_provisional", EpochState::Provisional))
            .unwrap();
        drop(store);

        let mint = test_mint().await;
        let settings = test_settings(store_path.clone());
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .expect("resume must succeed");

        let mint_info = mint.mint_info().await.unwrap();
        for unit_str in [
            "hash_test_100_final",
            "hash_test_200_provisional",
            "hash_test_300_provisional",
        ] {
            let unit = CurrencyUnit::Custom(unit_str.to_string().into());
            assert!(
                mint_info
                    .nuts
                    .nut04
                    .get_settings(&unit, &ehash_method())
                    .is_some(),
                "{unit_str} could resume as current through a cascade of dissolves and must stay quotable"
            );
        }

        let _ = std::fs::remove_file(&store_path);
        drop(manager);
    }

    #[tokio::test]
    async fn resume_retires_an_epoch_excluded_from_the_register_set() {
        let store_path = temp_store_path("excluded-retire");
        let _ = std::fs::remove_file(&store_path);

        let mut store = EpochStore::load(&store_path).unwrap();
        store
            .append(record(100, "hash_test_100_stale", EpochState::Final))
            .unwrap();
        store
            .append(record(200, "hash_test_200_current", EpochState::Final))
            .unwrap();
        drop(store);

        let mint = test_mint().await;
        let stale_unit = CurrencyUnit::Custom("hash_test_100_stale".to_string().into());

        // Stand in for a quote-creation gate an earlier process opened and
        // crashed before retiring: register it directly, bypassing this
        // run's (bounded) register set entirely.
        let processor = Arc::new(EhashPaymentProcessor::new(stale_unit.clone()));
        mint.register_payment_processor(
            stale_unit.clone(),
            ehash_method(),
            MintMeltLimits::new(1, u64::MAX),
            processor as Arc<dyn MintPayment<Err = cdk::cdk_payment::Error> + Send + Sync>,
        )
        .await
        .unwrap();

        let settings = test_settings(store_path.clone());
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .expect("resume must succeed");

        let mint_info = mint.mint_info().await.unwrap();
        assert!(
            mint_info
                .nuts
                .nut04
                .get_settings(&stale_unit, &ehash_method())
                .is_none(),
            "an epoch excluded from the register set must still be retired"
        );

        let _ = std::fs::remove_file(&store_path);
        drop(manager);
    }

    #[tokio::test]
    async fn retire_all_except_fails_closed_on_first_error() {
        let records = vec![
            record(100, "a", EpochState::Final),
            record(200, "b", EpochState::Final),
            record(300, "c", EpochState::Final),
        ];
        let keep = HashSet::from(["c".to_string()]);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let result = retire_all_except(&records, &keep, move |unit| {
            let seen = seen_clone.clone();
            async move {
                seen.lock().unwrap().push(unit.clone());
                if unit == "b" {
                    Err(anyhow!("simulated retire failure"))
                } else {
                    Ok(())
                }
            }
        })
        .await;

        assert!(result.is_err(), "a retire failure must abort, not log-and-continue");
        assert_eq!(*seen.lock().unwrap(), vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn retire_all_except_succeeds_when_every_retire_succeeds() {
        let records = vec![
            record(100, "a", EpochState::Final),
            record(200, "b", EpochState::Final),
            record(300, "c", EpochState::Final),
        ];
        let keep = HashSet::from(["c".to_string()]);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let result = retire_all_except(&records, &keep, move |unit| {
            let seen = seen_clone.clone();
            async move {
                seen.lock().unwrap().push(unit);
                Ok(())
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(*seen.lock().unwrap(), vec!["a".to_string(), "b".to_string()]);
    }
}
