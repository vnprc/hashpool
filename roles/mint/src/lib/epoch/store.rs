use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpochState {
    /// Boundary block seen but not yet confirmed to depth D. Quotes created
    /// while provisional are held unpaid. (Unused until the reward trigger
    /// lands; genesis and manual epochs are final immediately.)
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

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    records: Vec<EpochRecord>,
}

/// File-backed epoch log. Single writer (the mint process); atomic
/// write-via-rename; the last non-dissolved record is the current epoch.
pub struct EpochStore {
    path: PathBuf,
    records: Vec<EpochRecord>,
}

impl EpochStore {
    pub fn load(path: &Path) -> Result<Self> {
        let records = if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading epoch store {}", path.display()))?;
            serde_json::from_str::<StoreFile>(&raw)
                .with_context(|| format!("parsing epoch store {}", path.display()))?
                .records
        } else {
            Vec::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            records,
        })
    }

    pub fn current(&self) -> Option<&EpochRecord> {
        self.records
            .iter()
            .rev()
            .find(|r| r.state != EpochState::Dissolved)
    }

    pub fn unit_taken(&self, unit: &str) -> bool {
        self.records.iter().any(|r| r.unit == unit)
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

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(&StoreFile {
            records: self.records.clone(),
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

#[allow(dead_code)]
fn _err(msg: &str) -> anyhow::Error {
    anyhow!("{msg}")
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
}
