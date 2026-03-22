//! Server monitoring types
//!
//! These types are for monitoring the **server** (upstream connection).
//! An app typically has one server connection with one or more channels.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Information about an extended channel opened with the server
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerExtendedChannelInfo {
    pub channel_id: u32,
    pub user_identity: String,
    /// None when vardiff is disabled and hashrate cannot be reliably tracked
    pub nominal_hashrate: Option<f32>,
    pub target_hex: String,
    pub extranonce_prefix_hex: String,
    pub full_extranonce_size: usize,
    pub rollable_extranonce_size: u16,
    pub version_rolling: bool,
    pub shares_accepted: u32,
    pub share_work_sum: f64,
    pub shares_submitted: u32,
    pub best_diff: f64,
    pub blocks_found: u32,
}

/// Information about a standard channel opened with the server
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerStandardChannelInfo {
    pub channel_id: u32,
    pub user_identity: String,
    /// None when vardiff is disabled and hashrate cannot be reliably tracked
    pub nominal_hashrate: Option<f32>,
    pub target_hex: String,
    pub extranonce_prefix_hex: String,
    pub shares_accepted: u32,
    pub share_work_sum: f64,
    pub shares_submitted: u32,
    pub best_diff: f64,
    pub blocks_found: u32,
}

/// Information about the server (upstream connection)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerInfo {
    pub extended_channels: Vec<ServerExtendedChannelInfo>,
    pub standard_channels: Vec<ServerStandardChannelInfo>,
}

impl ServerInfo {
    /// Get total number of channels with the server
    pub fn total_channels(&self) -> usize {
        self.extended_channels.len() + self.standard_channels.len()
    }

    /// Get total hashrate across all server channels
    pub fn total_hashrate(&self) -> f32 {
        self.extended_channels
            .iter()
            .filter_map(|c| c.nominal_hashrate)
            .sum::<f32>()
            + self
                .standard_channels
                .iter()
                .filter_map(|c| c.nominal_hashrate)
                .sum::<f32>()
    }
}

/// Aggregate information about the server connection
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerSummary {
    pub total_channels: usize,
    pub extended_channels: usize,
    pub standard_channels: usize,
    pub total_hashrate: f32,
}

/// Trait for monitoring the server (upstream connection)
pub trait ServerMonitoring: Send + Sync {
    /// Get server connection info with all its channels
    fn get_server(&self) -> ServerInfo;

    /// Get summary of server connection
    fn get_server_summary(&self) -> ServerSummary {
        let server = self.get_server();

        ServerSummary {
            total_channels: server.total_channels(),
            extended_channels: server.extended_channels.len(),
            standard_channels: server.standard_channels.len(),
            total_hashrate: server.total_hashrate(),
        }
    }
}
