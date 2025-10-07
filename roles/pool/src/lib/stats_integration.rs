use super::mining_pool::Pool;
use stats::stats_adapter::{PoolSnapshot, ProxyConnection, ServiceConnection, ServiceType, StatsSnapshotProvider};
use std::time::SystemTime;

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

impl StatsSnapshotProvider for Pool {
    type Snapshot = PoolSnapshot;

    fn get_snapshot(&self) -> PoolSnapshot {
        // Get service connections (mint, jd-server if connected)
        let mut services = Vec::new();

        // Add mint connections
        for (addr, _) in &self.mint_connections {
            services.push(ServiceConnection {
                service_type: ServiceType::Mint,
                address: addr.to_string(),
            });
        }

        // Get downstream proxy connections
        let downstream_proxies: Vec<ProxyConnection> = self
            .downstreams
            .iter()
            .map(|(id, downstream)| {
                // Try to get downstream info - if it fails, use defaults
                let (address, channels, shares, quotes, ehash, last_share) = downstream
                    .safe_lock(|d| {
                        // Get channel IDs for this downstream
                        let channels: Vec<u32> = self
                            .channel_to_downstream
                            .iter()
                            .filter_map(|(channel_id, downstream_id)| {
                                if downstream_id == id {
                                    Some(*channel_id)
                                } else {
                                    None
                                }
                            })
                            .collect();

                        (
                            d.address.to_string(),
                            channels,
                            0u64, // shares_submitted - TODO: track this
                            0u64, // quotes_created - TODO: track this
                            0u64, // ehash_mined - TODO: track this
                            None, // last_share_at - TODO: track this
                        )
                    })
                    .unwrap_or_else(|_| {
                        (
                            "unknown".to_string(),
                            vec![],
                            0,
                            0,
                            0,
                            None,
                        )
                    });

                ProxyConnection {
                    id: *id,
                    address,
                    channels,
                    shares_submitted: shares,
                    quotes_created: quotes,
                    ehash_mined: ehash,
                    last_share_at: last_share,
                }
            })
            .collect();

        PoolSnapshot {
            services,
            downstream_proxies,
            listen_address: self.listen_address.clone(),
            timestamp: unix_timestamp(),
        }
    }
}
