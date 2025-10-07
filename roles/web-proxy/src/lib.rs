use std::sync::{Arc, RwLock};
use stats::stats_adapter::ProxySnapshot;

pub mod config;
pub mod web;

/// In-memory storage for proxy snapshot data
pub struct SnapshotStorage {
    snapshot: Arc<RwLock<Option<ProxySnapshot>>>,
}

impl SnapshotStorage {
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(None)),
        }
    }

    pub fn update(&self, snapshot: ProxySnapshot) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = Some(snapshot);
        }
    }

    pub fn get(&self) -> Option<ProxySnapshot> {
        self.snapshot.read().ok().and_then(|guard| guard.clone())
    }

    pub fn is_stale(&self, threshold_secs: u64) -> bool {
        match self.snapshot.read().ok().and_then(|guard| guard.clone()) {
            Some(snapshot) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                now - snapshot.timestamp > threshold_secs
            }
            None => true,
        }
    }
}

impl Default for SnapshotStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_storage() {
        let storage = SnapshotStorage::new();

        let snapshot = ProxySnapshot {
            ehash_balance: 750,
            upstream_pool: None,
            downstream_miners: vec![],
            timestamp: 123,
        };

        storage.update(snapshot.clone());
        let retrieved = storage.get().unwrap();
        assert_eq!(retrieved.ehash_balance, 750);
    }

    #[test]
    fn test_storage_returns_none_initially() {
        let storage = SnapshotStorage::new();
        assert!(storage.get().is_none());
    }

    #[test]
    fn test_staleness_detection() {
        let storage = SnapshotStorage::new();

        // No data = stale
        assert!(storage.is_stale(15));

        // Fresh data
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let snapshot = ProxySnapshot {
            ehash_balance: 100,
            upstream_pool: None,
            downstream_miners: vec![],
            timestamp: now,
        };
        storage.update(snapshot);
        assert!(!storage.is_stale(15));

        // Old data (30 seconds ago)
        let old_snapshot = ProxySnapshot {
            ehash_balance: 100,
            upstream_pool: None,
            downstream_miners: vec![],
            timestamp: now - 30,
        };
        storage.update(old_snapshot);
        assert!(storage.is_stale(15));
    }
}
