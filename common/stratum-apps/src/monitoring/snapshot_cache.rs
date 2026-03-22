//! Snapshot cache for monitoring data
//!
//! This module provides a cache layer that decouples monitoring API requests
//! from the business logic locks (e.g., `ChannelManagerData`).
//!
//! ## Problem
//!
//! Without caching, every monitoring request acquires the same lock used by
//! share validation and job distribution. An attacker can spam monitoring
//! endpoints to cause lock contention, degrading mining performance.
//!
//! ## Solution
//!
//! The `SnapshotCache` periodically copies monitoring data from the source
//! (via the monitoring traits) into a cache. API requests read from the cache
//! without acquiring the business logic lock.
//!
//! ```text
//! Business Logic                    Monitoring
//! ──────────────                    ──────────
//!     │                                  │
//!     │ (holds lock for                  │
//!     │  share validation)               │
//!     │                                  │
//!     └──────────────────────────────────┤
//!                                        │
//!                              ┌─────────▼─────────┐
//!                              │  SnapshotCache    │
//!                              │  (RwLock, fast)   │
//!                              └─────────┬─────────┘
//!                                        │
//!                    ┌───────────────────┼───────────────────┐
//!                    │                   │                   │
//!              ┌─────▼─────┐       ┌─────▼─────┐       ┌─────▼─────┐
//!              │ /metrics  │       │ /api/v1/* │       │ /health   │
//!              └───────────┘       └───────────┘       └───────────┘
//! ```

use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use super::{
    client::{Sv2ClientInfo, Sv2ClientsMonitoring, Sv2ClientsSummary},
    server::{ServerInfo, ServerMonitoring, ServerSummary},
    sv1::{Sv1ClientInfo, Sv1ClientsMonitoring, Sv1ClientsSummary},
};

/// Cached snapshot of monitoring data.
///
/// This struct holds a point-in-time copy of all monitoring data,
/// allowing API requests to read without acquiring business logic locks.
#[derive(Debug, Clone, Default)]
pub struct MonitoringSnapshot {
    pub timestamp: Option<Instant>,
    pub server_info: Option<ServerInfo>,
    pub server_summary: Option<ServerSummary>,
    pub sv2_clients: Option<Vec<Sv2ClientInfo>>,
    pub sv2_clients_summary: Option<Sv2ClientsSummary>,
    pub sv1_clients: Option<Vec<Sv1ClientInfo>>,
    pub sv1_clients_summary: Option<Sv1ClientsSummary>,
}

/// A cache that holds monitoring snapshots and refreshes them periodically.
pub struct SnapshotCache {
    snapshot: RwLock<MonitoringSnapshot>,
    refresh_interval: Duration,
    server_source: Option<Arc<dyn ServerMonitoring + Send + Sync>>,
    sv2_clients_source: Option<Arc<dyn Sv2ClientsMonitoring + Send + Sync>>,
    sv1_clients_source: Option<Arc<dyn Sv1ClientsMonitoring + Send + Sync>>,
}

impl Clone for SnapshotCache {
    fn clone(&self) -> Self {
        // Clone creates a new cache with the same sources and current snapshot
        let current_snapshot = self.snapshot.read().unwrap().clone();
        Self {
            snapshot: RwLock::new(current_snapshot),
            refresh_interval: self.refresh_interval,
            server_source: self.server_source.clone(),
            sv2_clients_source: self.sv2_clients_source.clone(),
            sv1_clients_source: self.sv1_clients_source.clone(),
        }
    }
}

impl SnapshotCache {
    /// Create a new snapshot cache with the given refresh interval.
    ///
    /// # Arguments
    ///
    /// * `refresh_interval` - How often to refresh the cache (e.g., 15 seconds)
    /// * `server_source` - Optional server monitoring trait object
    /// * `sv2_clients_source` - Optional Sv2 clients monitoring trait object
    pub fn new(
        refresh_interval: Duration,
        server_source: Option<Arc<dyn ServerMonitoring + Send + Sync>>,
        sv2_clients_source: Option<Arc<dyn Sv2ClientsMonitoring + Send + Sync>>,
    ) -> Self {
        Self {
            snapshot: RwLock::new(MonitoringSnapshot::default()),
            refresh_interval,
            server_source,
            sv2_clients_source,
            sv1_clients_source: None,
        }
    }

