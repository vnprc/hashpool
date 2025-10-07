use serde::{Deserialize, Serialize};

/// Trait for collecting stats snapshot from hub services
/// Implemented by Pool and Translator to expose their state
pub trait StatsSnapshotProvider {
    type Snapshot: Serialize + for<'de> Deserialize<'de>;

    fn get_snapshot(&self) -> Self::Snapshot;
}

// Proxy-specific snapshot types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxySnapshot {
    pub ehash_balance: u64,
    pub upstream_pool: Option<PoolConnection>,
    pub downstream_miners: Vec<MinerInfo>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConnection {
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerInfo {
    pub name: String,
    pub id: u32,
    pub address: String,
    pub hashrate: f64,
    pub shares_submitted: u64,
    pub connected_at: u64,
}

// Pool-specific snapshot types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSnapshot {
    pub services: Vec<ServiceConnection>,
    pub downstream_proxies: Vec<ProxyConnection>,
    pub listen_address: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConnection {
    pub service_type: ServiceType,
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServiceType {
    Mint,
    JobDeclarator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConnection {
    pub id: u32,
    pub address: String,
    pub channels: Vec<u32>,
    pub shares_submitted: u64,
    pub quotes_created: u64,
    pub ehash_mined: u64,
    pub last_share_at: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_serialization() {
        // Test ProxySnapshot serializes to JSON correctly
        let snapshot = ProxySnapshot {
            ehash_balance: 1000,
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
            timestamp: 1234567890,
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: ProxySnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.ehash_balance, 1000);
        assert_eq!(deserialized.downstream_miners.len(), 1);
    }

    #[test]
    fn test_pool_snapshot_serialization() {
        // Test PoolSnapshot serializes to JSON correctly
        let snapshot = PoolSnapshot {
            services: vec![ServiceConnection {
                service_type: ServiceType::Mint,
                address: "127.0.0.1:8080".to_string(),
            }],
            downstream_proxies: vec![],
            listen_address: "0.0.0.0:34254".to_string(),
            timestamp: 1234567890,
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("Mint"));
    }
}
