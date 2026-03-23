//! Monitoring integration for Translation Proxy (tProxy)
//!
//! This module implements the ServerMonitoring trait on `ChannelManager`.
//! tProxy has server channels (upstream to pool) but no SV2 clients
//! (SV1 clients are handled separately in sv1_monitoring.rs).

use stratum_apps::monitoring::server::{ServerExtendedChannelInfo, ServerInfo, ServerMonitoring};

use crate::{
    sv2::channel_manager::ChannelManager, tproxy_mode, utils::AGGREGATED_CHANNEL_ID,
    vardiff_enabled, TproxyMode,
};

impl ServerMonitoring for ChannelManager {
    fn get_server(&self) -> ServerInfo {
        let mut extended_channels = Vec::new();
        let standard_channels = Vec::new(); // tProxy only uses extended channels
        let report_hashrate = vardiff_enabled();

        match tproxy_mode() {
            TproxyMode::Aggregated => {
                // In Aggregated mode: one shared channel to the server
                // stored under AGGREGATED_CHANNEL_ID
                if let Some(aggregated_extended_channel) =
                    self.extended_channels.get(&AGGREGATED_CHANNEL_ID)
                {
                    let channel_id = aggregated_extended_channel.get_channel_id();
                    let target = aggregated_extended_channel.get_target();
                    let extranonce_prefix = aggregated_extended_channel.get_extranonce_prefix();
                    let user_identity = aggregated_extended_channel.get_user_identity();
                    let share_accounting = aggregated_extended_channel.get_share_accounting();

                    extended_channels.push(ServerExtendedChannelInfo {
                        channel_id,
                        user_identity: user_identity.clone(),
                        nominal_hashrate: if report_hashrate {
                            Some(aggregated_extended_channel.get_nominal_hashrate())
                        } else {
                            None
                        },
                        target_hex: hex::encode(target.to_be_bytes()),
                        extranonce_prefix_hex: hex::encode(extranonce_prefix),
                        full_extranonce_size: aggregated_extended_channel
                            .get_full_extranonce_size(),
                        rollable_extranonce_size: aggregated_extended_channel
                            .get_rollable_extranonce_size(),
                        version_rolling: aggregated_extended_channel.is_version_rolling(),
                        shares_acknowledged: share_accounting.get_acknowledged_shares(),
                        shares_submitted: share_accounting.get_validated_shares(),
                        shares_rejected: share_accounting.get_rejected_shares(),
                        share_work_sum: share_accounting.get_share_work_sum(),
                        best_diff: share_accounting.get_best_diff(),
                        blocks_found: share_accounting.get_blocks_found(),
                    });
                }
            }
            TproxyMode::NonAggregated => {
                // In NonAggregated mode: each downstream Sv1 miner has its own upstream Sv2
                // channel to the server
                for channel in self.extended_channels.iter() {
                    let extended_channel = channel.value();

                    let channel_id = extended_channel.get_channel_id();
                    let target = extended_channel.get_target();
                    let extranonce_prefix = extended_channel.get_extranonce_prefix();
                    let user_identity = extended_channel.get_user_identity();
                    let share_accounting = extended_channel.get_share_accounting();

                    extended_channels.push(ServerExtendedChannelInfo {
                        channel_id,
                        user_identity: user_identity.clone(),
                        nominal_hashrate: if report_hashrate {
                            Some(extended_channel.get_nominal_hashrate())
                        } else {
                            None
                        },
                        target_hex: hex::encode(target.to_be_bytes()),
                        extranonce_prefix_hex: hex::encode(extranonce_prefix),
                        full_extranonce_size: extended_channel.get_full_extranonce_size(),
                        rollable_extranonce_size: extended_channel.get_rollable_extranonce_size(),
                        version_rolling: extended_channel.is_version_rolling(),
                        shares_acknowledged: share_accounting.get_acknowledged_shares(),
                        shares_submitted: share_accounting.get_validated_shares(),
                        shares_rejected: share_accounting.get_rejected_shares(),
                        share_work_sum: share_accounting.get_share_work_sum(),
                        best_diff: share_accounting.get_best_diff(),
                        blocks_found: share_accounting.get_blocks_found(),
                    });
                }
            }
        }

        ServerInfo {
            extended_channels,
            standard_channels,
        }
    }
}
