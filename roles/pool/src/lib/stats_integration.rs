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

        // Separate JD connections from proxy connections
        let mut downstream_proxies = Vec::new();

        for (id, downstream) in &self.downstreams {
            // Try to get downstream info - if it fails, use defaults
            if let Ok((address, is_jd, channels, shares, quotes, ehash, last_share, work_selection)) = downstream
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

                    let is_jd = d.is_job_declarator();
                    tracing::debug!("Downstream {} ({}) - is_jd: {}", id, d.address, is_jd);

                    (
                        d.address.to_string(),
                        is_jd, // true = JD, false = proxy
                        channels,
                        0u64, // shares_submitted - TODO: track this
                        0u64, // quotes_created - TODO: track this
                        0u64, // ehash_mined - TODO: track this
                        None, // last_share_at - TODO: track this
                        d.has_work_selection(),
                    )
                })
            {
                if is_jd {
                    // This is a Job Declarator - add to services
                    services.push(ServiceConnection {
                        service_type: ServiceType::JobDeclarator,
                        address,
                    });
                } else {
                    // This is a regular proxy - add to downstream_proxies
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
        }

        PoolSnapshot {
            services,
            downstream_proxies,
            listen_address: self.listen_address.clone(),
            timestamp: unix_timestamp(),
        }
    }
}