    /// Add SV1 monitoring source (for Tproxy)
    pub fn with_sv1_clients_source(
        mut self,
        sv1_source: Arc<dyn Sv1ClientsMonitoring + Send + Sync>,
    ) -> Self {
        self.sv1_clients_source = Some(sv1_source);
        self
    }

    /// Get the current snapshot.
    ///
    /// This is a fast read that does NOT acquire any business logic locks.
    /// The returned snapshot may be up to `refresh_interval` old.
    pub fn get_snapshot(&self) -> MonitoringSnapshot {
        self.snapshot.read().unwrap().clone()
    }

    /// Refresh the cache by reading from the data sources.
    ///
    /// This method DOES acquire the business logic locks (via the trait methods),
    /// but it's only called periodically by a background task, not on every request.
    pub fn refresh(&self) {
        let mut new_snapshot = MonitoringSnapshot {
            timestamp: Some(Instant::now()),
            ..Default::default()
        };

        // Collect server data
        if let Some(ref source) = self.server_source {
            new_snapshot.server_info = Some(source.get_server());
            new_snapshot.server_summary = Some(source.get_server_summary());
        }

        // Collect Sv2 clients data
        if let Some(ref source) = self.sv2_clients_source {
            new_snapshot.sv2_clients = Some(source.get_sv2_clients());
            new_snapshot.sv2_clients_summary = Some(source.get_sv2_clients_summary());
        }

        // Collect Sv1 clients data
        if let Some(ref source) = self.sv1_clients_source {
            new_snapshot.sv1_clients = Some(source.get_sv1_clients());
            new_snapshot.sv1_clients_summary = Some(source.get_sv1_clients_summary());
        }

        // Update the cache
        *self.snapshot.write().unwrap() = new_snapshot;
    }

