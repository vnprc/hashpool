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
