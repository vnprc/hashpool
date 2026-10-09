use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    records: Vec<EpochRecord>,
    #[serde(default)]
    watermark: Option<ScannedBlock>,
    #[serde(default)]
    recent: Vec<ScannedBlock>,
}

/// File-backed epoch log. Single writer (the mint process); atomic
/// write-via-rename; the last non-dissolved record is the current epoch.
pub struct EpochStore {
    path: PathBuf,
    records: Vec<EpochRecord>,
    watermark: Option<ScannedBlock>,
    /// Trailing window of processed blocks, oldest first, watermark last.
    recent: Vec<ScannedBlock>,
}

impl EpochStore {
    pub fn load(path: &Path) -> Result<Self> {
        let file = if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading epoch store {}", path.display()))?;
            serde_json::from_str::<StoreFile>(&raw)
                .with_context(|| format!("parsing epoch store {}", path.display()))?
        } else {
            StoreFile::default()
        };
        Ok(Self {
            path: path.to_path_buf(),
            records: file.records,
            watermark: file.watermark,
            recent: file.recent,
        })
    }

    pub fn current(&self) -> Option<&EpochRecord> {
        self.records
            .iter()
            .rev()
            .find(|r| r.state != EpochState::Dissolved)
    }

    /// Oldest first. A dissolved record must never resolve again.
    pub fn non_dissolved(&self) -> Vec<EpochRecord> {
        self.records
            .iter()
            .filter(|r| r.state != EpochState::Dissolved)
            .cloned()
            .collect()
    }

    pub fn unit_taken(&self, unit: &str) -> bool {
        self.records.iter().any(|r| r.unit == unit)
    }

    /// The record for `unit`, if any.
    pub fn record(&self, unit: &str) -> Option<EpochRecord> {
        self.records.iter().find(|r| r.unit == unit).cloned()
    }

    /// The nearest non-dissolved record older than `unit`.
    pub fn previous_non_dissolved(&self, unit: &str) -> Option<EpochRecord> {
        let idx = self.records.iter().position(|r| r.unit == unit)?;
        self.records[..idx]
            .iter()
            .rev()
            .find(|r| r.state != EpochState::Dissolved)
            .cloned()
    }

    pub fn append(&mut self, record: EpochRecord) -> Result<()> {
        self.records.push(record);
        if let Err(e) = self.persist() {
            // Keep memory and disk agreeing: a failed persist must not leave a
            // phantom record that a later successful append would resurrect.
            self.records.pop();
            return Err(e);
        }
        Ok(())
    }

    /// Mutates one record in place and persists. Reverts the mutation if the
    /// persist fails, so memory and disk never disagree.
    pub fn update_record(&mut self, unit: &str, f: impl FnOnce(&mut EpochRecord)) -> Result<()> {
        let idx = self
            .records
            .iter()
            .position(|r| r.unit == unit)
            .ok_or_else(|| anyhow!("no epoch record for unit {unit}"))?;
        let backup = self.records[idx].clone();
        f(&mut self.records[idx]);
        if let Err(e) = self.persist() {
            self.records[idx] = backup;
            return Err(e);
        }
        Ok(())
    }

    pub fn watermark(&self) -> Option<&ScannedBlock> {
        self.watermark.as_ref()
    }

    pub fn recent(&self) -> &[ScannedBlock] {
        &self.recent
    }

    /// Advances the watermark: pushes `block` onto `recent`, trims the front
    /// down to `cap` entries, and persists. Reverts on a persist failure.
    pub fn set_watermark(&mut self, block: ScannedBlock, cap: usize) -> Result<()> {
        let backup_recent = self.recent.clone();
        let backup_watermark = self.watermark.clone();

        self.recent.push(block.clone());
        if self.recent.len() > cap {
            let excess = self.recent.len() - cap;
            self.recent.drain(0..excess);
        }
        self.watermark = Some(block);

        if let Err(e) = self.persist() {
            self.recent = backup_recent;
            self.watermark = backup_watermark;
            return Err(e);
        }
        Ok(())
    }

    /// Truncates `recent` to entries at or below `height` and sets the
    /// watermark to the last remaining entry (`None` if none remain).
    /// Reverts on a persist failure.
    pub fn rollback_to(&mut self, height: u64) -> Result<()> {
        let backup_recent = self.recent.clone();
        let backup_watermark = self.watermark.clone();

        self.recent.retain(|b| b.height <= height);
        self.watermark = self.recent.last().cloned();

        if let Err(e) = self.persist() {
            self.recent = backup_recent;
            self.watermark = backup_watermark;
            return Err(e);
        }
        Ok(())
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(&StoreFile {
            records: self.records.clone(),
            watermark: self.watermark.clone(),
            recent: self.recent.clone(),
        })?;
        std::fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming into {}", self.path.display()))?;
        Ok(())
    }

    /// Number of records already at this height (drives the unit-name suffix).
    pub fn count_at_height(&self, height: u64) -> u32 {
        self.records.iter().filter(|r| r.height == height).count() as u32
    }
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

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "epoch-store-test-{}-{label}.json",
            std::process::id()
        ))
    }

    #[test]
    fn round_trips_and_tracks_current() {
        let dir = std::env::temp_dir().join(format!("epoch-store-test-{}", std::process::id()));
        let path = dir.join("epochs.json");
        let _ = std::fs::remove_file(&path);

        let mut store = EpochStore::load(&path).unwrap();
        assert!(store.current().is_none());

        store.append(record(100, "hash_ab_100", EpochState::Final)).unwrap();
        store.append(record(105, "hash_ab_105", EpochState::Final)).unwrap();
        assert_eq!(store.current().unwrap().unit, "hash_ab_105");
        assert!(store.unit_taken("hash_ab_100"));
        assert_eq!(store.count_at_height(105), 1);

        // Reload from disk: same view.
        let reloaded = EpochStore::load(&path).unwrap();
        assert_eq!(reloaded.current().unwrap().unit, "hash_ab_105");

        // A dissolved record is never current.
        let mut store = reloaded;
        store.append(record(106, "hash_ab_106", EpochState::Dissolved)).unwrap();
        assert_eq!(store.current().unwrap().unit, "hash_ab_105");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn non_dissolved_excludes_dissolved_records_only() {
        let dir = std::env::temp_dir().join(format!("epoch-store-test-nd-{}", std::process::id()));
        let path = dir.join("epochs.json");
        let _ = std::fs::remove_file(&path);

        let mut store = EpochStore::load(&path).unwrap();
        store.append(record(100, "hash_ab_100", EpochState::Final)).unwrap();
        store.append(record(105, "hash_ab_105", EpochState::Dissolved)).unwrap();
        store.append(record(110, "hash_ab_110", EpochState::Final)).unwrap();

        let non_dissolved = store.non_dissolved();
        let units: Vec<&str> = non_dissolved.iter().map(|r| r.unit.as_str()).collect();
        assert_eq!(units, vec!["hash_ab_100", "hash_ab_110"]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn previous_non_dissolved_skips_dissolved_records() {
        let path = temp_path("prev-non-dissolved");
        let _ = std::fs::remove_file(&path);

        let mut store = EpochStore::load(&path).unwrap();
        store.append(record(100, "a", EpochState::Final)).unwrap();
        store.append(record(105, "b", EpochState::Dissolved)).unwrap();
        store.append(record(110, "c", EpochState::Provisional)).unwrap();

        assert_eq!(store.previous_non_dissolved("c").unwrap().unit, "a");
        assert!(store.previous_non_dissolved("a").is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn watermark_and_recent_round_trip_through_the_file() {
        let path = temp_path("watermark-roundtrip");
        let _ = std::fs::remove_file(&path);

        let mut store = EpochStore::load(&path).unwrap();
        assert!(store.watermark().is_none());
        assert!(store.recent().is_empty());

        store.set_watermark(block(100, "h100"), 16).unwrap();
        store.set_watermark(block(101, "h101"), 16).unwrap();

        let reloaded = EpochStore::load(&path).unwrap();
        assert_eq!(reloaded.watermark(), Some(&block(101, "h101")));
        assert_eq!(reloaded.recent(), &[block(100, "h100"), block(101, "h101")]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_old_format_file_with_no_watermark_key_loads_as_none() {
        let path = temp_path("old-format");
        let _ = std::fs::remove_file(&path);
        std::fs::write(
            &path,
            serde_json::json!({ "records": [] }).to_string(),
        )
        .unwrap();

        let store = EpochStore::load(&path).unwrap();
        assert!(store.watermark().is_none());
        assert!(store.recent().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn set_watermark_trims_to_the_cap() {
        let path = temp_path("cap");
        let _ = std::fs::remove_file(&path);

        let mut store = EpochStore::load(&path).unwrap();
        for h in 0..5 {
            store.set_watermark(block(h, &format!("h{h}")), 3).unwrap();
        }

        assert_eq!(
            store.recent(),
            &[block(2, "h2"), block(3, "h3"), block(4, "h4")]
        );
        assert_eq!(store.watermark(), Some(&block(4, "h4")));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rollback_to_truncates_recent_and_moves_the_watermark_back() {
        let path = temp_path("rollback");
        let _ = std::fs::remove_file(&path);

        let mut store = EpochStore::load(&path).unwrap();
        store.set_watermark(block(100, "h100"), 16).unwrap();
        store.set_watermark(block(101, "h101"), 16).unwrap();
        store.set_watermark(block(102, "h102"), 16).unwrap();

        store.rollback_to(101).unwrap();

        assert_eq!(store.recent(), &[block(100, "h100"), block(101, "h101")]);
        assert_eq!(store.watermark(), Some(&block(101, "h101")));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn update_record_keeps_memory_and_disk_agreeing_on_a_persist_failure() {
        // Point `path` at a directory so the rename in `persist` fails.
        let dir = temp_path("update-record-fail-dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut store = EpochStore {
            path: dir.clone(),
            records: vec![record(100, "a", EpochState::Final)],
            watermark: None,
            recent: Vec::new(),
        };

        let result = store.update_record("a", |r| r.state = EpochState::Dissolved);
        assert!(result.is_err());
        assert_eq!(store.records[0].state, EpochState::Final);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