    /// Get the refresh interval
    pub fn refresh_interval(&self) -> Duration {
        self.refresh_interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockServerMonitoring;
    impl ServerMonitoring for MockServerMonitoring {
        fn get_server(&self) -> ServerInfo {
            ServerInfo {
                extended_channels: vec![],
                standard_channels: vec![],
            }
        }
    }

    struct MockSv2ClientsMonitoring;
    impl Sv2ClientsMonitoring for MockSv2ClientsMonitoring {
        fn get_sv2_clients(&self) -> Vec<Sv2ClientInfo> {
            vec![]
        }
    }

    #[test]
    fn test_snapshot_cache_creation() {
        let cache = SnapshotCache::new(
            Duration::from_secs(5),
            Some(Arc::new(MockServerMonitoring)),
            Some(Arc::new(MockSv2ClientsMonitoring)),
        );

        // Before refresh, snapshot has no timestamp
        let snapshot = cache.get_snapshot();
        assert!(snapshot.timestamp.is_none());
        assert_eq!(cache.refresh_interval(), Duration::from_secs(5));
    }

    #[test]
    fn test_snapshot_refresh() {
        let cache = SnapshotCache::new(
            Duration::from_secs(5),
            Some(Arc::new(MockServerMonitoring)),
            Some(Arc::new(MockSv2ClientsMonitoring)),
        );

        // Before refresh, snapshot has no data
        let snapshot = cache.get_snapshot();
        assert!(snapshot.timestamp.is_none());
        assert!(snapshot.server_info.is_none());

        // After refresh, snapshot has data
        cache.refresh();
        let snapshot = cache.get_snapshot();
        assert!(snapshot.timestamp.is_some());
        assert!(snapshot.server_info.is_some());
        assert!(snapshot.sv2_clients.is_some());
        assert!(snapshot.sv2_clients_summary.is_some());
    }

    /// Mock monitoring that simulates lock contention with business logic.
    ///
    /// This is used to verify that the snapshot cache eliminates lock contention
    /// between monitoring API requests and business logic operations.
    struct ContendedMonitoring {
        lock_hold_duration: Duration,
        monitoring_lock_acquisitions: std::sync::atomic::AtomicU64,
        business_lock: std::sync::Mutex<()>,
    }

    impl ContendedMonitoring {
        fn new(lock_hold_duration: Duration) -> Self {
            Self {
                lock_hold_duration,
                monitoring_lock_acquisitions: std::sync::atomic::AtomicU64::new(0),
                business_lock: std::sync::Mutex::new(()),
            }
        }

        fn simulate_business_logic(&self) {
            let _guard = self.business_lock.lock().unwrap();
            std::thread::sleep(self.lock_hold_duration);
        }

        fn get_monitoring_acquisitions(&self) -> u64 {
            self.monitoring_lock_acquisitions
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Sv2ClientsMonitoring for ContendedMonitoring {
        fn get_sv2_clients(&self) -> Vec<Sv2ClientInfo> {
            let _guard = self.business_lock.lock().unwrap();
            self.monitoring_lock_acquisitions
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Minimal sleep to simulate lock acquisition overhead
            std::thread::sleep(Duration::from_micros(10));
            vec![]
        }
    }

    impl ServerMonitoring for ContendedMonitoring {
        fn get_server(&self) -> ServerInfo {
            let _guard = self.business_lock.lock().unwrap();
            self.monitoring_lock_acquisitions
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Minimal sleep to simulate lock acquisition overhead
            std::thread::sleep(Duration::from_micros(10));
            ServerInfo {
                extended_channels: vec![],
                standard_channels: vec![],
            }
        }
    }

    /// Verifies that the snapshot cache eliminates lock contention.
    ///
    /// Without the cache, monitoring API requests would acquire the same lock
    /// used by business logic (share validation, job distribution), causing
    /// performance degradation. The cache decouples these operations by
    /// periodically refreshing a snapshot that API requests read from.
    #[test]
    fn test_snapshot_cache_eliminates_lock_contention() {
        let real_monitoring = Arc::new(ContendedMonitoring::new(Duration::from_millis(1)));

        let cache = Arc::new(SnapshotCache::new(
            Duration::from_secs(5),
            None,
            Some(real_monitoring.clone() as Arc<dyn Sv2ClientsMonitoring + Send + Sync>),
        ));

        cache.refresh();

        // Simulate business logic running concurrently
        let business_mon = Arc::clone(&real_monitoring);
        let business_handle = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut ops = 0u64;
            while start.elapsed() < Duration::from_millis(100) {
                business_mon.simulate_business_logic();
                ops += 1;
            }
            ops
        });

        // Simulate rapid API requests via cache (16 threads for higher throughput)
        let mut monitoring_handles = vec![];
        for _ in 0..16 {
            let cache_ref = Arc::clone(&cache);
            monitoring_handles.push(std::thread::spawn(move || {
                let start = std::time::Instant::now();
                let mut requests = 0u64;
                // Tight loop - cache reads are extremely fast
                while start.elapsed() < Duration::from_millis(100) {
                    let _ = cache_ref.get_snapshot();
                    requests += 1;
                }
                requests
            }));
        }

        let _business_ops = business_handle.join().unwrap();
        let total_cache_requests: u64 = monitoring_handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .sum();

        let real_lock_acquisitions = real_monitoring.get_monitoring_acquisitions();

        // Cache should only acquire lock during refresh (1-2 times), not per request
        assert!(
            real_lock_acquisitions <= 2,
            "Cache acquired lock {} times, expected ≤2 (refresh only)",
            real_lock_acquisitions
        );

        // Cache should enable high throughput without acquiring business logic locks
        assert!(
            total_cache_requests > 2,
            "Cache should have processed requests",
        );
    }
}
