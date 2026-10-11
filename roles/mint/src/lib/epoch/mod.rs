//! Mining epoch mechanics: per-epoch currency units named by block height,
//! opened and closed by the mint. See docs/EPOCH_DESIGN.md.

pub mod naming;
pub mod store;

use anyhow::{anyhow, Context, Result};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use cdk::{
    cdk_payment::{MintPayment, WaitPaymentResponse},
    mint::{Mint, MintMeltLimits},
    nuts::{CurrencyUnit, PaymentMethod},
    Amount,
};
use cdk_common::database::DynMintDatabase;
use cdk_ehash::EhashPaymentProcessor;
use rpc_sv2::mini_rpc_client::{Auth, BlockInfo, MiniRpcClient};
use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use store::{EpochRecord, EpochSource, EpochState, EpochStore, ScannedBlock, Tx};
use tokio::sync::{RwLock, RwLockReadGuard};
use tracing::{info, warn};

/// HRTB-boxed: the closure is polymorphic over a lifetime chosen by
/// `with_store_tx`'s own locals, which a bare `Fut: Future` can't express.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const NUM_KEYS: u32 = 64;

/// A hung (not refusing) node must not wedge the watcher task.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

fn ehash_method() -> PaymentMethod {
    PaymentMethod::Custom("ehash".to_string())
}

/// Distinguishes "stop the watcher" from a transient RPC error in `spawn_watcher`'s loop.
#[derive(Debug)]
pub struct InvariantViolation(pub String);

impl std::fmt::Display for InvariantViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for InvariantViolation {}

fn is_invariant_violation(e: &anyhow::Error) -> bool {
    e.downcast_ref::<InvariantViolation>().is_some()
}

/// A `Final` open was attempted while current is provisional.
#[derive(Debug)]
pub struct ProvisionalCurrent;

impl std::fmt::Display for ProvisionalCurrent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "current epoch is provisional; manual rotation refused")
    }
}

impl std::error::Error for ProvisionalCurrent {}

#[derive(Debug, Clone)]
pub struct EpochSettings {
    /// Namespaces every epoch unit (`hash_<pool>_<height>`).
    pub pool_pubkey: String,
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_pass: String,
    pub admin_listen: String,
    /// Matched byte-for-byte against the coinbase output, never as an address string.
    pub receive_script: bitcoin::ScriptBuf,
    /// A provisional boundary is final at `tip - height + 1 >= confirmation_depth`.
    pub confirmation_depth: u32,
    pub poll_interval: Duration,
    /// The dissolve re-stamp opens a second sqlite connection to this path directly.
    pub mint_db_path: PathBuf,
}

pub struct EpochManager {
    mint: Arc<Mint>,
    /// Lock order is always store, then `current` (never the reverse).
    store: tokio::sync::Mutex<EpochStore>,
    current: RwLock<EpochRecord>,
    pool_pubkey: String,
    amounts: Vec<u64>,
    rpc: MiniRpcClient,
    receive_script: bitcoin::ScriptBuf,
    confirmation_depth: u32,
    poll_interval: Duration,
    mint_db_path: PathBuf,
    /// Test-only seam: forces `pay_unpaid_quotes_in_tx` to fail without touching the mint.
    #[cfg(test)]
    pay_hook: std::sync::Mutex<Option<Box<dyn Fn() -> Result<()> + Send + Sync>>>,
}

