use std::sync::Arc;
use tracing::{debug, warn};

use stats::stats_adapter::{JdsSnapshot, PoolSnapshot};
use stats_sv2::types::ServiceSnapshot;

use crate::db::StatsData;

pub struct StatsHandler {
    db: Arc<StatsData>,
}

impl StatsHandler {
    pub fn new(db: Arc<StatsData>) -> Self {
        Self { db }
    }

    /// Accept a newline-delimited JSON payload, deserialize it into a
    /// `PoolSnapshot`, `JdsSnapshot`, or `ServiceSnapshot`, and store it appropriately.
    pub async fn handle_message(&self, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        // Try to deserialize as ServiceSnapshot (metrics data) first
        if let Ok(snapshot) = serde_json::from_slice::<ServiceSnapshot>(data) {
            debug!(
                "Received metrics snapshot: service_type={:?}, downstreams={}, timestamp={}",
                snapshot.service_type,
                snapshot.downstreams.len(),
                snapshot.timestamp
            );

            // Store metrics in database
            self.db.store_metrics_snapshot(snapshot).await?;
            return Ok(());
        }

        // Try to deserialize as PoolSnapshot
        if let Ok(snapshot) = serde_json::from_slice::<PoolSnapshot>(data) {
            debug!(
                "Received pool snapshot: services={}, proxies={}, listen={}, ts={}",
                snapshot.services.len(),
                snapshot.downstream_proxies.len(),
                snapshot.listen_address,
                snapshot.timestamp
            );

            self.db.store_snapshot(snapshot);
            return Ok(());
        }

        // Try JdsSnapshot
        if let Ok(snapshot) = serde_json::from_slice::<JdsSnapshot>(data) {
            debug!(
                "Received JDS snapshot: listen={}, ts={}",
                snapshot.listen_address, snapshot.timestamp
            );

            self.db.store_jds_snapshot(snapshot);
            return Ok(());
        }

        warn!("Failed to parse snapshot message as ServiceSnapshot, PoolSnapshot, or JdsSnapshot");
        Err("Unknown snapshot type".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stats::stats_adapter::{ProxyConnection, ServiceConnection, ServiceType};
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

        let snapshot = PoolSnapshot {
            services: vec![ServiceConnection {
                service_type: ServiceType::Mint,
                address: "127.0.0.1:9000".to_string(),
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

        let json = serde_json::to_vec(&snapshot).unwrap();
        handler.handle_message(&json).await.unwrap();

        let retrieved = db.get_latest_snapshot().unwrap();
        assert_eq!(retrieved.services.len(), 1);
        assert_eq!(retrieved.downstream_proxies.len(), 1);
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
    async fn test_multiple_snapshots_overwrite() {
        let db = Arc::new(StatsData::new());
        let handler = StatsHandler::new(db.clone());

        let first = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "first".to_string(),
            timestamp: unix_timestamp(),
        };
        handler
            .handle_message(&serde_json::to_vec(&first).unwrap())
            .await
            .unwrap();

        let second = PoolSnapshot {
            services: vec![],
            downstream_proxies: vec![],
            listen_address: "second".to_string(),
            timestamp: unix_timestamp() + 1,
        };
        handler
            .handle_message(&serde_json::to_vec(&second).unwrap())
            .await
            .unwrap();

        let retrieved = db.get_latest_snapshot().unwrap();
        assert_eq!(retrieved.listen_address, "second");
    }
}
