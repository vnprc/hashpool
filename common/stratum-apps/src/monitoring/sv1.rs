//! SV1 client monitoring types
//!
//! These types are specific to SV1 protocol client connections.
//! Used by Translator Proxy (tProxy) that accepts SV1 miner connections.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Information about a single SV1 client connection
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Sv1ClientInfo {
    pub client_id: usize,
    pub channel_id: Option<u32>,
    pub authorized_worker_name: String,
    pub user_identity: String,
    pub target_hex: String,
    pub hashrate: Option<f32>,
    pub extranonce1_hex: String,
    pub extranonce2_len: usize,
    pub version_rolling_mask: Option<String>,
    pub version_rolling_min_bit: Option<String>,
    // Hashpool extensions
    pub shares_submitted: u64,
    pub connected_at_secs: u64,
    pub peer_address: Option<String>,
    /// Windowed hashrate over the last 5 minutes (H/s)
    pub hashrate_5min: Option<f64>,
}

/// Aggregate information about SV1 client connections
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Sv1ClientsSummary {
    pub total_clients: usize,
    pub total_hashrate: f32,
}

/// Trait for monitoring SV1 client connections
pub trait Sv1ClientsMonitoring: Send + Sync {
    /// Get all SV1 clients
    fn get_sv1_clients(&self) -> Vec<Sv1ClientInfo>;

    /// Get a single SV1 client by client_id
    ///
    /// Default implementation does O(n) scan. Override for O(1) lookup
    /// if your implementation uses a HashMap internally.
    fn get_sv1_client_by_id(&self, client_id: usize) -> Option<Sv1ClientInfo> {
        self.get_sv1_clients()
            .into_iter()
            .find(|c| c.client_id == client_id)
    }

    /// Get summary of SV1 clients
    fn get_sv1_clients_summary(&self) -> Sv1ClientsSummary {
        let clients = self.get_sv1_clients();

        Sv1ClientsSummary {
            total_clients: clients.len(),
            total_hashrate: clients.iter().filter_map(|c| c.hashrate).sum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_sv1_client_info(id: usize, hashrate: Option<f32>) -> Sv1ClientInfo {
        Sv1ClientInfo {
            client_id: id,
            channel_id: Some(id as u32),
            authorized_worker_name: format!("worker-{}", id),
            user_identity: format!("miner-{}", id),
            target_hex: "00ff".into(),
            hashrate,
            extranonce1_hex: "aabb".into(),
            extranonce2_len: 8,
            version_rolling_mask: Some("ffffffff".into()),
            version_rolling_min_bit: Some("00000000".into()),
            shares_submitted: 0,
            connected_at_secs: 0,
            peer_address: None,
            hashrate_5min: None,
        }
    }

    struct MockSv1Clients(Vec<Sv1ClientInfo>);
    impl Sv1ClientsMonitoring for MockSv1Clients {
        fn get_sv1_clients(&self) -> Vec<Sv1ClientInfo> {
            self.0.clone()
        }
    }

    #[test]
    fn sv1_get_client_by_id_found() {
        let monitor = MockSv1Clients(vec![
            create_sv1_client_info(1, Some(10.0)),
            create_sv1_client_info(2, Some(20.0)),
        ]);
        let found = monitor.get_sv1_client_by_id(2);
        assert!(found.is_some());
        assert_eq!(found.unwrap().client_id, 2);
    }

    #[test]
    fn sv1_get_client_by_id_not_found() {
        let monitor = MockSv1Clients(vec![create_sv1_client_info(1, Some(10.0))]);
        assert!(monitor.get_sv1_client_by_id(999).is_none());
    }

    #[test]
    fn sv1_summary_empty() {
        let monitor = MockSv1Clients(vec![]);
        let summary = monitor.get_sv1_clients_summary();
        assert_eq!(summary.total_clients, 0);
        assert_eq!(summary.total_hashrate, 0.0);
    }

    #[test]
    fn sv1_summary_skips_none_hashrate() {
        let monitor = MockSv1Clients(vec![
            create_sv1_client_info(1, Some(100.0)),
            create_sv1_client_info(2, None),
            create_sv1_client_info(3, Some(50.0)),
        ]);
        let summary = monitor.get_sv1_clients_summary();
        assert_eq!(summary.total_clients, 3);
        assert_eq!(summary.total_hashrate, 150.0);
    }
}
