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
    pub shares_acknowledged: u32,
    pub shares_submitted: u32,
    pub shares_rejected: u32,
    pub share_work_sum: f64,
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────

    fn create_server_extended_channel_info(
        channel_id: u32,
        hashrate: Option<f32>,
    ) -> ServerExtendedChannelInfo {
        ServerExtendedChannelInfo {
            channel_id,
            user_identity: format!("pool-ext-{}", channel_id),
            nominal_hashrate: hashrate,
            target_hex: "00ff".into(),
            extranonce_prefix_hex: "aa".into(),
            full_extranonce_size: 16,
            rollable_extranonce_size: 4,
            version_rolling: true,
            shares_acknowledged: 10,
            shares_rejected: 0,
            share_work_sum: 100.0,
            shares_submitted: 12,
            best_diff: 50.0,
            blocks_found: 0,
        }
    }

    fn create_server_standard_channel_info(
        channel_id: u32,
        hashrate: Option<f32>,
    ) -> ServerStandardChannelInfo {
        ServerStandardChannelInfo {
            channel_id,
            user_identity: format!("pool-std-{}", channel_id),
            nominal_hashrate: hashrate,
            target_hex: "00ff".into(),
            extranonce_prefix_hex: "bb".into(),
            shares_accepted: 20,
            share_work_sum: 200.0,
            shares_submitted: 22,
            best_diff: 80.0,
            blocks_found: 0,
        }
    }

    // ── ServerInfo unit tests ───────────────────────────────────────

    #[test]
    fn server_info_empty() {
        let server = ServerInfo {
            extended_channels: vec![],
            standard_channels: vec![],
        };
        assert_eq!(server.total_channels(), 0);
        assert_eq!(server.total_hashrate(), 0.0);
    }

    #[test]
    fn server_info_aggregates_both_channel_types() {
        let server = ServerInfo {
            extended_channels: vec![create_server_extended_channel_info(1, Some(100.0))],
            standard_channels: vec![
                create_server_standard_channel_info(2, Some(50.0)),
                create_server_standard_channel_info(3, Some(75.0)),
            ],
        };
        assert_eq!(server.total_channels(), 3);
        assert_eq!(server.total_hashrate(), 225.0);
    }

    #[test]
    fn server_info_hashrate_skips_none_values() {
        let server = ServerInfo {
            extended_channels: vec![
                create_server_extended_channel_info(1, Some(100.0)),
                create_server_extended_channel_info(2, None),
            ],
            standard_channels: vec![
                create_server_standard_channel_info(3, Some(50.0)),
                create_server_standard_channel_info(4, None),
            ],
        };
        assert_eq!(server.total_channels(), 4);
        assert_eq!(server.total_hashrate(), 150.0);
    }

    // ── ServerMonitoring trait default implementations ───────────────

    struct MockServer(ServerInfo);
    impl ServerMonitoring for MockServer {
        fn get_server(&self) -> ServerInfo {
            self.0.clone()
        }
    }

    #[test]
    fn server_monitoring_summary_empty() {
        let monitor = MockServer(ServerInfo {
            extended_channels: vec![],
            standard_channels: vec![],
        });
        let summary = monitor.get_server_summary();

        assert_eq!(summary.total_channels, 0);
        assert_eq!(summary.extended_channels, 0);
        assert_eq!(summary.standard_channels, 0);
        assert_eq!(summary.total_hashrate, 0.0);
    }

    #[test]
    fn server_monitoring_summary_aggregates_correctly() {
        let monitor = MockServer(ServerInfo {
            extended_channels: vec![
                create_server_extended_channel_info(1, Some(100.0)),
                create_server_extended_channel_info(2, Some(200.0)),
            ],
            standard_channels: vec![create_server_standard_channel_info(3, Some(50.0))],
        });
        let summary = monitor.get_server_summary();

        assert_eq!(summary.total_channels, 3);
        assert_eq!(summary.extended_channels, 2);
        assert_eq!(summary.standard_channels, 1);
        assert_eq!(summary.total_hashrate, 350.0);
    }
}
