use super::mining_pool::Pool;
use stats::stats_adapter::{
    PoolSnapshot, ProxyConnection, ServiceConnection, ServiceType, StatsSnapshotProvider,
};
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
        // Get service connections (pool, mint, jd-server if connected)
        let mut services = Vec::new();

        // Add the pool itself as first service
        services.push(ServiceConnection {
            service_type: ServiceType::Pool,
            address: self.listen_address.clone(),
        });

        // Add mint connections
        for (addr, _) in &self.mint_connections {
            services.push(ServiceConnection {
                service_type: ServiceType::Mint,
                address: addr.to_string(),
            });
        }

        // Get stats snapshot from registry
        let stats_snapshot = self.stats_registry.snapshot();

        // Collect all downstream proxy connections
        let mut downstream_proxies = Vec::new();

        for (id, downstream) in &self.downstreams {
            // Try to get downstream info - if it fails, use defaults
            if let Ok((address, channels, work_selection)) = downstream.safe_lock(|d| {
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

                tracing::debug!("Downstream {} ({}) - has_work_selection: {}", id, d.address, d.has_work_selection());

                (
                    d.address.to_string(),
                    channels,
                    d.has_work_selection(),
                )
            }) {
                // Lookup stats from registry
                let (shares, quotes, ehash, last_share) =
                    stats_snapshot.get(id).copied().unwrap_or((0, 0, 0, None));

                // All downstream connections (whether using Mining or Job Declaration protocol)
                // are tracked as proxies in the downstream_proxies list
                downstream_proxies.push(ProxyConnection {
                    id: *id,
                    address,
                    channels,
                    shares_submitted: shares,
                    quotes_created: quotes,
                    ehash_mined: ehash,
                    last_share_at: last_share,
                    work_selection,
                });
            }
        }

        PoolSnapshot {
            services,
            downstream_proxies,
            listen_address: self.listen_address.clone(),
            timestamp: unix_timestamp(),
        }
    }
}
