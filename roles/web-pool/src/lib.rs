use std::sync::{Arc, RwLock};
use stats::stats_adapter::PoolSnapshot;

pub mod config;
pub mod web;

/// In-memory storage for pool snapshot data
pub struct SnapshotStorage {
    snapshot: Arc<RwLock<Option<PoolSnapshot>>>,
}

impl SnapshotStorage {
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(None)),
        }
    }

    pub fn update(&self, snapshot: PoolSnapshot) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = Some(snapshot);
        }
    }

    pub fn get(&self) -> Option<PoolSnapshot> {
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

        let snapshot = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "test".to_string(),
            timestamp: 456,
        };

        storage.update(snapshot.clone());
        let retrieved = storage.get().unwrap();
        assert_eq!(retrieved.timestamp, 456);
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

        let snapshot = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "0.0.0.0:34254".to_string(),
            timestamp: now,
        };
        storage.update(snapshot);
        assert!(!storage.is_stale(15));

        // Old data (30 seconds ago)
        let old_snapshot = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "0.0.0.0:34254".to_string(),
            timestamp: now - 30,
        };
        storage.update(old_snapshot);
        assert!(storage.is_stale(15));
    }
}
