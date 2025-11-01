use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct MinerInfo {
    pub id: u32,
    pub name: String,
    pub address: SocketAddr,
    pub connected_time: Instant,
    pub shares_submitted: u64,
    pub last_share_time: Option<Instant>,
    pub estimated_hashrate: f64, // H/s

    // Window metrics for time-series collection
    pub shares_in_window: u64,
    pub sum_difficulty_in_window: f64,
    pub window_start: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerStats {
    pub total_miners: usize,
    pub total_hashrate: String,
    pub total_shares: u64,
    pub miners: Vec<MinerApiInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerApiInfo {
    pub id: u32,
    pub name: String,
    pub address: String,
    pub hashrate: String,
    pub shares: u64,
    pub connected_time: String,
}

#[derive(Debug)]
pub struct MinerTracker {
    miners: Arc<RwLock<HashMap<u32, MinerInfo>>>,
    next_id: Arc<RwLock<u32>>,
}

impl MinerTracker {
    pub fn new() -> Self {
        Self {
            miners: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)),
        }
    }

    pub async fn add_miner(&self, address: SocketAddr, name: String) -> u32 {
        let mut next_id = self.next_id.write().await;
        let id = *next_id;
        *next_id += 1;
        drop(next_id);

        let miner = MinerInfo {
            id,
            name,
            address,
            connected_time: Instant::now(),
            shares_submitted: 0,
            last_share_time: None,
            estimated_hashrate: 0.0,
            shares_in_window: 0,
            sum_difficulty_in_window: 0.0,
            window_start: Instant::now(),
        };

        self.miners.write().await.insert(id, miner);
        id
    }

    pub async fn remove_miner(&self, id: u32) {
        self.miners.write().await.remove(&id);
    }

    pub async fn increment_shares(&self, id: u32, current_hashrate: f32) {
        let mut miners = self.miners.write().await;
        if let Some(miner) = miners.get_mut(&id) {
            miner.shares_submitted += 1;
            miner.last_share_time = Some(Instant::now());
            // Update with current hashrate from difficulty management
            // This gets adjusted by the difficulty system over time
            miner.estimated_hashrate = current_hashrate as f64;
        }
    }

    /// Record a share with its difficulty for time-series metrics.
    pub async fn record_share(&self, id: u32, difficulty: f64) {
        let mut miners = self.miners.write().await;
        if let Some(miner) = miners.get_mut(&id) {
            miner.shares_submitted += 1;
            miner.last_share_time = Some(Instant::now());
            miner.shares_in_window += 1;
            miner.sum_difficulty_in_window += difficulty;
        }
    }

    pub async fn update_hashrate(&self, id: u32, hashrate: f64) {
        let mut miners = self.miners.write().await;
        if let Some(miner) = miners.get_mut(&id) {
            miner.estimated_hashrate = hashrate;
        }
    }

    pub async fn update_miner_name(&self, id: u32, name: String) {
        let mut miners = self.miners.write().await;
        if let Some(miner) = miners.get_mut(&id) {
            miner.name = name;
        }
    }

    pub async fn get_hashrate(&self, id: u32) -> Option<f64> {
        let miners = self.miners.read().await;
        miners.get(&id).map(|m| m.estimated_hashrate)
    }

    pub async fn get_address(&self, id: u32) -> Option<String> {
        let miners = self.miners.read().await;
        miners.get(&id).map(|m| m.address.to_string())
    }

    pub async fn get_all_miners(&self) -> Vec<MinerInfo> {
        let miners = self.miners.read().await;
        miners.values().cloned().collect()
    }

    pub async fn get_stats(&self) -> MinerStats {
        let miners = self.miners.read().await;
        let total_miners = miners.len();
        let total_shares: u64 = miners.values().map(|m| m.shares_submitted).sum();
        let total_hashrate_raw: f64 = miners.values().map(|m| m.estimated_hashrate).sum();

        let total_hashrate = if total_hashrate_raw >= 1_000_000_000_000.0 {
            format!("{:.1} TH/s", total_hashrate_raw / 1_000_000_000_000.0)
        } else if total_hashrate_raw >= 1_000_000_000.0 {
            format!("{:.1} GH/s", total_hashrate_raw / 1_000_000_000.0)
        } else if total_hashrate_raw >= 1_000_000.0 {
            format!("{:.1} MH/s", total_hashrate_raw / 1_000_000.0)
        } else if total_hashrate_raw >= 1_000.0 {
            format!("{:.1} KH/s", total_hashrate_raw / 1_000.0)
        } else {
            format!("{:.1} H/s", total_hashrate_raw)
        };

        let miners_info: Vec<MinerApiInfo> = miners.values().map(|miner| {
            let hashrate = if miner.estimated_hashrate >= 1_000_000_000_000.0 {
                format!("{:.1} TH/s", miner.estimated_hashrate / 1_000_000_000_000.0)
            } else if miner.estimated_hashrate >= 1_000_000_000.0 {
                format!("{:.1} GH/s", miner.estimated_hashrate / 1_000_000_000.0)
            } else if miner.estimated_hashrate >= 1_000_000.0 {
                format!("{:.1} MH/s", miner.estimated_hashrate / 1_000_000.0)
            } else if miner.estimated_hashrate >= 1_000.0 {
                format!("{:.1} KH/s", miner.estimated_hashrate / 1_000.0)
            } else {
                format!("{:.1} H/s", miner.estimated_hashrate)
            };

            let connected_duration = Instant::now().duration_since(miner.connected_time);
            let connected_time = if connected_duration.as_secs() > 3600 {
                format!("{}h ago", connected_duration.as_secs() / 3600)
            } else if connected_duration.as_secs() > 60 {
                format!("{}m ago", connected_duration.as_secs() / 60)
            } else {
                format!("{}s ago", connected_duration.as_secs())
            };

            MinerApiInfo {
                id: miner.id,
                name: miner.name.clone(),
                address: "REDACTED".to_string(),
                hashrate,
                shares: miner.shares_submitted,
                connected_time,
            }
        }).collect();

        MinerStats {
            total_miners,
            total_hashrate,
            total_shares,
            miners: miners_info,
        }
    }
}

impl Default for MinerTracker {
    fn default() -> Self {
        Self::new()
    }
}
