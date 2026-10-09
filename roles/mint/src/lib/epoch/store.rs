use anyhow::{anyhow, Context, Result};
use cdk_common::database::DynMintDatabase;
use serde::{Deserialize, Serialize};

/// Transaction type every mutating `EpochStore` method takes: the caller
/// owns begin/commit, so a hashpool state change can share one database
/// transaction with the cdk change it pairs with (e.g. paying a quote).
pub type Tx = cdk_common::database::DynMintTransaction;

const PRIMARY_NAMESPACE: &str = "hashpool";
const SECONDARY_NAMESPACE: &str = "epochs";
const RECORDS_KEY: &str = "records";
const WATCHER_KEY: &str = "watcher";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpochState {
    /// Boundary block seen but not yet confirmed to depth D. Quotes created
    /// while provisional are held unpaid.
    Provisional,
    Final,
    /// Boundary orphaned before finality; quotes were re-stamped to the
    /// previous epoch. Kept for audit.
    Dissolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpochSource {
    Genesis,
    Manual,
    Reward,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRecord {
    /// Block height that opened the epoch (current tip for genesis/manual).
    pub height: u64,
    /// Full currency unit string, e.g. `hash_<pool>_<height>`.
    pub unit: String,
    pub keyset_id: String,
    /// Hash of the boundary block; None for genesis/manual epochs.
    pub block_hash: Option<String>,
    /// Coinbase value paid to the mint's script; None for genesis/manual.
    pub reward_sats: Option<u64>,
    pub state: EpochState,
    pub source: EpochSource,
    pub opened_at: u64,
}

/// One block the watcher has processed: its height and the hash it had when
/// processed (not necessarily the hash currently at that height).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedBlock {
    pub height: u64,
    pub hash: String,
}

/// The `watcher` KV value: the last processed block and the trailing window
/// used to detect and resync reorgs.
#[derive(Debug, Default, Serialize, Deserialize)]
struct WatcherState {
    #[serde(default)]
    watermark: Option<ScannedBlock>,
    #[serde(default)]
    recent: Vec<ScannedBlock>,
}

/// In-memory epoch log, backed by two keys (`records`, `watcher`) in cdk's
/// key-value store inside the mint's own database — primary namespace
/// `hashpool`, secondary namespace `epochs`. Holds no database handle: every
/// mutating method takes the caller's transaction, so a hashpool state
/// change can commit atomically with the cdk change it pairs with. The
/// caller owns begin/commit; see `EpochManager::with_store_tx` for the
/// clone-mutate-commit-then-assign pattern that keeps memory and the
/// database from disagreeing when a commit fails.
#[derive(Debug, Clone, Default)]
pub struct EpochStore {
    records: Vec<EpochRecord>,
    watermark: Option<ScannedBlock>,
    recent: Vec<ScannedBlock>,
}

impl EpochStore {
    pub fn new_empty() -> Self {
        Self::default()
    }

    /// `None` when the `records` key does not exist yet (nothing has ever
    /// been written — genesis has not run).
    pub async fn load(db: &DynMintDatabase) -> Result<Option<Self>> {
        let records_bytes = db
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, RECORDS_KEY)
            .await
            .map_err(|e| anyhow!("reading epoch records: {e}"))?;
        let records_bytes = match records_bytes {
            Some(b) => b,
            None => return Ok(None),
        };
        let records: Vec<EpochRecord> =
            serde_json::from_slice(&records_bytes).context("parsing epoch records")?;

        let watcher_bytes = db
            .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, WATCHER_KEY)
            .await
            .map_err(|e| anyhow!("reading watcher state: {e}"))?;
        let watcher: WatcherState = match watcher_bytes {
            Some(b) => serde_json::from_slice(&b).context("parsing watcher state")?,
            None => WatcherState::default(),
        };

        Ok(Some(Self {
            records,
            watermark: watcher.watermark,
            recent: watcher.recent,
        }))
    }

    pub fn current(&self) -> Option<&EpochRecord> {
        self.records
            .iter()
            .rev()
            .find(|r| r.state != EpochState::Dissolved)
    }

    /// Oldest first, by insertion order (not chronology — see
    /// `chronological`). A dissolved record must never resolve again.
    pub fn non_dissolved(&self) -> Vec<EpochRecord> {
        self.records
            .iter()
            .filter(|r| r.state != EpochState::Dissolved)
            .cloned()
            .collect()
    }

    /// Non-dissolved records in chronological order: ascending by
    /// `(height, insertion index)`, not by insertion order alone. A reorg
    /// rescan can append a lower-height record after an already-inserted
    /// higher one — the watermark rolls back, then walks forward again, so
    /// a replacement branch's reward lands after the orphaned branch's
    /// still-provisional records — so insertion order is not chronology.
    /// `previous_non_dissolved` and callers that need "oldest first" for
    /// real (`resumable_chain`, the finality pass) use this, never
    /// `non_dissolved`.
    pub fn chronological(&self) -> Vec<EpochRecord> {
        let mut indexed: Vec<(usize, &EpochRecord)> = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.state != EpochState::Dissolved)
            .collect();
        indexed.sort_by_key(|(idx, r)| (r.height, *idx));
        indexed.into_iter().map(|(_, r)| r.clone()).collect()
    }

    pub fn unit_taken(&self, unit: &str) -> bool {
        self.records.iter().any(|r| r.unit == unit)
    }

    /// The record for `unit`, if any.
    pub fn record(&self, unit: &str) -> Option<EpochRecord> {
        self.records.iter().find(|r| r.unit == unit).cloned()
    }

    /// The non-dissolved record with the greatest `(height, insertion
    /// index)` strictly below `unit`'s own — chronologically previous, not
    /// positionally previous (see `chronological`).
    pub fn previous_non_dissolved(&self, unit: &str) -> Option<EpochRecord> {
        let chrono = self.chronological();
        let pos = chrono.iter().position(|r| r.unit == unit)?;
        pos.checked_sub(1).map(|i| chrono[i].clone())
    }

    pub fn watermark(&self) -> Option<&ScannedBlock> {
        self.watermark.as_ref()
    }

    pub fn recent(&self) -> &[ScannedBlock] {
        &self.recent
    }

    /// Number of records already at this height (drives the unit-name suffix).
    pub fn count_at_height(&self, height: u64) -> u32 {
        self.records.iter().filter(|r| r.height == height).count() as u32
    }

    /// Appends `record`, writing the whole list into `tx`. `self` changes
    /// only after the write succeeds, so a caller that does not commit `tx`
    /// never has to unwind this method's effect by hand.
    pub async fn append(&mut self, tx: &mut Tx, record: EpochRecord) -> Result<()> {
        let mut records = self.records.clone();
        records.push(record);
        write_records(tx, &records).await?;
        self.records = records;
        Ok(())
    }

    /// Mutates one record and writes the whole list into `tx`. Same
    /// write-before-assign rule as `append`.
    pub async fn update_record(
        &mut self,
        tx: &mut Tx,
        unit: &str,
        f: impl FnOnce(&mut EpochRecord),
    ) -> Result<()> {
        let mut records = self.records.clone();
        let idx = records
            .iter()
            .position(|r| r.unit == unit)
            .ok_or_else(|| anyhow!("no epoch record for unit {unit}"))?;
        f(&mut records[idx]);
        write_records(tx, &records).await?;
        self.records = records;
        Ok(())
    }

    /// Advances the watermark: pushes `block` onto `recent`, trims the
    /// front down to `cap` entries, and writes both into `tx`.
    pub async fn set_watermark(
        &mut self,
        tx: &mut Tx,
        block: ScannedBlock,
        cap: usize,
    ) -> Result<()> {
        let mut recent = self.recent.clone();
        recent.push(block.clone());
        if recent.len() > cap {
            let excess = recent.len() - cap;
            recent.drain(0..excess);
        }
        let watermark = Some(block);
        write_watcher(tx, &watermark, &recent).await?;
        self.watermark = watermark;
        self.recent = recent;
        Ok(())
    }

    /// Truncates `recent` to entries at or below `height` and sets the
    /// watermark to the last remaining entry (`None` if none remain).
    pub async fn rollback_to(&mut self, tx: &mut Tx, height: u64) -> Result<()> {
        let mut recent = self.recent.clone();
        recent.retain(|b| b.height <= height);
        let watermark = recent.last().cloned();
        write_watcher(tx, &watermark, &recent).await?;
        self.watermark = watermark;
        self.recent = recent;
        Ok(())
    }
}