impl EpochManager {
    /// Blocks (with retry) until bitcoind answers.
    pub async fn load_or_genesis(mint: Arc<Mint>, settings: EpochSettings) -> Result<Arc<Self>> {
        let pool_pubkey = naming::validate_pool_pubkey(&settings.pool_pubkey)?;
        let uri: hyper::Uri = settings
            .rpc_url
            .parse()
            .with_context(|| format!("invalid bitcoin_rpc url {}", settings.rpc_url))?;
        let rpc = MiniRpcClient::new(uri, Auth::new(settings.rpc_user, settings.rpc_pass));
        let db: DynMintDatabase = mint.localstore();
        let store_opt = EpochStore::load(&db).await?;
        let amounts: Vec<u64> = (0..NUM_KEYS).map(|i| 2_u64.pow(i)).collect();

        if let Some(store) = store_opt {
            // Resume's KV records are authoritative, unlike genesis (below), so this only warns.
            if let Some(dir) = settings.mint_db_path.parent() {
                let legacy = dir.join("epochs.json");
                if legacy.exists() {
                    warn!(
                        path = %legacy.display(),
                        "stale epochs.json from an older version found beside the mint database; \
                         it is ignored (epoch records now live in the mint database) and can be deleted"
                    );
                }
            }

            let current = store
                .current()
                .cloned()
                .ok_or_else(|| anyhow!("epoch store has records but no current (non-dissolved) one"))?;
            // Chronological, not insertion order: a reorg rescan can append
            // a lower height after still-provisional higher ones.
            let chronological = store.chronological();
            let outstanding = outstanding_quote_units(&mint).await?;

            // The whole resumable chain must stay quotable (EPOCH_DESIGN.md, "Dissolve").
            let chain = resumable_chain(&chronological);

            // EPOCH_DESIGN.md, `only_mintable`.
            let to_restore: Vec<EpochRecord> = chronological
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
                receive_script: settings.receive_script,
                confirmation_depth: settings.confirmation_depth,
                poll_interval: settings.poll_interval,
                mint_db_path: settings.mint_db_path,
                #[cfg(test)]
                pay_hook: std::sync::Mutex::new(None),
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
            retire_all_except(&chronological, &chain, move |unit| {
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

        // Refuse genesis next to a legacy epochs.json: its records can't be
        // imported, so starting fresh here would silently abandon them.
        if let Some(dir) = settings.mint_db_path.parent() {
            let legacy = dir.join("epochs.json");
            if legacy.exists() {
                return Err(anyhow!(
                    "{} exists from an older version; the epoch store now lives in the mint \
                     database and this file's records cannot be imported. Run `just clean \
                     cashu` for a clean slate, or move the file elsewhere for reference.",
                    legacy.display()
                ));
            }
        }

        let height = block_count_with_retry(&rpc, 30, Duration::from_secs(2))
            .await
            .context("genesis needs the chain height; is bitcoind reachable?")?;
        let genesis_hash = block_hash_with_retry(&rpc, height, 30, Duration::from_secs(2))
            .await
            .context("genesis needs the chain hash; is bitcoind reachable?")?;
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
            store: tokio::sync::Mutex::new(EpochStore::new_empty()),
            current: RwLock::new(placeholder),
            pool_pubkey,
            amounts,
            rpc,
            receive_script: settings.receive_script,
            confirmation_depth: settings.confirmation_depth,
            poll_interval: settings.poll_interval,
            mint_db_path: settings.mint_db_path,
            #[cfg(test)]
            pay_hook: std::sync::Mutex::new(None),
        });
        let watermark = ScannedBlock {
            height,
            hash: genesis_hash,
        };
        let record = manager
            .open_epoch_inner(
                height,
                None,
                None,
                EpochSource::Genesis,
                EpochState::Final,
                Some(watermark),
            )
            .await?;
        info!(unit = %record.unit, height, "genesis epoch opened");
        Ok(manager)
    }

    /// Owns the transaction so it can await `tx.rollback()` on error instead
    /// of dropping `tx`: cdk's `Drop` only schedules a background rollback,
    /// not guaranteed to finish before the next writer needs the lock.
    async fn with_store_tx<F, T>(&self, f: F) -> Result<T>
    where
        for<'a> F: FnOnce(&'a mut EpochStore, &'a mut Tx) -> BoxFuture<'a, Result<T>>,
    {
        let mut guard = self.store.lock().await;
        let mut clone = guard.clone();
        let mut tx = self
            .mint
            .localstore()
            .begin_transaction()
            .await
            .map_err(|e| anyhow!("begin_transaction: {e}"))?;

        match f(&mut clone, &mut tx).await {
            Ok(value) => {
                tx.commit().await.map_err(|e| anyhow!("commit: {e}"))?;
                *guard = clone;
                Ok(value)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    /// Contract: hold the returned guard across both "which unit" and "pay
    /// now or not" — releasing it between the two can strand a final-epoch
    /// quote unpaid, or create one in a unit mid-dissolve.
    pub async fn current_epoch(&self) -> RwLockReadGuard<'_, EpochRecord> {
        self.current.read().await
    }

    pub async fn chain_height(&self) -> Result<u64> {
        rpc_block_count(&self.rpc).await
    }

    fn recent_cap(&self) -> usize {
        std::cmp::max(2 * self.confirmation_depth as usize, 16)
    }

    fn invariant_error(&self, msg: String) -> anyhow::Error {
        tracing::error!("critical invariant violation: {msg}");
        anyhow::Error::new(InvariantViolation(msg))
    }

    #[cfg(test)]
    fn set_pay_hook(&self, hook: impl Fn() -> Result<()> + Send + Sync + 'static) {
        *self.pay_hook.lock().expect("pay_hook lock poisoned") = Some(Box::new(hook));
    }

    #[cfg(test)]
    fn clear_pay_hook(&self) {
        *self.pay_hook.lock().expect("pay_hook lock poisoned") = None;
    }

    async fn get_block_hash(&self, height: u64) -> Result<String> {
        tokio::time::timeout(RPC_TIMEOUT, self.rpc.get_block_hash(height))
            .await
            .map_err(|_| anyhow!("getblockhash timed out after {RPC_TIMEOUT:?}"))?
            .map_err(|e| anyhow!("getblockhash failed: {e:?}"))
    }

    async fn get_block_info(&self, hash: &str) -> Result<BlockInfo> {
        tokio::time::timeout(RPC_TIMEOUT, self.rpc.get_block_info(hash))
            .await
            .map_err(|_| anyhow!("getblock timed out after {RPC_TIMEOUT:?}"))?
            .map_err(|e| anyhow!("getblock failed: {e:?}"))
    }

    async fn get_raw_transaction_hex(&self, txid: &str, block_hash: &str) -> Result<String> {
        tokio::time::timeout(RPC_TIMEOUT, self.rpc.get_raw_transaction_hex(txid, block_hash))
            .await
            .map_err(|_| anyhow!("getrawtransaction timed out after {RPC_TIMEOUT:?}"))?
            .map_err(|e| anyhow!("getrawtransaction failed: {e:?}"))
    }

    async fn coinbase_reward(&self, info: &BlockInfo, hash: &str) -> Result<u64> {
        let coinbase_txid = info
            .tx
            .first()
            .ok_or_else(|| anyhow!("block {hash} has no transactions"))?;
        let raw_hex = self.get_raw_transaction_hex(coinbase_txid, hash).await?;
        let bytes = hex::decode(&raw_hex)
            .with_context(|| format!("decoding coinbase tx hex for block {hash}"))?;
        let tx: bitcoin::Transaction = bitcoin::consensus::deserialize(&bytes)
            .with_context(|| format!("decoding coinbase tx for block {hash}"))?;
        let sats = tx
            .output
            .iter()
            .filter(|o| o.script_pubkey.as_bytes() == self.receive_script.as_bytes())
            .map(|o| o.value.to_sat())
            .sum();
        Ok(sats)
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

    /// `state: Final` retires the previous epoch's quote-creation entry at
    /// once; `Provisional` leaves it quotable in case dissolve resumes it.
    pub async fn open_epoch(
        &self,
        height: u64,
        block_hash: Option<String>,
        reward_sats: Option<u64>,
        source: EpochSource,
        state: EpochState,
    ) -> Result<EpochRecord> {
        self.open_epoch_inner(height, block_hash, reward_sats, source, state, None)
            .await
    }

    /// `watermark`, when given, is written in the same transaction as the
    /// new record (genesis only; see `load_or_genesis`).
    async fn open_epoch_inner(
        &self,
        height: u64,
        block_hash: Option<String>,
        reward_sats: Option<u64>,
        source: EpochSource,
        state: EpochState,
        watermark: Option<ScannedBlock>,
    ) -> Result<EpochRecord> {
        const MAX_NAME_ATTEMPTS: u32 = 32;

        // Keyset creation is cdk's own transaction and happens before the
        // store is touched: a losing race or a crash here leaves only an
        // inert orphan keyset.
        let mut suffix = {
            let store = self.store.lock().await;
            store.count_at_height(height)
        };
        let mut attempts = 0u32;
        let (unit, keyset_id) = loop {
            attempts += 1;
            if attempts > MAX_NAME_ATTEMPTS {
                return Err(anyhow!(
                    "no usable unit name for height {height} after {MAX_NAME_ATTEMPTS} attempts"
                ));
            }
            let name = naming::unit_name(&self.pool_pubkey, height, suffix);
            let taken = {
                let store = self.store.lock().await;
                store.unit_taken(&name)
            };
            if taken {
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

        let record = EpochRecord {
            height,
            unit: unit.to_string(),
            keyset_id,
            block_hash,
            reward_sats,
            state,
            source,
            opened_at: store::unix_now(),
        };
        let cap = self.recent_cap();

        // Not `with_store_tx`: `current` must publish before the store lock
        // releases, or two concurrent opens could commit and publish in
        // opposite order.
        let mut store_guard = self.store.lock().await;
        let mut current = self.current.write().await;

        // Re-checked under both locks: a pre-check anywhere else could race a reward.
        let previous = store_guard.current().cloned();
        if state == EpochState::Final {
            if let Some(prev) = &previous {
                if prev.state == EpochState::Provisional {
                    return Err(anyhow::Error::new(ProvisionalCurrent));
                }
            }
        }

        let mut store = store_guard.clone();
        let mut tx = self
            .mint
            .localstore()
            .begin_transaction()
            .await
            .map_err(|e| anyhow!("begin_transaction: {e}"))?;

        let work = async {
            store.append(&mut tx, record.clone()).await?;
            if let Some(w) = watermark.clone() {
                store.set_watermark(&mut tx, w, cap).await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(e) = work {
            let _ = tx.rollback().await;
            return Err(e);
        }

        tx.commit().await.map_err(|e| anyhow!("commit: {e}"))?;

        *store_guard = store;
        *current = record.clone();
        drop(current);
        drop(store_guard);

        // Retire, not deregister: deregister drops the processor map entry
        // too, stranding paid-but-unissued quotes (EPOCH_DESIGN.md, "Dissolve").
        if state == EpochState::Final {
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
        }

        info!(unit = %record.unit, height, ?source, ?state, "epoch opened");
        Ok(record)
    }

    /// A transient tick error logs and retries; an invariant violation logs
    /// and stops the watcher (the mint keeps serving everything else).
    pub fn spawn_watcher(self: &Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                match manager.tick().await {
                    Ok(()) => {}
                    Err(e) if is_invariant_violation(&e) => {
                        tracing::error!("epoch watcher stopped: {e}");
                        break;
                    }
                    Err(e) => {
                        warn!("epoch watcher tick failed (will retry): {e}");
                    }
                }
                tokio::time::sleep(manager.poll_interval).await;
            }
        });
    }

    async fn tick(&self) -> Result<()> {
        self.init_watermark_if_absent().await?;
        // Shared tip: nothing below asks the node for a height above it.
        let tip = rpc_block_count(&self.rpc).await?;
        self.resync(tip).await?;
        self.walk_forward(tip).await?;
        self.run_finality_pass(tip).await?;
        Ok(())
    }

    /// An absent watermark starts the scan at the current tip (rewards
    /// below it are never scanned); genesis always sets its own.
    async fn init_watermark_if_absent(&self) -> Result<()> {
        let has_watermark = {
            let store = self.store.lock().await;
            store.watermark().is_some()
        };
        if has_watermark {
            return Ok(());
        }
        let tip = rpc_block_count(&self.rpc).await?;
        let hash = self.get_block_hash(tip).await?;
        warn!(height = tip, "epoch store has no watermark; starting scan at the current tip");
        let cap = self.recent_cap();
        self.with_store_tx(|store, tx| {
            let hash = hash.clone();
            Box::pin(async move {
                store.set_watermark(tx, ScannedBlock { height: tip, hash }, cap).await?;
                Ok(())
            })
        })
        .await
    }

    /// The newest agreed entry is the rollback point; an entry above `tip`
    /// is never canonical and costs no RPC call.
    async fn resync(&self, tip: u64) -> Result<()> {
        let recent = {
            let store = self.store.lock().await;
            store.recent().to_vec()
        };
        if recent.is_empty() {
            return Ok(());
        }
        let watermark = {
            let store = self.store.lock().await;
            store.watermark().cloned()
        };

        let found = rollback_point(&recent, tip, |b| async move {
            let hash = self.get_block_hash(b.height).await?;
            Ok(hash == b.hash)
        })
        .await?;

        match found {
            Some(point) => {
                if Some(&point) != watermark.as_ref() {
                    info!(
                        from = watermark.map(|w| w.height).unwrap_or(0),
                        to = point.height,
                        "watermark rollback"
                    );
                    self.with_store_tx(|store, tx| {
                        let height = point.height;
                        Box::pin(async move {
                            store.rollback_to(tx, height).await?;
                            Ok(())
                        })
                    })
                    .await?;
                }
                Ok(())
            }
            None => {
                tracing::error!("reorg deeper than the retained window");
                let oldest = recent.first().expect("checked non-empty above");
                // Clamp to `tip`: if the chain also shrank, "one below the
                // oldest retained height" can itself still be above it.
                let height = oldest.height.saturating_sub(1).min(tip);
                let hash = self.get_block_hash(height).await?;
                let cap = self.recent_cap();
                self.with_store_tx(|store, tx| {
                    let hash = hash.clone();
                    Box::pin(async move {
                        store.rollback_to(tx, height).await?;
                        store.set_watermark(tx, ScannedBlock { height, hash }, cap).await?;
                        Ok(())
                    })
                })
                .await?;
                Ok(())
            }
        }
    }

    async fn walk_forward(&self, tip: u64) -> Result<()> {
        loop {
            let watermark = {
                let store = self.store.lock().await;
                store.watermark().cloned()
            };
            let watermark = match watermark {
                Some(w) => w,
                None => return Ok(()),
            };
            if watermark.height >= tip {
                return Ok(());
            }
            let height = watermark.height + 1;
            let hash = self.get_block_hash(height).await?;
            let info = self.get_block_info(&hash).await?;
            if info.previousblockhash.as_deref() != Some(watermark.hash.as_str()) {
                // The chain moved under the walk; the next tick's resync handles it.
                return Ok(());
            }
            let reward_sats = self.coinbase_reward(&info, &hash).await?;
            self.process_block(height, hash.clone(), reward_sats).await?;
            tracing::debug!(height, hash = %hash, "watermark advancing");
            let cap = self.recent_cap();
            self.with_store_tx(|store, tx| {
                let hash = hash.clone();
                Box::pin(async move {
                    store.set_watermark(tx, ScannedBlock { height, hash }, cap).await?;
                    Ok(())
                })
            })
            .await?;
        }
    }

    async fn process_block(&self, height: u64, hash: String, reward_sats: u64) -> Result<()> {
        if reward_sats > 0 {
            info!(height, hash = %hash, reward_sats, "reward detected");
        }
        let records = {
            let store = self.store.lock().await;
            store.non_dissolved()
        };
        match plan_block(&records, height, &hash, reward_sats) {
            BlockAction::Nothing => {}
            BlockAction::ReMine { unit } => {
                let old_hash = records
                    .iter()
                    .find(|r| r.unit == unit)
                    .and_then(|r| r.block_hash.clone());
                // `current` is written only after `with_store_tx` commits,
                // never inside the closure: a failed commit must not have
                // already announced the new hash.
                let updated = self
                    .with_store_tx(|store, tx| {
                        let unit = unit.clone();
                        let hash = hash.clone();
                        Box::pin(async move {
                            store
                                .update_record(tx, &unit, |r| {
                                    r.block_hash = Some(hash.clone());
                                    r.reward_sats = Some(reward_sats);
                                })
                                .await?;
                            Ok(store.record(&unit))
                        })
                    })
                    .await?;
                if let Some(updated) = updated {
                    let mut current = self.current.write().await;
                    if current.unit == unit {
                        *current = updated;
                    }
                }
                info!(unit = %unit, height, old_hash = ?old_hash, new_hash = %hash, "epoch re-mined at same height");
            }
            BlockAction::Dissolve { unit } => {
                self.dissolve(&unit).await?;
            }
            BlockAction::Open => {
                self.open_epoch(
                    height,
                    Some(hash.clone()),
                    Some(reward_sats),
                    EpochSource::Reward,
                    EpochState::Provisional,
                )
                .await?;
            }
            BlockAction::ReorgPastFinal { unit } => {
                tracing::error!(
                    unit = %unit,
                    height,
                    hash = %hash,
                    "reorg crossed a final boundary; accepted residual risk, taking no action"
                );
            }
        }
        Ok(())
    }

    async fn run_finality_pass(&self, tip: u64) -> Result<()> {
        // Chronological order, not insertion order: see `chronological`.
        let records = {
            let store = self.store.lock().await;
            store.chronological()
        };
        let mut canonical = std::collections::HashMap::new();
        for r in records.iter().filter(|r| r.state == EpochState::Provisional) {
            if r.height > tip {
                // Above tip: never canonical, so skip rather than ask the node for a height it doesn't have yet.
                tracing::debug!(height = r.height, tip, unit = %r.unit, "boundary above the tip; skipping finality check");
                continue;
            }
            if let Some(expected_hash) = &r.block_hash {
                let actual = self.get_block_hash(r.height).await?;
                canonical.insert(r.unit.clone(), actual == *expected_hash);
            }
        }
        let due = due_for_finality(&records, tip, self.confirmation_depth, |r| {
            canonical.get(&r.unit).copied().unwrap_or(false)
        });
        for unit in due {
            self.finalize(&unit).await?;
        }
        Ok(())
    }

    /// Pays and flips to `Final` in one transaction; a failing pay leaves
    /// nothing persisted and the next finality pass retries.
    ///
    /// Not `with_store_tx`: sqlite allows one writer, so `mint.mint_quotes()`
    /// (a second connection) must run before our transaction opens.
    async fn finalize(&self, unit: &str) -> Result<()> {
        let unit = unit.to_string();
        let mut store_guard = self.store.lock().await;
        let mut current = self.current.write().await;

        let target = CurrencyUnit::Custom(unit.clone().into());
        let candidates: Vec<_> = self
            .mint
            .mint_quotes()
            .await
            .map_err(|e| anyhow!("mint_quotes: {e}"))?
            .into_iter()
            .filter(|q| q.unit == target && q.amount_paid().value() == 0)
            .collect();

        let mut store = store_guard.clone();
        let mut tx = self
            .mint
            .localstore()
            .begin_transaction()
            .await
            .map_err(|e| anyhow!("begin_transaction: {e}"))?;

        // Explicit awaited rollback, not drop: cdk's background rollback-on-drop
        // may still be running when the next writer needs the lock.
        let work = async {
            let notify = self.pay_unpaid_quotes_in_tx(&mut tx, candidates).await?;
            store.update_record(&mut tx, &unit, |r| r.state = EpochState::Final).await?;
            Ok::<_, anyhow::Error>(notify)
        }
        .await;
        let notify = match work {
            Ok(notify) => notify,
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(e);
            }
        };
        let updated = store
            .record(&unit)
            .expect("just updated, must still be present");
        let prev = store.previous_non_dissolved(&unit);

        tx.commit().await.map_err(|e| anyhow!("commit: {e}"))?;

        *store_guard = store;
        if current.unit == unit {
            *current = updated.clone();
        }
        drop(current);
        drop(store_guard);

        let (height, block_hash) = (updated.height, updated.block_hash.clone());

        for (quote, amount) in &notify {
            self.mint.pubsub_manager().mint_quote_payment(quote, amount.clone());
        }

        if let Some(prev) = &prev {
            let prev_unit = CurrencyUnit::Custom(prev.unit.clone().into());
            if let Err(e) = self
                .mint
                .retire_payment_processor(prev_unit, ehash_method())
                .await
            {
                warn!(unit = %prev.unit, "failed to retire previous epoch entry at finality: {e}");
            }
        }

        info!(
            unit = %unit,
            height,
            block_hash = ?block_hash,
            quotes_paid = notify.len(),
            retired_unit = ?prev.as_ref().map(|r| r.unit.clone()),
            "epoch finalized"
        );
        Ok(())
    }

    /// Refuses (nothing mutated) if the unit's keyset has issued anything or
    /// any of its quotes are owing. Pays before persisting the flip, so a
    /// retry after a failed pay re-stamps 0 rows and just pays `prev`.
    ///
    /// Not `with_store_tx`: sqlite allows one writer, and the raw-SQL
    /// re-stamp can't overlap our own open transaction.
    async fn dissolve(&self, unit: &str) -> Result<()> {
        let unit = unit.to_string();
        let mut store_guard = self.store.lock().await;
        let mut current = self.current.write().await;

        let record = store_guard
            .record(&unit)
            .ok_or_else(|| anyhow!("no epoch record for unit {unit}"))?;
        let prev = store_guard.previous_non_dissolved(&unit).ok_or_else(|| {
            self.invariant_error(format!(
                "dissolve {unit}: no previous non-dissolved epoch to resume into"
            ))
        })?;

        let keyset_id = cdk::nuts::Id::from_str(&record.keyset_id)
            .map_err(|e| anyhow!("parsing keyset id {}: {e}", record.keyset_id))?;
        let issued = self
            .mint
            .total_issued()
            .await
            .map_err(|e| anyhow!("total_issued: {e}"))?;
        let issued_amount = issued.get(&keyset_id).copied().map(|a| a.to_u64()).unwrap_or(0);
        if issued_amount != 0 {
            return Err(self.invariant_error(format!(
                "dissolve {unit}: keyset {} has issued {issued_amount}, must be zero",
                record.keyset_id
            )));
        }

        let target_unit = CurrencyUnit::Custom(unit.clone().into());
        let quotes = self
            .mint
            .mint_quotes()
            .await
            .map_err(|e| anyhow!("mint_quotes: {e}"))?;
        let owing: Vec<_> = quotes.into_iter().filter(|q| q.unit == target_unit).collect();
        if owing
            .iter()
            .any(|q| q.amount_paid().value() != 0 || q.amount_issued().value() != 0)
        {
            return Err(self.invariant_error(format!(
                "dissolve {unit}: a quote in the dissolving unit has nonzero amount_paid or amount_issued"
            )));
        }
        let expected = owing.len();

        let affected = self.restamp_quotes(&unit, &prev.unit).await?;
        if affected != expected {
            return Err(self.invariant_error(format!(
                "dissolve {unit}: re-stamp affected {affected} row(s), expected {expected}"
            )));
        }

        let prev_currency = CurrencyUnit::Custom(prev.unit.clone().into());
        let pay_candidates = if prev.state == EpochState::Final {
            self.mint
                .mint_quotes()
                .await
                .map_err(|e| anyhow!("mint_quotes: {e}"))?
                .into_iter()
                .filter(|q| q.unit == prev_currency && q.amount_paid().value() == 0)
                .collect()
        } else {
            Vec::new()
        };

        let mut store = store_guard.clone();
        let mut tx = self
            .mint
            .localstore()
            .begin_transaction()
            .await
            .map_err(|e| anyhow!("begin_transaction: {e}"))?;

        // Explicit awaited rollback, same reason as `finalize`.
        let work = async {
            let notify = self.pay_unpaid_quotes_in_tx(&mut tx, pay_candidates).await?;
            store.update_record(&mut tx, &unit, |r| r.state = EpochState::Dissolved).await?;
            Ok::<_, anyhow::Error>(notify)
        }
        .await;
        let notify = match work {
            Ok(notify) => notify,
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(e);
            }
        };
        let was_current = current.unit == unit;

        tx.commit().await.map_err(|e| anyhow!("commit: {e}"))?;

        *store_guard = store;
        if was_current {
            *current = prev.clone();
        }
        drop(current);
        drop(store_guard);

        let (height, old_hash, restamped) = (record.height, record.block_hash.clone(), affected);

        for (quote, amount) in &notify {
            self.mint.pubsub_manager().mint_quote_payment(quote, amount.clone());
        }

        let dissolved_unit = CurrencyUnit::Custom(unit.clone().into());
        if let Err(e) = self
            .mint
            .retire_payment_processor(dissolved_unit, ehash_method())
            .await
        {
            warn!(unit = %unit, "failed to retire dissolved epoch entry: {e}");
        }

        info!(
            unit = %unit,
            height,
            old_hash = ?old_hash,
            quotes_restamped = restamped,
            quotes_paid_in_target = notify.len(),
            target_unit = %prev.unit,
            "epoch dissolved"
        );
        Ok(())
    }

    /// Direct SQL re-stamp: cdk exposes no API to change a quote's unit.
    /// Opens a second connection on the same sqlite file through cdk's own
    /// pool and statement layer.
    async fn restamp_quotes(&self, from_unit: &str, to_unit: &str) -> Result<usize> {
        let path_str = self.mint_db_path.to_str().ok_or_else(|| {
            anyhow!(
                "mint db path {} is not valid UTF-8",
                self.mint_db_path.display()
            )
        })?;
        let pool = cdk_sql_common::pool::Pool::<cdk_sqlite::SqliteConnectionManager>::new(
            path_str.into(),
        );
        let conn = pool.get().await.map_err(|e| {
            anyhow!(
                "opening a second sqlite connection on {}: {e}",
                self.mint_db_path.display()
            )
        })?;
        let affected = cdk_sql_common::stmt::query(
            "UPDATE mint_quote SET unit = :to WHERE unit = :from AND amount_paid = 0 AND amount_issued = 0",
        )
        .map_err(|e| anyhow!("{e}"))?
        .bind("to", to_unit.to_string())
        .bind("from", from_unit.to_string())
        .execute(&*conn)
        .await
        .map_err(|e| anyhow!("re-stamping quotes from {from_unit} to {to_unit}: {e}"))?;
        Ok(affected)
    }

    /// `candidates` must be listed by the caller before `tx` opens (see
    /// `finalize`/`dissolve`). Returns newly-paid quotes for the caller to
    /// notify after `tx` commits, not before. Safe only because main.rs
    /// closes the HTTP route that creates ehash quotes; otherwise this
    /// mints for free at finality.
    async fn pay_unpaid_quotes_in_tx(
        &self,
        tx: &mut Tx,
        candidates: Vec<cdk::mint::MintQuote>,
    ) -> Result<Vec<(cdk::mint::MintQuote, Amount<CurrencyUnit>)>> {
        #[cfg(test)]
        if let Some(hook) = self.pay_hook.lock().expect("pay_hook lock poisoned").as_ref() {
            hook()?;
        }
        let mut notify = Vec::new();
        for quote in candidates {
            let acquired = tx
                .get_mint_quote_by_request_lookup_id(&quote.request_lookup_id)
                .await
                .map_err(|e| anyhow!("get_mint_quote_by_request_lookup_id: {e}"))?;
            let mut acquired = match acquired {
                Some(a) => a,
                None => continue,
            };
            let amount = acquired
                .amount
                .clone()
                .ok_or_else(|| anyhow!("quote {} has no amount", acquired.id))?;
            let payment_id = match &acquired.request_lookup_id {
                cdk::cdk_payment::PaymentIdentifier::CustomId(s) => s.clone(),
                other => other.to_string(),
            };
            let response = WaitPaymentResponse {
                payment_identifier: acquired.request_lookup_id.clone(),
                payment_amount: amount,
                payment_id,
            };
            let notified = self
                .mint
                .pay_mint_quote(tx, &mut acquired, response)
                .await
                .map_err(|e| anyhow!("paying quote {}: {e}", acquired.id))?;
            if notified {
                notify.push(((*acquired).clone(), acquired.amount_paid()));
            }
        }
        Ok(notify)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockAction {
    Nothing,
    ReMine { unit: String },
    Dissolve { unit: String },
    Open,
    ReorgPastFinal { unit: String },
}

pub fn plan_block(
    records: &[EpochRecord],
    height: u64,
    hash: &str,
    reward_sats: u64,
) -> BlockAction {
    if let Some(r) = records
        .iter()
        .find(|r| r.height == height && r.state == EpochState::Provisional)
    {
        if r.block_hash.as_deref() == Some(hash) {
            BlockAction::Nothing
        } else if reward_sats > 0 {
            BlockAction::ReMine { unit: r.unit.clone() }
        } else {
            BlockAction::Dissolve { unit: r.unit.clone() }
        }
    } else if reward_sats > 0 {
        // Hashless Final records (Manual/Genesis) were never a chain
        // boundary and must not suppress or reinterpret a reward here.
        if let Some(r) = records
            .iter()
            .find(|r| r.height == height && r.state == EpochState::Final && r.block_hash.is_some())
        {
            if r.block_hash.as_deref() == Some(hash) {
                BlockAction::Nothing
            } else {
                BlockAction::ReorgPastFinal { unit: r.unit.clone() }
            }
        } else {
            BlockAction::Open
        }
    } else {
        BlockAction::Nothing
    }
}

/// Due at `tip - height + 1 >= depth` (depth 1 is final at detection).
/// Preserves `records`' order.
pub fn due_for_finality(
    records: &[EpochRecord],
    tip: u64,
    depth: u32,
    is_canonical: impl Fn(&EpochRecord) -> bool,
) -> Vec<String> {
    records
        .iter()
        .filter(|r| r.state == EpochState::Provisional)
        .filter(|r| tip.saturating_sub(r.height) + 1 >= depth as u64)
        .filter(|r| is_canonical(r))
        .map(|r| r.unit.clone())
        .collect()
}

/// An entry above `tip` is never canonical and `is_canonical` is never called for it.
pub async fn rollback_point<F, Fut>(
    recent: &[ScannedBlock],
    tip: u64,
    is_canonical: F,
) -> Result<Option<ScannedBlock>>
where
    F: Fn(ScannedBlock) -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    for b in recent.iter().rev() {
        if b.height > tip {
            tracing::debug!(height = b.height, tip, "retained entry above the tip; skipping");
            continue;
        }
        if is_canonical(b.clone()).await? {
            return Ok(Some(b.clone()));
        }
    }
    Ok(None)
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

/// Every epoch a cascade of dissolves could hand `current` to. Requires
/// `records` in chronological order, oldest first.
fn resumable_chain(records: &[EpochRecord]) -> HashSet<String> {
    let mut chain = HashSet::new();
    for record in records.iter().rev() {
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

async fn rpc_block_count(rpc: &MiniRpcClient) -> Result<u64> {
    tokio::time::timeout(RPC_TIMEOUT, rpc.get_block_count())
        .await
        .map_err(|_| anyhow!("getblockcount timed out after {RPC_TIMEOUT:?}"))?
        .map_err(|e| anyhow!("getblockcount failed: {e:?}"))
}

async fn rpc_block_hash(rpc: &MiniRpcClient, height: u64) -> Result<String> {
    tokio::time::timeout(RPC_TIMEOUT, rpc.get_block_hash(height))
        .await
        .map_err(|_| anyhow!("getblockhash timed out after {RPC_TIMEOUT:?}"))?
        .map_err(|e| anyhow!("getblockhash failed: {e:?}"))
}

async fn block_count_with_retry(
    rpc: &MiniRpcClient,
    attempts: u32,
    delay: Duration,
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

async fn block_hash_with_retry(
    rpc: &MiniRpcClient,
    height: u64,
    attempts: u32,
    delay: Duration,
) -> Result<String> {
    let mut last_err = None;
    for _ in 0..attempts {
        match rpc_block_hash(rpc, height).await {
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
        .open_epoch(height, None, None, EpochSource::Manual, EpochState::Final)
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
        Err(e) if e.downcast_ref::<ProvisionalCurrent>().is_some() => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": e.to_string() })),
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

    fn temp_db_path(label: &str) -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "epoch-manager-test-{}-{label}-{n}.sqlite",
            std::process::id()
        ))
    }

    /// A syntactically valid V1 keyset id, for records seeded without a real keyset behind them.
    fn valid_keyset_id(n: u64) -> String {
        format!("00{n:014x}")
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

    fn block(height: u64, hash: &str) -> ScannedBlock {
        ScannedBlock {
            height,
            hash: hash.into(),
        }
    }

    /// Commits so `load_or_genesis` sees it on the resume path without reaching bitcoind.
    async fn seed_genesis_store(db: &DynMintDatabase, unit: &str) {
        let mut store = EpochStore::new_empty();
        let mut tx = db.begin_transaction().await.unwrap();
        store
            .append(&mut tx, record(1, unit, EpochState::Final))
            .await
            .unwrap();
        store
            .set_watermark(&mut tx, block(1, "genesis_hash"), 16)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn seed_records(db: &DynMintDatabase, records: Vec<EpochRecord>) {
        let mut store = EpochStore::new_empty();
        let mut tx = db.begin_transaction().await.unwrap();
        for record in records {
            store.append(&mut tx, record).await.unwrap();
        }
        tx.commit().await.unwrap();
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

    /// The dissolve re-stamp opens a second connection on this file; an in-memory DB can't serve that.
    async fn test_mint_file(path: &std::path::PathBuf) -> Arc<Mint> {
        let db = Arc::new(cdk_sqlite::MintSqliteDatabase::new(path).await.unwrap());
        let mint = MintBuilder::new(db.clone())
            .build_with_seed(db, &[7u8; 64])
            .await
            .unwrap();
        let mint = Arc::new(mint);
        mint.start().await.unwrap();
        mint
    }

    fn test_settings() -> EpochSettings {
        EpochSettings {
            pool_pubkey: POOL_PUBKEY.to_string(),
            // Resume never calls bitcoind; this just needs to parse.
            rpc_url: "http://127.0.0.1:0".to_string(),
            rpc_user: "user".to_string(),
            rpc_pass: "pass".to_string(),
            admin_listen: "127.0.0.1:0".to_string(),
            receive_script: bitcoin::ScriptBuf::new(),
            confirmation_depth: 1,
            poll_interval: Duration::from_secs(1),
            mint_db_path: std::path::PathBuf::from(":memory:"),
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

    async fn seed_unpaid_quote(mint: &Mint, unit: &CurrencyUnit, amount: u64) -> String {
        if mint
            .get_payment_processor(unit.clone(), ehash_method())
            .is_err()
        {
            let processor = Arc::new(EhashPaymentProcessor::new(unit.clone()));
            mint.register_payment_processor(
                unit.clone(),
                ehash_method(),
                MintMeltLimits::new(1, u64::MAX),
                processor as Arc<dyn MintPayment<Err = cdk::cdk_payment::Error> + Send + Sync>,
            )
            .await
            .unwrap();
        }

        let header_hash = "22".repeat(32);
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
        response.quote().to_string()
    }

    /// Bypasses any processor: a second, unregistered `EhashPaymentProcessor`
    /// has no consumer task, so `pay_ehash_quote` on it would never mark the quote paid.
    async fn seed_quote_paid_directly(mint: &Mint, unit: &CurrencyUnit, amount: u64) -> String {
        let header_hash = "33".repeat(32);
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
        let quote_id = response.quote().to_string();

        mint.pay_mint_quote_for_request_id(WaitPaymentResponse {
            payment_identifier: cdk::cdk_payment::PaymentIdentifier::CustomId(header_hash.clone()),
            payment_amount: cdk::Amount::new(amount, unit.clone()),
            payment_id: header_hash,
        })
        .await
        .unwrap();

        quote_id
    }

    #[tokio::test]
    async fn resume_restores_current_and_owing_but_not_settled_closed_epochs() {
        let mint = test_mint().await;
        seed_records(
            &mint.localstore(),
            vec![
                record(100, "hash_test_100_dissolved", EpochState::Dissolved),
                record(150, "hash_test_150_settled", EpochState::Final),
                record(200, "hash_test_200_owing", EpochState::Final),
                record(300, "hash_test_300_current", EpochState::Final),
            ],
        )
        .await;

        let owing_unit = CurrencyUnit::Custom("hash_test_200_owing".to_string().into());
        seed_paid_quote(&mint, &owing_unit, 10).await;

        let settings = test_settings();
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

        drop(manager);
    }

    #[tokio::test]
    async fn resume_keeps_the_epoch_before_a_provisional_current_quotable() {
        let mint = test_mint().await;
        seed_records(
            &mint.localstore(),
            vec![
                record(100, "hash_test_100_prev", EpochState::Final),
                record(200, "hash_test_200_provisional", EpochState::Provisional),
            ],
        )
        .await;

        let settings = test_settings();
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

        drop(manager);
    }

    #[tokio::test]
    async fn resume_keeps_the_whole_resumable_chain_quotable() {
        let mint = test_mint().await;
        seed_records(
            &mint.localstore(),
            vec![
                record(100, "hash_test_100_final", EpochState::Final),
                record(200, "hash_test_200_provisional", EpochState::Provisional),
                record(300, "hash_test_300_provisional", EpochState::Provisional),
            ],
        )
        .await;

        let settings = test_settings();
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

        drop(manager);
    }

    #[tokio::test]
    async fn resume_retires_an_epoch_excluded_from_the_register_set() {
        let mint = test_mint().await;
        seed_records(
            &mint.localstore(),
            vec![
                record(100, "hash_test_100_stale", EpochState::Final),
                record(200, "hash_test_200_current", EpochState::Final),
            ],
        )
        .await;

        let stale_unit = CurrencyUnit::Custom("hash_test_100_stale".to_string().into());

        // Simulates a gate left by an earlier crashed process, bypassing this run's register set.
        let processor = Arc::new(EhashPaymentProcessor::new(stale_unit.clone()));
        mint.register_payment_processor(
            stale_unit.clone(),
            ehash_method(),
            MintMeltLimits::new(1, u64::MAX),
            processor as Arc<dyn MintPayment<Err = cdk::cdk_payment::Error> + Send + Sync>,
        )
        .await
        .unwrap();

        let settings = test_settings();
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

    #[test]
    fn plan_block_nothing_when_rewalking_an_untouched_provisional_boundary() {
        let records = vec![record(100, "u100", EpochState::Provisional)];
        let mut r = records;
        r[0].block_hash = Some("hash_a".into());
        assert_eq!(plan_block(&r, 100, "hash_a", 0), BlockAction::Nothing);
    }

    #[test]
    fn plan_block_remine_when_provisional_boundary_replaced_by_a_paying_block() {
        let mut r = vec![record(100, "u100", EpochState::Provisional)];
        r[0].block_hash = Some("hash_a".into());
        assert_eq!(
            plan_block(&r, 100, "hash_b", 500),
            BlockAction::ReMine { unit: "u100".into() }
        );
    }

    #[test]
    fn plan_block_dissolve_when_provisional_boundary_replaced_with_no_payment() {
        let mut r = vec![record(100, "u100", EpochState::Provisional)];
        r[0].block_hash = Some("hash_a".into());
        assert_eq!(
            plan_block(&r, 100, "hash_b", 0),
            BlockAction::Dissolve { unit: "u100".into() }
        );
    }

    #[test]
    fn plan_block_open_when_reward_with_no_existing_record() {
        let r = vec![record(100, "u100", EpochState::Final)];
        assert_eq!(plan_block(&r, 200, "hash_c", 500), BlockAction::Open);
    }

    #[test]
    fn plan_block_dissolved_record_at_same_height_is_left_alone_and_opens_anew() {
        let r = vec![record(100, "u100_1", EpochState::Dissolved)];
        assert_eq!(plan_block(&r, 100, "hash_c", 500), BlockAction::Open);
    }

    #[test]
    fn plan_block_nothing_when_rewalking_a_final_boundary() {
        let mut r = vec![record(100, "u100", EpochState::Final)];
        r[0].block_hash = Some("hash_a".into());
        assert_eq!(plan_block(&r, 100, "hash_a", 500), BlockAction::Nothing);
    }

    #[test]
    fn plan_block_reorg_past_final_when_a_reward_replaces_a_final_boundary() {
        let mut r = vec![record(100, "u100", EpochState::Final)];
        r[0].block_hash = Some("hash_a".into());
        assert_eq!(
            plan_block(&r, 100, "hash_b", 500),
            BlockAction::ReorgPastFinal { unit: "u100".into() }
        );
    }

    #[test]
    fn plan_block_opens_a_new_epoch_when_a_reward_lands_on_a_hashless_final_record() {
        let r = vec![record(100, "u100_manual", EpochState::Final)];
        assert_eq!(plan_block(&r, 100, "hash_b", 500), BlockAction::Open);
    }

    #[test]
    fn plan_block_nothing_when_no_reward_and_no_provisional_record() {
        let r = vec![record(100, "u100", EpochState::Final)];
        assert_eq!(plan_block(&r, 200, "hash_c", 0), BlockAction::Nothing);
    }

    #[test]
    fn due_for_finality_orders_oldest_first_and_excludes_non_canonical() {
        let mut older = record(100, "older", EpochState::Provisional);
        older.block_hash = Some("h100".into());
        let mut newer = record(200, "newer", EpochState::Provisional);
        newer.block_hash = Some("h200".into());
        let mut stale = record(150, "stale", EpochState::Provisional);
        stale.block_hash = Some("h150".into());
        let records = vec![older, stale, newer];

        let due = due_for_finality(&records, 300, 3, |r| r.unit != "stale");
        assert_eq!(due, vec!["older".to_string(), "newer".to_string()]);
    }

    #[test]
    fn due_for_finality_boundary_is_exactly_at_and_one_below() {
        let mut r = record(100, "u", EpochState::Provisional);
        r.block_hash = Some("h".into());
        let records = vec![r];

        assert_eq!(
            due_for_finality(&records, 102, 3, |_| true),
            vec!["u".to_string()]
        );
        assert_eq!(due_for_finality(&records, 101, 3, |_| true), Vec::<String>::new());
    }

    #[test]
    fn due_for_finality_only_considers_provisional_records() {
        let records = vec![record(100, "final", EpochState::Final)];
        assert_eq!(
            due_for_finality(&records, 1000, 1, |_| true),
            Vec::<String>::new()
        );
    }

    #[test]
    fn resumable_chain_and_due_for_finality_use_chronological_order_after_a_reorg_rescan() {
        let genesis = record(0, "genesis", EpochState::Final);
        let mut h95 = record(95, "h95", EpochState::Provisional);
        h95.block_hash = Some("hash95".into());
        let mut h100 = record(100, "h100", EpochState::Provisional);
        h100.block_hash = Some("hash100".into());
        let mut h110 = record(110, "h110", EpochState::Provisional);
        h110.block_hash = Some("hash110".into());
        let chronological = vec![genesis, h95, h100, h110];

        let chain = resumable_chain(&chronological);
        assert_eq!(
            chain,
            HashSet::from([
                "genesis".to_string(),
                "h95".to_string(),
                "h100".to_string(),
                "h110".to_string(),
            ]),
            "the resumable chain reaches all the way back to genesis through the stacked provisionals"
        );

        let due = due_for_finality(&chronological, 1000, 1, |_| true);
        assert_eq!(
            due,
            vec!["h95".to_string(), "h100".to_string(), "h110".to_string()],
            "finality order follows height, not insertion order"
        );
    }

    #[tokio::test]
    async fn rollback_point_prefers_the_newest_canonical_entry() {
        let recent = vec![block(100, "h100"), block(101, "h101"), block(102, "h102")];
        let point = rollback_point(&recent, 102, |b| async move { Ok(b.height != 102) })
            .await
            .unwrap();
        assert_eq!(point, Some(block(101, "h101")));
    }

    #[tokio::test]
    async fn rollback_point_none_when_nothing_is_canonical() {
        let recent = vec![block(100, "h100"), block(101, "h101")];
        let point = rollback_point(&recent, 101, |_| async { Ok(false) })
            .await
            .unwrap();
        assert_eq!(point, None);
    }

    #[tokio::test]
    async fn rollback_point_skips_entries_above_the_tip_without_consulting_is_canonical() {
        let recent: Vec<ScannedBlock> = (10..20).map(|h| block(h, &format!("h{h}"))).collect();
        let tip = 15;
        let point = rollback_point(&recent, tip, |b| async move {
            assert!(
                b.height <= tip,
                "must never ask is_canonical about height {} above tip {tip}",
                b.height
            );
            Ok(true)
        })
        .await
        .unwrap();
        assert_eq!(point, Some(block(15, "h15")), "the newest canonical entry at or below the tip");
    }

    #[tokio::test]
    async fn with_store_tx_leaves_the_store_unchanged_when_the_closure_errors() {
        let mint = test_mint().await;
        seed_genesis_store(&mint.localstore(), "hash_test_genesis_with_store_tx").await;
        let settings = test_settings();
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .unwrap();

        let before = {
            let store = manager.store.lock().await;
            store.non_dissolved().len()
        };

        let result: Result<()> = manager
            .with_store_tx(|store, tx| {
                Box::pin(async move {
                    store
                        .append(tx, record(999, "hash_test_should_not_persist", EpochState::Final))
                        .await?;
                    Err(anyhow!("simulated failure before commit"))
                })
            })
            .await;
        assert!(result.is_err(), "a failing closure must propagate its error");

        let after_count = {
            let store = manager.store.lock().await;
            store.non_dissolved().len()
        };
        assert_eq!(before, after_count, "the locked store must be unchanged");
        assert!(
            !manager.store.lock().await.unit_taken("hash_test_should_not_persist"),
            "the appended record must not be visible in memory"
        );

        let reloaded = EpochStore::load(&mint.localstore()).await.unwrap().unwrap();
        assert!(
            !reloaded.unit_taken("hash_test_should_not_persist"),
            "an uncommitted write must not be visible to a fresh load either"
        );
    }

    #[tokio::test]
    async fn load_or_genesis_refuses_when_a_legacy_epochs_json_file_exists() {
        let db_path = temp_db_path("migration-guard");
        let _ = std::fs::remove_file(&db_path);
        let legacy = db_path.parent().unwrap().join("epochs.json");
        std::fs::write(&legacy, "{}").unwrap();

        let mint = test_mint_file(&db_path).await;
        let mut settings = test_settings();
        settings.mint_db_path = db_path.clone();

        let result = EpochManager::load_or_genesis(mint, settings).await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("genesis must refuse to run while a legacy epochs.json file exists"),
        };
        let message = err.to_string();
        assert!(
            message.contains("epochs.json"),
            "the error must name the offending file: {message}"
        );

        let _ = std::fs::remove_file(&legacy);
        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn open_epoch_refuses_a_final_open_while_current_is_provisional() {
        let mint = test_mint().await;
        seed_genesis_store(&mint.localstore(), "hash_test_genesis_provisional_refuse").await;

        let settings = test_settings();
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .unwrap();

        manager
            .open_epoch(
                100,
                Some("hash_a".into()),
                Some(500),
                EpochSource::Reward,
                EpochState::Provisional,
            )
            .await
            .unwrap();

        let record_count_before = {
            let store = manager.store.lock().await;
            store.non_dissolved().len()
        };

        let result = manager
            .open_epoch(200, None, None, EpochSource::Manual, EpochState::Final)
            .await;
        assert!(
            result.is_err(),
            "a manual Final open must be refused while current is provisional"
        );
        assert!(
            result
                .unwrap_err()
                .downcast_ref::<ProvisionalCurrent>()
                .is_some(),
            "the refusal must be the typed ProvisionalCurrent error"
        );

        let record_count_after = {
            let store = manager.store.lock().await;
            store.non_dissolved().len()
        };
        assert_eq!(
            record_count_before, record_count_after,
            "a refused open must create no record"
        );
    }

    #[tokio::test]
    async fn finalize_flips_state_pays_seeded_quote_and_retires_previous_unit() {
        let mint = test_mint().await;
        seed_genesis_store(&mint.localstore(), "hash_test_genesis_finalize").await;

        let settings = test_settings();
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .unwrap();
        let genesis_unit = manager.current_epoch().await.unit.clone();

        let record = manager
            .open_epoch(
                100,
                Some("hash_a".into()),
                Some(500),
                EpochSource::Reward,
                EpochState::Provisional,
            )
            .await
            .unwrap();

        let unit = CurrencyUnit::Custom(record.unit.clone().into());
        let quote_id = seed_unpaid_quote(&mint, &unit, 10).await;

        manager.finalize(&record.unit).await.unwrap();

        let quotes = mint.mint_quotes().await.unwrap();
        let seeded = quotes.iter().find(|q| q.id.to_string() == quote_id).unwrap();
        assert_eq!(seeded.amount_paid().value(), 10, "seeded quote must be paid at finality");

        assert_eq!(manager.current_epoch().await.state, EpochState::Final);

        let reloaded = EpochStore::load(&mint.localstore()).await.unwrap().unwrap();
        assert_eq!(
            reloaded.record(&record.unit).unwrap().state,
            EpochState::Final,
            "the Final flip must be visible from a fresh load, not just in memory"
        );

        let mint_info = mint.mint_info().await.unwrap();
        let genesis_currency = CurrencyUnit::Custom(genesis_unit.into());
        assert!(
            mint_info
                .nuts
                .nut04
                .get_settings(&genesis_currency, &ehash_method())
                .is_none(),
            "the previous unit's quote-creation settings must be retired"
        );
        assert!(
            mint.get_payment_processor(genesis_currency, ehash_method()).is_ok(),
            "the previous unit's processor-map entry must remain (retire, not deregister)"
        );
    }

    #[tokio::test]
    async fn finalize_leaves_the_record_provisional_and_unpaid_when_pay_fails() {
        let mint = test_mint().await;
        seed_genesis_store(&mint.localstore(), "hash_test_genesis_finalize_retry").await;

        let settings = test_settings();
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .unwrap();

        let record = manager
            .open_epoch(
                100,
                Some("hash_a".into()),
                Some(500),
                EpochSource::Reward,
                EpochState::Provisional,
            )
            .await
            .unwrap();

        let unit = CurrencyUnit::Custom(record.unit.clone().into());
        let quote_id = seed_unpaid_quote(&mint, &unit, 10).await;

        manager.set_pay_hook(|| Err(anyhow!("simulated pay failure")));
        let result = manager.finalize(&record.unit).await;
        assert!(result.is_err(), "a failing pay must fail finalize");

        // Checked via a fresh load, not memory: pay-before-persist must hold in the database itself.
        let reloaded = EpochStore::load(&mint.localstore()).await.unwrap().unwrap();
        assert_eq!(
            reloaded.record(&record.unit).unwrap().state,
            EpochState::Provisional,
            "a failed pay must leave the record provisional for the next tick to retry"
        );

        let quotes = mint.mint_quotes().await.unwrap();
        let seeded = quotes.iter().find(|q| q.id.to_string() == quote_id).unwrap();
        assert_eq!(seeded.amount_paid().value(), 0, "the quote must remain unpaid");

        manager.clear_pay_hook();
    }

    #[tokio::test]
    async fn dissolve_restamps_and_pays_an_unpaid_quote_onto_the_previous_unit() {
        let db_path = temp_db_path("dissolve");
        let _ = std::fs::remove_file(&db_path);
        let mint = test_mint_file(&db_path).await;
        seed_genesis_store(&mint.localstore(), "hash_test_genesis_dissolve").await;

        let mut settings = test_settings();
        settings.mint_db_path = db_path.clone();
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .unwrap();
        let genesis_unit = manager.current_epoch().await.unit.clone();

        let record = manager
            .open_epoch(
                100,
                Some("hash_a".into()),
                Some(500),
                EpochSource::Reward,
                EpochState::Provisional,
            )
            .await
            .unwrap();

        let unit = CurrencyUnit::Custom(record.unit.clone().into());
        let quote_id = seed_unpaid_quote(&mint, &unit, 10).await;

        manager.dissolve(&record.unit).await.unwrap();

        let quotes = mint.mint_quotes().await.unwrap();
        let moved = quotes.iter().find(|q| q.id.to_string() == quote_id).unwrap();
        assert_eq!(moved.unit.to_string(), genesis_unit, "quote must move to the previous unit");
        assert_eq!(moved.amount_paid().value(), 10, "re-stamped quote must be paid");

        let current = manager.current_epoch().await;
        assert_eq!(current.unit, genesis_unit, "current must roll back to the previous epoch");
        drop(current);

        let dissolved = {
            let store = manager.store.lock().await;
            store.record(&record.unit).unwrap()
        };
        assert_eq!(dissolved.state, EpochState::Dissolved);

        let mint_info = mint.mint_info().await.unwrap();
        let genesis_currency = CurrencyUnit::Custom(genesis_unit.into());
        assert!(
            mint_info
                .nuts
                .nut04
                .get_settings(&genesis_currency, &ehash_method())
                .is_some(),
            "the previous unit must still have nut04 settings after dissolve"
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn dissolve_refuses_and_mutates_nothing_when_a_quote_is_already_paid() {
        let db_path = temp_db_path("dissolve-invariant");
        let _ = std::fs::remove_file(&db_path);
        let mint = test_mint_file(&db_path).await;
        seed_genesis_store(&mint.localstore(), "hash_test_genesis_dissolve_invariant").await;

        let mut settings = test_settings();
        settings.mint_db_path = db_path.clone();
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .unwrap();

        let record = manager
            .open_epoch(
                100,
                Some("hash_a".into()),
                Some(500),
                EpochSource::Reward,
                EpochState::Provisional,
            )
            .await
            .unwrap();

        let unit = CurrencyUnit::Custom(record.unit.clone().into());
        seed_quote_paid_directly(&mint, &unit, 10).await;

        let result = manager.dissolve(&record.unit).await;
        assert!(result.is_err(), "dissolving a unit with a paid quote must fail");

        let state_after = {
            let store = manager.store.lock().await;
            store.record(&record.unit).unwrap().state
        };
        assert_eq!(
            state_after,
            EpochState::Provisional,
            "a failed dissolve must not flip the record"
        );

        let quotes = mint.mint_quotes().await.unwrap();
        assert!(
            quotes.iter().any(|q| q.unit == unit && q.amount_paid().value() == 10),
            "the paid quote must be untouched"
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn dissolve_retries_cleanly_after_a_failed_pay_leaves_the_record_provisional() {
        let db_path = temp_db_path("dissolve-retry");
        let _ = std::fs::remove_file(&db_path);
        let mint = test_mint_file(&db_path).await;
        seed_genesis_store(&mint.localstore(), "hash_test_genesis_dissolve_retry").await;

        let mut settings = test_settings();
        settings.mint_db_path = db_path.clone();
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .unwrap();
        let prev_unit = manager.current_epoch().await.unit.clone();

        let record = manager
            .open_epoch(
                100,
                Some("hash_a".into()),
                Some(500),
                EpochSource::Reward,
                EpochState::Provisional,
            )
            .await
            .unwrap();

        let unit = CurrencyUnit::Custom(record.unit.clone().into());
        let quote_id = seed_unpaid_quote(&mint, &unit, 10).await;

        // The re-stamp is idempotent and already ran; only the pay step fails here.
        manager.set_pay_hook(|| Err(anyhow!("simulated pay failure")));
        let result = manager.dissolve(&record.unit).await;
        assert!(result.is_err(), "a failing pay must fail dissolve");

        let state_after_failure = {
            let store = manager.store.lock().await;
            store.record(&record.unit).unwrap().state
        };
        assert_eq!(
            state_after_failure,
            EpochState::Provisional,
            "a failed pay must leave the record provisional for the next tick to retry"
        );

        manager.clear_pay_hook();
        manager.dissolve(&record.unit).await.unwrap();

        let dissolved = {
            let store = manager.store.lock().await;
            store.record(&record.unit).unwrap()
        };
        assert_eq!(dissolved.state, EpochState::Dissolved);

        let quotes = mint.mint_quotes().await.unwrap();
        let moved = quotes.iter().find(|q| q.id.to_string() == quote_id).unwrap();
        assert_eq!(moved.unit.to_string(), prev_unit, "the quote must have moved to the previous unit");
        assert_eq!(moved.amount_paid().value(), 10, "the quote must be paid after the retry");

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn dissolve_uses_chronological_not_insertion_order_for_the_previous_epoch() {
        let db_path = temp_db_path("dissolve-chronology");
        let _ = std::fs::remove_file(&db_path);
        let mint = test_mint_file(&db_path).await;

        {
            let db = mint.localstore();
            let mut store = EpochStore::new_empty();
            let mut tx = db.begin_transaction().await.unwrap();

            let mut genesis = record(0, "genesis", EpochState::Final);
            genesis.keyset_id = valid_keyset_id(0);
            store.append(&mut tx, genesis).await.unwrap();

            let mut r100 = record(100, "h100", EpochState::Provisional);
            r100.keyset_id = valid_keyset_id(100);
            r100.block_hash = Some("hash100".into());
            store.append(&mut tx, r100).await.unwrap();

            let mut r110 = record(110, "h110", EpochState::Provisional);
            r110.keyset_id = valid_keyset_id(110);
            r110.block_hash = Some("hash110".into());
            store.append(&mut tx, r110).await.unwrap();

            // Inserted last, even though its height is the smallest.
            let mut r95 = record(95, "h95", EpochState::Provisional);
            r95.keyset_id = valid_keyset_id(95);
            r95.block_hash = Some("hash95".into());
            store.append(&mut tx, r95).await.unwrap();

            tx.commit().await.unwrap();
        }

        let mut settings = test_settings();
        settings.mint_db_path = db_path.clone();
        let manager = EpochManager::load_or_genesis(mint.clone(), settings)
            .await
            .unwrap();

        // `current()` is the last-inserted non-dissolved record (95), not
        // the tallest height (110) — by design, during this rescan window.
        assert_eq!(manager.current_epoch().await.unit, "h95");

        let unit_100 = CurrencyUnit::Custom("h100".to_string().into());
        let quote_id = seed_unpaid_quote(&mint, &unit_100, 10).await;

        manager.dissolve("h100").await.unwrap();

        let quotes = mint.mint_quotes().await.unwrap();
        let moved = quotes.iter().find(|q| q.id.to_string() == quote_id).unwrap();
        assert_eq!(
            moved.unit.to_string(),
            "h95",
            "100's chronological predecessor is 95, not genesis (insertion order would say genesis)"
        );
        assert_eq!(
            moved.amount_paid().value(),
            0,
            "95 is itself still provisional, so the re-stamped quote waits for 95's own finality"
        );

        let dissolved_100 = {
            let store = manager.store.lock().await;
            store.record("h100").unwrap()
        };
        assert_eq!(dissolved_100.state, EpochState::Dissolved);

        // With 100 dissolved, 110's chronological predecessor is now 95 too.
        manager.dissolve("h110").await.unwrap();
        let dissolved_110 = {
            let store = manager.store.lock().await;
            store.record("h110").unwrap()
        };
        assert_eq!(dissolved_110.state, EpochState::Dissolved);

        let _ = std::fs::remove_file(&db_path);
    }
}
