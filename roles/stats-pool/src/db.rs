use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use stats::stats_adapter::PoolSnapshot;

/// In-memory storage for the latest pool snapshot.
///
/// The pool emits complete snapshots on every heartbeat, so we only need to
/// retain the most recent copy. Web services can derive any secondary views
/// off this structure without touching the SRI internals.
pub struct StatsData {
    snapshot: RwLock<Option<PoolSnapshot>>,
}

impl StatsData {
    pub fn new() -> Self {
        Self {
            snapshot: RwLock::new(None),
        }
    }

    /// Replace the currently stored snapshot with a new one.
    pub fn store_snapshot(&self, snapshot: PoolSnapshot) {
        let mut guard = self.snapshot.write().unwrap();
        *guard = Some(snapshot);
    }

    /// Fetch the latest snapshot clone for read-only consumers.
    pub fn get_latest_snapshot(&self) -> Option<PoolSnapshot> {
        let guard = self.snapshot.read().unwrap();
        guard.clone()
    }

    /// Determine if the stored snapshot is older than the provided threshold
    /// (expressed in seconds). Missing data is treated as stale so callers can
    /// surface appropriate warnings in health endpoints.
    pub fn is_stale(&self, threshold_secs: i64) -> bool {
        let guard = self.snapshot.read().unwrap();

        match guard.as_ref() {
            Some(snapshot) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;

                (now - snapshot.timestamp as i64) > threshold_secs
            }
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stats::stats_adapter::{ProxyConnection, ServiceConnection, ServiceType};

    fn unix_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn test_store_pool_snapshot() {
        let store = StatsData::new();

        let snapshot = PoolSnapshot {
            services: vec![ServiceConnection {
                service_type: ServiceType::Mint,
                address: "127.0.0.1:9000".to_string(),
            }],
            downstream_proxies: vec![],
            listen_address: "0.0.0.0:34254".to_string(),
            timestamp: 1234567890,
        };

        store.store_snapshot(snapshot.clone());
        let retrieved = store.get_latest_snapshot().unwrap();

        assert_eq!(retrieved.listen_address, "0.0.0.0:34254");
        assert_eq!(retrieved.services.len(), 1);
    }

    #[test]
    fn test_snapshot_replaced() {
        let store = StatsData::new();

        let first = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "first".to_string(),
            timestamp: unix_timestamp(),
        };

        let second = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "second".to_string(),
            timestamp: unix_timestamp() + 5,
        };

        store.store_snapshot(first);
        store.store_snapshot(second.clone());

        let retrieved = store.get_latest_snapshot().unwrap();
        assert_eq!(retrieved.listen_address, "second");
        assert_eq!(retrieved.timestamp, second.timestamp);
    }

    #[test]
    fn test_is_stale_without_snapshot() {
        let store = StatsData::new();
        assert!(store.is_stale(15));
    }

    #[test]
    fn test_is_stale_with_recent_snapshot() {
        let store = StatsData::new();

        let snapshot = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "0.0.0.0:34254".to_string(),
            timestamp: unix_timestamp(),
        };

        store.store_snapshot(snapshot);
        assert!(!store.is_stale(15));
    }

    #[test]
    fn test_is_stale_with_old_snapshot() {
        let store = StatsData::new();

        let snapshot = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "0.0.0.0:34254".to_string(),
            timestamp: unix_timestamp() - 60,
        };

        store.store_snapshot(snapshot);
        assert!(store.is_stale(15));
    }

    #[test]
    fn test_store_snapshot_with_proxies() {
        let store = StatsData::new();

        let snapshot = PoolSnapshot {
            services: vec![ServiceConnection {
                service_type: ServiceType::JobDeclarator,
                address: "127.0.0.1:9001".to_string(),
            }],
            downstream_proxies: vec![ProxyConnection {
                id: 1,
                address: "10.0.0.2:34255".to_string(),
                channels: vec![10, 11],
                shares_submitted: 5,
                quotes_created: 2,
                ehash_mined: 50,
                last_share_at: Some(unix_timestamp()),
            }],
            listen_address: "0.0.0.0:34254".to_string(),
            timestamp: unix_timestamp(),
        };

        store.store_snapshot(snapshot.clone());
        let retrieved = store.get_latest_snapshot().unwrap();

        assert_eq!(retrieved.downstream_proxies.len(), 1);
        assert_eq!(retrieved.downstream_proxies[0].shares_submitted, 5);
        assert_eq!(retrieved.services[0].service_type, ServiceType::JobDeclarator);
    }
}