async fn write_records(tx: &mut Tx, records: &[EpochRecord]) -> Result<()> {
    let bytes = serde_json::to_vec(records).context("serializing epoch records")?;
    tx.kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, RECORDS_KEY, &bytes)
        .await
        .map_err(|e| anyhow!("writing epoch records: {e}"))
}

async fn write_watcher(
    tx: &mut Tx,
    watermark: &Option<ScannedBlock>,
    recent: &[ScannedBlock],
) -> Result<()> {
    let state = WatcherState {
        watermark: watermark.clone(),
        recent: recent.to_vec(),
    };
    let bytes = serde_json::to_vec(&state).context("serializing watcher state")?;
    tx.kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, WATCHER_KEY, &bytes)
        .await
        .map_err(|e| anyhow!("writing watcher state: {e}"))
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    async fn test_db() -> DynMintDatabase {
        Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap())
    }

    #[tokio::test]
    async fn round_trips_through_a_committed_transaction() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();

        let mut tx = db.begin_transaction().await.unwrap();
        store
            .append(&mut tx, record(100, "hash_ab_100", EpochState::Final))
            .await
            .unwrap();
        store
            .append(&mut tx, record(105, "hash_ab_105", EpochState::Final))
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(store.current().unwrap().unit, "hash_ab_105");
        assert!(store.unit_taken("hash_ab_100"));
        assert_eq!(store.count_at_height(105), 1);

        let reloaded = EpochStore::load(&db).await.unwrap().unwrap();
        assert_eq!(reloaded.current().unwrap().unit, "hash_ab_105");
    }

    #[tokio::test]
    async fn load_returns_none_on_an_empty_database() {
        let db = test_db().await;
        assert!(EpochStore::load(&db).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_uncommitted_transaction_is_not_visible_to_a_fresh_load() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();

        let mut tx = db.begin_transaction().await.unwrap();
        store
            .append(&mut tx, record(100, "hash_ab_100", EpochState::Final))
            .await
            .unwrap();
        drop(tx); // never committed

        let reloaded = EpochStore::load(&db).await.unwrap();
        assert!(
            reloaded.is_none(),
            "an uncommitted write must not be visible to a fresh load"
        );
    }

    #[tokio::test]
    async fn load_defaults_watermark_and_recent_when_the_watcher_key_is_absent() {
        let db = test_db().await;
        let mut tx = db.begin_transaction().await.unwrap();
        let bytes = serde_json::to_vec(&vec![record(100, "a", EpochState::Final)]).unwrap();
        tx.kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, RECORDS_KEY, &bytes)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let loaded = EpochStore::load(&db).await.unwrap().unwrap();
        assert!(loaded.watermark().is_none());
        assert!(loaded.recent().is_empty());
    }

    #[tokio::test]
    async fn non_dissolved_excludes_dissolved_records_only() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();
        let mut tx = db.begin_transaction().await.unwrap();
        store
            .append(&mut tx, record(100, "hash_ab_100", EpochState::Final))
            .await
            .unwrap();
        store
            .append(&mut tx, record(105, "hash_ab_105", EpochState::Dissolved))
            .await
            .unwrap();
        store
            .append(&mut tx, record(110, "hash_ab_110", EpochState::Final))
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let non_dissolved = store.non_dissolved();
        let units: Vec<&str> = non_dissolved.iter().map(|r| r.unit.as_str()).collect();
        assert_eq!(units, vec!["hash_ab_100", "hash_ab_110"]);
    }

    #[tokio::test]
    async fn previous_non_dissolved_skips_dissolved_records() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();
        let mut tx = db.begin_transaction().await.unwrap();
        store.append(&mut tx, record(100, "a", EpochState::Final)).await.unwrap();
        store.append(&mut tx, record(105, "b", EpochState::Dissolved)).await.unwrap();
        store.append(&mut tx, record(110, "c", EpochState::Provisional)).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(store.previous_non_dissolved("c").unwrap().unit, "a");
        assert!(store.previous_non_dissolved("a").is_none());
    }

    #[tokio::test]
    async fn chronological_and_previous_non_dissolved_use_height_not_insertion_order() {
        // A reorg rescan can append a lower-height record after
        // still-provisional higher ones: provisional rewards at 100 and
        // 110 on branch A, then branch B's reward at 95 is opened and
        // appended last, even though 95 < 100 < 110.
        let db = test_db().await;
        let mut store = EpochStore::new_empty();
        let mut tx = db.begin_transaction().await.unwrap();
        store.append(&mut tx, record(0, "genesis", EpochState::Final)).await.unwrap();
        store.append(&mut tx, record(100, "h100", EpochState::Provisional)).await.unwrap();
        store.append(&mut tx, record(110, "h110", EpochState::Provisional)).await.unwrap();
        store.append(&mut tx, record(95, "h95", EpochState::Provisional)).await.unwrap();
        tx.commit().await.unwrap();

        let chrono: Vec<String> = store.chronological().into_iter().map(|r| r.unit).collect();
        assert_eq!(chrono, vec!["genesis", "h95", "h100", "h110"]);

        assert_eq!(store.previous_non_dissolved("h95").unwrap().unit, "genesis");
        assert_eq!(store.previous_non_dissolved("h100").unwrap().unit, "h95");
        assert_eq!(store.previous_non_dissolved("h110").unwrap().unit, "h100");
    }

    #[tokio::test]
    async fn previous_non_dissolved_skips_a_dissolved_record_in_chronological_order() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();
        let mut tx = db.begin_transaction().await.unwrap();
        store.append(&mut tx, record(0, "genesis", EpochState::Final)).await.unwrap();
        store.append(&mut tx, record(100, "h100", EpochState::Dissolved)).await.unwrap();
        store.append(&mut tx, record(110, "h110", EpochState::Provisional)).await.unwrap();
        store.append(&mut tx, record(95, "h95", EpochState::Provisional)).await.unwrap();
        tx.commit().await.unwrap();

        // h100 is dissolved, so h110's chronological predecessor is h95.
        assert_eq!(store.previous_non_dissolved("h110").unwrap().unit, "h95");
    }

    #[tokio::test]
    async fn update_record_changes_the_record_and_persists() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();

        let mut tx = db.begin_transaction().await.unwrap();
        store
            .append(&mut tx, record(100, "a", EpochState::Provisional))
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut tx = db.begin_transaction().await.unwrap();
        store
            .update_record(&mut tx, "a", |r| r.state = EpochState::Final)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(store.record("a").unwrap().state, EpochState::Final);

        let reloaded = EpochStore::load(&db).await.unwrap().unwrap();
        assert_eq!(reloaded.record("a").unwrap().state, EpochState::Final);
    }

    #[tokio::test]
    async fn watermark_and_recent_round_trip_through_a_transaction() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();
        assert!(store.watermark().is_none());
        assert!(store.recent().is_empty());

        let mut tx = db.begin_transaction().await.unwrap();
        // `load` treats an absent `records` key as "nothing ever written"
        // (pre-genesis); seed one record so this round-trips like real
        // usage, where a watermark write never precedes genesis.
        store.append(&mut tx, record(1, "a", EpochState::Final)).await.unwrap();
        store.set_watermark(&mut tx, block(100, "h100"), 16).await.unwrap();
        store.set_watermark(&mut tx, block(101, "h101"), 16).await.unwrap();
        tx.commit().await.unwrap();

        let reloaded = EpochStore::load(&db).await.unwrap().unwrap();
        assert_eq!(reloaded.watermark(), Some(&block(101, "h101")));
        assert_eq!(reloaded.recent(), &[block(100, "h100"), block(101, "h101")]);
    }

    #[tokio::test]
    async fn set_watermark_trims_to_the_cap() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();
        for h in 0..5u64 {
            let mut tx = db.begin_transaction().await.unwrap();
            store
                .set_watermark(&mut tx, block(h, &format!("h{h}")), 3)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }

        assert_eq!(
            store.recent(),
            &[block(2, "h2"), block(3, "h3"), block(4, "h4")]
        );
        assert_eq!(store.watermark(), Some(&block(4, "h4")));
    }

    #[tokio::test]
    async fn rollback_to_truncates_recent_and_moves_the_watermark_back() {
        let db = test_db().await;
        let mut store = EpochStore::new_empty();
        for (h, hash) in [(100u64, "h100"), (101, "h101"), (102, "h102")] {
            let mut tx = db.begin_transaction().await.unwrap();
            store.set_watermark(&mut tx, block(h, hash), 16).await.unwrap();
            tx.commit().await.unwrap();
        }

        let mut tx = db.begin_transaction().await.unwrap();
        store.rollback_to(&mut tx, 101).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(store.recent(), &[block(100, "h100"), block(101, "h101")]);
        assert_eq!(store.watermark(), Some(&block(101, "h101")));
    }
}
