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

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────

    fn create_extended_channel_info(channel_id: u32, hashrate: f32) -> ExtendedChannelInfo {
        ExtendedChannelInfo {
            channel_id,
            user_identity: format!("user-ext-{}", channel_id),
            nominal_hashrate: hashrate,
            target_hex: "00ff".into(),
            requested_max_target_hex: "00ff".into(),
            extranonce_prefix_hex: "aa".into(),
            full_extranonce_size: 16,
            rollable_extranonce_size: 4,
            expected_shares_per_minute: 1.0,
            shares_accepted: 10,
            share_work_sum: 100.0,
            last_share_sequence_number: 5,
            best_diff: 50.0,
            last_batch_accepted: 3,
            last_batch_work_sum: 30.0,
            share_batch_size: 10,
            blocks_found: 0,
        }
    }

    fn create_standard_channel_info(channel_id: u32, hashrate: f32) -> StandardChannelInfo {
        StandardChannelInfo {
            channel_id,
            user_identity: format!("user-std-{}", channel_id),
            nominal_hashrate: hashrate,
            target_hex: "00ff".into(),
            requested_max_target_hex: "00ff".into(),
            extranonce_prefix_hex: "bb".into(),
            expected_shares_per_minute: 2.0,
            shares_accepted: 20,
            share_work_sum: 200.0,
            last_share_sequence_number: 8,
            best_diff: 80.0,
            last_batch_accepted: 5,
            last_batch_work_sum: 50.0,
            share_batch_size: 20,
            blocks_found: 0,
        }
    }

    fn create_sv2_client_info(
        id: usize,
        ext: Vec<ExtendedChannelInfo>,
        std: Vec<StandardChannelInfo>,
    ) -> Sv2ClientInfo {
        Sv2ClientInfo {
            client_id: id,
            extended_channels: ext,
            standard_channels: std,
        }
    }

    // ── ClientInfo unit tests ───────────────────────────────────────

    #[test]
    fn client_info_empty_channels() {
        let client = create_sv2_client_info(1, vec![], vec![]);
        assert_eq!(client.total_channels(), 0);
        assert_eq!(client.total_hashrate(), 0.0);
    }

    #[test]
    fn client_info_aggregates_both_channel_types() {
        let client = create_sv2_client_info(
            1,
            vec![
                create_extended_channel_info(1, 100.0),
                create_extended_channel_info(2, 200.0),
            ],
            vec![create_standard_channel_info(3, 50.0)],
        );
        assert_eq!(client.total_channels(), 3);
        assert_eq!(client.total_hashrate(), 350.0);
    }

    #[test]
    fn client_info_to_metadata() {
        let client = create_sv2_client_info(
            42,
            vec![create_extended_channel_info(1, 100.0)],
            vec![
                create_standard_channel_info(2, 50.0),
                create_standard_channel_info(3, 75.0),
            ],
        );
        let meta = client.to_metadata();

        assert_eq!(meta.client_id, 42);
        assert_eq!(meta.extended_channels_count, 1);
        assert_eq!(meta.standard_channels_count, 2);
        assert_eq!(meta.total_hashrate, 225.0);
    }

    // ── ClientsMonitoring trait default implementations ─────────────

    struct MockClients(Vec<Sv2ClientInfo>);
    impl Sv2ClientsMonitoring for MockClients {
        fn get_sv2_clients(&self) -> Vec<Sv2ClientInfo> {
            self.0.clone()
        }
    }

    #[test]
    fn clients_monitoring_get_client_by_id_found() {
        let monitor = MockClients(vec![
            create_sv2_client_info(1, vec![create_extended_channel_info(1, 10.0)], vec![]),
            create_sv2_client_info(2, vec![], vec![create_standard_channel_info(1, 20.0)]),
        ]);
        let found = monitor.get_sv2_client_by_id(2);
        assert!(found.is_some());
        assert_eq!(found.unwrap().client_id, 2);
    }

    #[test]
    fn clients_monitoring_get_client_by_id_not_found() {
        let monitor = MockClients(vec![create_sv2_client_info(1, vec![], vec![])]);
        assert!(monitor.get_sv2_client_by_id(999).is_none());
    }

    #[test]
    fn clients_monitoring_summary_empty() {
        let monitor = MockClients(vec![]);
        let summary = monitor.get_sv2_clients_summary();

        assert_eq!(summary.total_clients, 0);
        assert_eq!(summary.total_channels, 0);
        assert_eq!(summary.extended_channels, 0);
        assert_eq!(summary.standard_channels, 0);
        assert_eq!(summary.total_hashrate, 0.0);
    }

    #[test]
    fn clients_monitoring_summary_aggregates_correctly() {
        let monitor = MockClients(vec![
            create_sv2_client_info(
                1,
                vec![create_extended_channel_info(1, 100.0)],
                vec![create_standard_channel_info(2, 50.0)],
            ),
            create_sv2_client_info(
                2,
                vec![
                    create_extended_channel_info(3, 200.0),
                    create_extended_channel_info(4, 300.0),
                ],
                vec![],
            ),
        ]);
        let summary = monitor.get_sv2_clients_summary();

        assert_eq!(summary.total_clients, 2);
        assert_eq!(summary.extended_channels, 3);
        assert_eq!(summary.standard_channels, 1);
        assert_eq!(summary.total_channels, 4);
        assert_eq!(summary.total_hashrate, 650.0);
    }
}
