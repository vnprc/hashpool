use stats::stats_adapter::ProxySnapshot;
use stats_sv2::types::ServiceSnapshot;
use stats_sv2::StatsStorage;
use std::{
    sync::Arc,
    sync::RwLock,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct StatsData {
    snapshot: RwLock<Option<ProxySnapshot>>,
    // Time-series metrics storage
    metrics_storage: Arc<tokio::sync::RwLock<Option<stats_sv2::storage::SqliteStorage>>>,
}

impl StatsData {
    pub fn new() -> Self {
        StatsData {
            snapshot: RwLock::new(None),
            metrics_storage: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    /// Initialize metrics storage with optional database path
    pub async fn init_metrics_storage(&self, db_path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let path = db_path.ok_or("Database path is required for metrics storage")?;
        let storage = stats_sv2::storage::SqliteStorage::new(path).await?;
        let mut guard = self.metrics_storage.write().await;
        *guard = Some(storage);
        Ok(())
    }

    /// Store a service snapshot in metrics database
    pub async fn store_metrics_snapshot(&self, snapshot: ServiceSnapshot) -> Result<(), Box<dyn std::error::Error>> {
        let guard = self.metrics_storage.read().await;
        if let Some(storage) = guard.as_ref() {
            for downstream in snapshot.downstreams {
                storage.store_downstream(&downstream).await?;
            }
        }
        Ok(())
    }

    /// Query hashrate for a specific downstream
    pub async fn query_hashrate(
        &self,
        downstream_id: u32,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<stats_sv2::types::HashratePoint>, Box<dyn std::error::Error>> {
        let guard = self.metrics_storage.read().await;
        if let Some(storage) = guard.as_ref() {
            Ok(storage.query_hashrate(downstream_id, from_timestamp, to_timestamp).await?)
        } else {
            Ok(Vec::new())
        }
    }

    /// Query aggregate hashrate across all downstreams
    pub async fn query_aggregate_hashrate(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<stats_sv2::types::HashratePoint>, Box<dyn std::error::Error>> {
        let guard = self.metrics_storage.read().await;
        if let Some(storage) = guard.as_ref() {
            Ok(storage.query_aggregate_hashrate(from_timestamp, to_timestamp).await?)
        } else {
            Ok(Vec::new())
        }
    }

    /// Store a complete proxy snapshot
    pub fn store_snapshot(&self, snapshot: ProxySnapshot) {
        let mut guard = self.snapshot.write().unwrap();
        *guard = Some(snapshot);
    }

    /// Get the latest proxy snapshot
    pub fn get_latest_snapshot(&self) -> Option<ProxySnapshot> {
        let guard = self.snapshot.read().unwrap();
        guard.clone()
    }

    /// Check if the latest snapshot is stale (older than threshold_secs)
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
            None => true, // No snapshot = stale
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stats::stats_adapter::{MinerInfo, PoolConnection};

    fn unix_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn test_store_and_retrieve_snapshot() {
        let db = StatsData::new();

        let snapshot = ProxySnapshot {
            ehash_balance: 1000,
            upstream_pool: None,
            downstream_miners: vec![],
            blockchain_network: "regtest".to_string(),
            timestamp: unix_timestamp(),
        };

        db.store_snapshot(snapshot);
        let retrieved = db.get_latest_snapshot().unwrap();
        assert_eq!(retrieved.ehash_balance, 1000);
    }

    #[test]
    fn test_staleness_detection() {
        let db = StatsData::new();

        // Store old snapshot (30 seconds ago)
        let old_snapshot = ProxySnapshot {
            ehash_balance: 100,
            upstream_pool: None,
            downstream_miners: vec![],
            blockchain_network: "regtest".to_string(),
            timestamp: unix_timestamp() - 30,
        };
        db.store_snapshot(old_snapshot);

        assert!(db.is_stale(15), "Should be stale after 30 seconds");
    }

    #[test]
    fn test_snapshot_with_miners() {
        let db = StatsData::new();

        let snapshot = ProxySnapshot {
            ehash_balance: 5000,
            upstream_pool: Some(PoolConnection {
                address: "pool.example.com:3333".to_string(),
            }),
            downstream_miners: vec![
                MinerInfo {
                    name: "miner1".to_string(),
                    id: 1,
                    address: "192.168.1.100:4444".to_string(),
                    hashrate: 100.5,
                    shares_submitted: 42,
                    connected_at: 1234567890,
                },
                MinerInfo {
                    name: "miner2".to_string(),
                    id: 2,
                    address: "192.168.1.101:4444".to_string(),
                    hashrate: 200.0,
                    shares_submitted: 84,
                    connected_at: 1234567891,
                },
            ],
            blockchain_network: "regtest".to_string(),
            timestamp: unix_timestamp(),
        };

        db.store_snapshot(snapshot);
        let retrieved = db.get_latest_snapshot().unwrap();

        assert_eq!(retrieved.ehash_balance, 5000);
        assert_eq!(retrieved.downstream_miners.len(), 2);
        assert_eq!(retrieved.downstream_miners[0].name, "miner1");
        assert_eq!(retrieved.downstream_miners[1].hashrate, 200.0);
        assert!(retrieved.upstream_pool.is_some());
    }

    #[test]
    fn test_no_snapshot_returns_none() {
        let db = StatsData::new();
        let retrieved = db.get_latest_snapshot();
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_snapshot_replacement() {
        let db = StatsData::new();

        // Store first snapshot
        let snapshot1 = ProxySnapshot {
            ehash_balance: 1000,
            upstream_pool: None,
            downstream_miners: vec![],
            blockchain_network: "regtest".to_string(),
            timestamp: unix_timestamp(),
        };
        db.store_snapshot(snapshot1);

        // Store second snapshot (should replace first)
        let snapshot2 = ProxySnapshot {
            ehash_balance: 2000,
            upstream_pool: None,
            downstream_miners: vec![],
            blockchain_network: "regtest".to_string(),
            timestamp: unix_timestamp() + 5,
        };
        db.store_snapshot(snapshot2);

        // Should retrieve the latest one
        let retrieved = db.get_latest_snapshot().unwrap();
        assert_eq!(retrieved.ehash_balance, 2000);
    }

    #[test]
    fn test_not_stale_when_recent() {
        let db = StatsData::new();

        // Store recent snapshot (1 second ago)
        let snapshot = ProxySnapshot {
            ehash_balance: 100,
            upstream_pool: None,
            downstream_miners: vec![],
            blockchain_network: "regtest".to_string(),
            timestamp: unix_timestamp() - 1,
        };
        db.store_snapshot(snapshot);

        assert!(!db.is_stale(15), "Should not be stale after 1 second");
    }

    #[test]
    fn test_no_snapshot_is_stale() {
        let db = StatsData::new();
        assert!(db.is_stale(15), "No snapshot should be considered stale");
    }
}
