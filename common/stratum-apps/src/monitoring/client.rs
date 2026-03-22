//! Sv2 client monitoring types
//!
//! These types are for monitoring **Sv2 clients** (downstream connections).
//! Each client can have multiple channels opened with the app.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Information about an extended channel
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExtendedChannelInfo {
    pub channel_id: u32,
    pub user_identity: String,
    pub nominal_hashrate: f32,
    pub target_hex: String,
    pub requested_max_target_hex: String,
    pub extranonce_prefix_hex: String,
    pub full_extranonce_size: usize,
    pub rollable_extranonce_size: u16,
    pub expected_shares_per_minute: f32,
    pub shares_accepted: u32,
    pub share_work_sum: f64,
    pub last_share_sequence_number: u32,
    pub best_diff: f64,
    pub last_batch_accepted: u32,
    pub last_batch_work_sum: f64,
    pub share_batch_size: usize,
    pub blocks_found: u32,
}

/// Information about a standard channel
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StandardChannelInfo {
    pub channel_id: u32,
    pub user_identity: String,
    pub nominal_hashrate: f32,
    pub target_hex: String,
    pub requested_max_target_hex: String,
    pub extranonce_prefix_hex: String,
    pub expected_shares_per_minute: f32,
    pub shares_accepted: u32,
    pub share_work_sum: f64,
    pub last_share_sequence_number: u32,
    pub best_diff: f64,
    pub last_batch_accepted: u32,
    pub last_batch_work_sum: f64,
    pub share_batch_size: usize,
    pub blocks_found: u32,
}

/// Full information about a single Sv2 client including all channels
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Sv2ClientInfo {
    pub client_id: usize,
    pub extended_channels: Vec<ExtendedChannelInfo>,
    pub standard_channels: Vec<StandardChannelInfo>,
}

impl Sv2ClientInfo {
    /// Get total number of channels for this client
    pub fn total_channels(&self) -> usize {
        self.extended_channels.len() + self.standard_channels.len()
    }

    /// Get total hashrate for this client
    pub fn total_hashrate(&self) -> f32 {
        self.extended_channels
            .iter()
            .map(|c| c.nominal_hashrate)
            .sum::<f32>()
            + self
                .standard_channels
                .iter()
                .map(|c| c.nominal_hashrate)
                .sum::<f32>()
    }

    /// Convert to metadata (without channel arrays)
    pub fn to_metadata(&self) -> Sv2ClientMetadata {
        Sv2ClientMetadata {
            client_id: self.client_id,
            extended_channels_count: self.extended_channels.len(),
            standard_channels_count: self.standard_channels.len(),
            total_hashrate: self.total_hashrate(),
        }
    }
}

/// Sv2 client metadata without channel arrays (for listings)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Sv2ClientMetadata {
    pub client_id: usize,
    pub extended_channels_count: usize,
    pub standard_channels_count: usize,
    pub total_hashrate: f32,
}

/// Aggregate information about all Sv2 clients
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Sv2ClientsSummary {
    pub total_clients: usize,
    pub total_channels: usize,
    pub extended_channels: usize,
    pub standard_channels: usize,
    pub total_hashrate: f32,
}

/// Trait for monitoring Sv2 clients (downstream connections)
pub trait Sv2ClientsMonitoring: Send + Sync {
    /// Get all Sv2 clients with their channels
    fn get_sv2_clients(&self) -> Vec<Sv2ClientInfo>;

    /// Get a single Sv2 client by client_id
    ///
    /// Default implementation does O(n) scan. Override for O(1) lookup
    /// if your implementation uses a HashMap internally.
    fn get_sv2_client_by_id(&self, client_id: usize) -> Option<Sv2ClientInfo> {
        self.get_sv2_clients()
            .into_iter()
            .find(|c| c.client_id == client_id)
    }

    /// Get summary of all Sv2 clients
    fn get_sv2_clients_summary(&self) -> Sv2ClientsSummary {
        let clients = self.get_sv2_clients();
        let extended: usize = clients.iter().map(|c| c.extended_channels.len()).sum();
        let standard: usize = clients.iter().map(|c| c.standard_channels.len()).sum();

        Sv2ClientsSummary {
            total_clients: clients.len(),
            total_channels: extended + standard,
            extended_channels: extended,
            standard_channels: standard,
            total_hashrate: clients.iter().map(|c| c.total_hashrate()).sum(),
        }
    }
}
