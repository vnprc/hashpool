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
        // Get raw miner info to access connected_time Instant
        let miner_tracker = self.miner_tracker.clone();
        let downstream_miners = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let miners = miner_tracker.get_all_miners().await;
                let now = std::time::SystemTime::now();

                miners.into_iter().map(|miner| {
                    // Convert Instant to Unix timestamp
                    // We calculate: now (SystemTime) - (Instant::now() - miner.connected_time)
                    let elapsed = std::time::Instant::now().duration_since(miner.connected_time);
                    let connected_at_systemtime = now - elapsed;
                    let connected_at = connected_at_systemtime
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    MinerInfo {
                        name: miner.name,
                        id: miner.id,
                        address: if self.config.redact_ip {
                            "REDACTED".to_string()
                        } else {
                            miner.address.to_string()
                        },
                        hashrate: miner.estimated_hashrate,
                        shares_submitted: miner.shares_submitted,
                        connected_at,
                    }
                }).collect()
            })
        });

        // Get blockchain network from environment variable
        let blockchain_network = std::env::var("BITCOIND_NETWORK")
            .unwrap_or_else(|_| "unknown".to_string())
            .to_lowercase();

        ProxySnapshot {
            ehash_balance,
            upstream_pool,
            downstream_miners,
            blockchain_network,
            timestamp: unix_timestamp(),
        }
    }
}
