use std::sync::Arc;
use tracing::debug;
use stats::stats_adapter::ProxySnapshot;

use crate::db::StatsData;

pub struct StatsHandler {
    db: Arc<StatsData>,
}

impl StatsHandler {
    pub fn new(db: Arc<StatsData>) -> Self {
        Self { db }
    }

    pub async fn handle_message(&self, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        // Parse JSON message as ProxySnapshot
        let snapshot: ProxySnapshot = serde_json::from_slice(data)?;

        debug!(
            "Received snapshot: balance={}, miners={}, timestamp={}",
            snapshot.ehash_balance,
            snapshot.downstream_miners.len(),
            snapshot.timestamp
        );

        // Store the snapshot in memory
        self.db.store_snapshot(snapshot);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stats::stats_adapter::{MinerInfo, PoolConnection};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unix_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[tokio::test]
    async fn test_handle_snapshot_message() {
        let db = Arc::new(StatsData::new());
        let handler = StatsHandler::new(db.clone());

        let snapshot = ProxySnapshot {
            ehash_balance: 5000,
            upstream_pool: Some(PoolConnection {
                address: "pool.example.com:3333".to_string(),
            }),
            downstream_miners: vec![MinerInfo {
                name: "miner1".to_string(),
                id: 1,
                address: "192.168.1.100:4444".to_string(),
                hashrate: 100.5,
                shares_submitted: 42,
                connected_at: 1234567890,
            }],
            timestamp: unix_timestamp(),
        };

        let json = serde_json::to_vec(&snapshot).unwrap();
        handler.handle_message(&json).await.unwrap();

        // Verify snapshot was stored
        let retrieved = db.get_latest_snapshot().unwrap();
        assert_eq!(retrieved.ehash_balance, 5000);
        assert_eq!(retrieved.downstream_miners.len(), 1);
    }

    #[tokio::test]
    async fn test_handle_invalid_json() {
        let db = Arc::new(StatsData::new());
        let handler = StatsHandler::new(db);

        let invalid_json = b"not valid json";
        let result = handler.handle_message(invalid_json).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_multiple_snapshots() {
        let db = Arc::new(StatsData::new());
        let handler = StatsHandler::new(db.clone());

        // Send first snapshot
        let snapshot1 = ProxySnapshot {
            ehash_balance: 1000,
            upstream_pool: None,
            downstream_miners: vec![],
            timestamp: unix_timestamp(),
        };
        let json1 = serde_json::to_vec(&snapshot1).unwrap();
        handler.handle_message(&json1).await.unwrap();

        // Send second snapshot
        let snapshot2 = ProxySnapshot {
            ehash_balance: 2000,
            upstream_pool: None,
            downstream_miners: vec![],
            timestamp: unix_timestamp() + 5,
        };
        let json2 = serde_json::to_vec(&snapshot2).unwrap();
        handler.handle_message(&json2).await.unwrap();

        // Latest snapshot should be stored
        let retrieved = db.get_latest_snapshot().unwrap();
        assert_eq!(retrieved.ehash_balance, 2000);
    }
}
