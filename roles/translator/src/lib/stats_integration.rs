use super::TranslatorSv2;
use super::miner_stats;
use stats::stats_adapter::{MinerInfo, PoolConnection, ProxySnapshot, StatsSnapshotProvider};
use std::time::SystemTime;

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Parse formatted hashrate string back to raw H/s value
/// e.g. "100.5 TH/s" -> 100500000000000.0
fn parse_hashrate_string(hashrate_str: &str) -> f64 {
    let parts: Vec<&str> = hashrate_str.split_whitespace().collect();
    if parts.len() != 2 {
        return 0.0;
    }

    let value: f64 = parts[0].parse().unwrap_or(0.0);
    let unit = parts[1];

    match unit {
        "TH/s" => value * 1_000_000_000_000.0,
        "GH/s" => value * 1_000_000_000.0,
        "MH/s" => value * 1_000_000.0,
        "KH/s" => value * 1_000.0,
        "H/s" => value,
        _ => 0.0,
    }
}

impl StatsSnapshotProvider for TranslatorSv2 {
    type Snapshot = ProxySnapshot;

    fn get_snapshot(&self) -> ProxySnapshot {
        // Get wallet balance
        let ehash_balance = if let Some(ref wallet) = self.wallet {
            // Try to get balance synchronously without blocking
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    match wallet.total_balance().await {
                        Ok(amount) => u64::from(amount),
                        Err(_) => 0,
                    }
                })
            })
        } else {
            0
        };

        // Get upstream pool connection info from config
        let upstream_pool = Some(PoolConnection {
            address: format!("{}:{}", self.config.upstream_address, self.config.upstream_port),
        });

        // Get downstream miner info from MinerTracker
        // We'll need to access the internal miners map, so let's use the get_stats method
        // and convert the API info back to snapshot format
        let miner_tracker = self.miner_tracker.clone();
        let downstream_miners = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                // Unfortunately, MinerTracker doesn't expose the internal miners directly
                // We'll use the public get_stats method and convert the result
                let stats = miner_tracker.get_stats().await;
                stats.miners.into_iter().map(|miner_api| {
                    MinerInfo {
                        name: miner_api.name,
                        id: miner_api.id,
                        address: if self.config.redact_ip {
                            "REDACTED".to_string()
                        } else {
                            miner_api.address
                        },
                        hashrate: parse_hashrate_string(&miner_api.hashrate),
                        shares_submitted: miner_api.shares,
                        // We don't have exact connected_at timestamp, use 0 for now
                        // This is the timestamp when they connected, not duration
                        connected_at: 0,
                    }
                }).collect()
            })
        });

        ProxySnapshot {
            ehash_balance,
            upstream_pool,
            downstream_miners,
            timestamp: unix_timestamp(),
        }
    }
}
