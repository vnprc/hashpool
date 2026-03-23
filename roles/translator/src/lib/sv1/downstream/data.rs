use std::{
    collections::VecDeque,
    net::SocketAddr,
    time::{Instant, SystemTime},
};
use stratum_apps::{
    stratum_core::{
        bitcoin::Target,
        sv1_api::{
            json_rpc,
            utils::{Extranonce, HexU32Be},
        },
    },
    utils::types::{ChannelId, DownstreamId, Hashrate},
};
use tracing::debug;

use super::SubmitShareWithChannelId;

#[derive(Debug)]
pub struct DownstreamData {
    pub channel_id: Option<ChannelId>,
    pub extranonce1: Extranonce<'static>,
    pub extranonce2_len: usize,
    pub target: Target,
    pub hashrate: Option<Hashrate>,
    pub version_rolling_mask: Option<HexU32Be>,
    pub version_rolling_min_bit: Option<HexU32Be>,
    pub last_job_version_field: Option<u32>,
    pub authorized_worker_name: String,
    pub user_identity: String,
    pub cached_set_difficulty: Option<json_rpc::Message>,
    pub cached_notify: Option<json_rpc::Message>,
    pub pending_target: Option<Target>,
    pub pending_hashrate: Option<Hashrate>,
    // Queue of Sv1 handshake messages received while waiting for SV2 channel to open
    pub queued_sv1_handshake_messages: Vec<json_rpc::Message>,
    // Stores pending shares to be sent to the sv1_server
    pub pending_share: Option<SubmitShareWithChannelId>,
    // Tracks the upstream target for this downstream, used for vardiff target comparison
    pub upstream_target: Option<Target>,
    // Timestamp of when the last job was received by this downstream, used for keepalive check
    pub last_job_received_time: Option<Instant>,
    // Per-miner monitoring fields
    pub shares_submitted: u64,
    pub connected_at: SystemTime,
    pub peer_address: Option<SocketAddr>,
    /// Ring buffer of share submission instants for windowed hashrate calculation (5-min window)
    pub share_timestamps: VecDeque<Instant>,
}

impl DownstreamData {
    pub fn new(hashrate: Option<Hashrate>, target: Target, peer_address: Option<SocketAddr>) -> Self {
        DownstreamData {
            channel_id: None,
            extranonce1: vec![0; 8]
                .try_into()
                .expect("8-byte extranonce is always valid"),
            extranonce2_len: 4,
            target,
            hashrate,
            version_rolling_mask: None,
            version_rolling_min_bit: None,
            last_job_version_field: None,
            authorized_worker_name: String::new(),
            user_identity: String::new(),
            cached_set_difficulty: None,
            cached_notify: None,
            pending_target: None,
            pending_hashrate: None,
            queued_sv1_handshake_messages: Vec::new(),
            pending_share: None,
            upstream_target: None,
            last_job_received_time: None,
            shares_submitted: 0,
            connected_at: SystemTime::now(),
            peer_address,
            share_timestamps: VecDeque::new(),
        }
    }

    pub fn set_pending_target(&mut self, new_target: Target, downstream_id: DownstreamId) {
        self.pending_target = Some(new_target);
        debug!("Downstream {downstream_id}: Set pending target");
    }

    pub fn set_pending_hashrate(
        &mut self,
        new_hashrate: Option<Hashrate>,
        downstream_id: DownstreamId,
    ) {
        self.pending_hashrate = new_hashrate;
        debug!("Downstream {downstream_id}: Set pending hashrate");
    }

    pub fn set_upstream_target(&mut self, upstream_target: Target, downstream_id: DownstreamId) {
        self.upstream_target = Some(upstream_target);
        debug!(
            "Downstream {downstream_id}: Set upstream target to {:?}",
            upstream_target
        );
    }

    /// Records a share submission for windowed hashrate tracking.
    pub fn record_share_submitted(&mut self) {
        self.shares_submitted += 1;
        let now = Instant::now();
        self.share_timestamps.push_back(now);
        // Prune timestamps older than 5 minutes
        let window = std::time::Duration::from_secs(300);
        while let Some(&oldest) = self.share_timestamps.front() {
            if now.duration_since(oldest) > window {
                self.share_timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    /// Computes windowed hashrate (H/s) over the last 5 minutes using the current target.
    ///
    /// Uses the SV2 formula: hashrate = 2^256 / target * shares_in_window / window_secs
    pub fn windowed_hashrate_5min(&self) -> Option<f64> {
        let shares_in_window = self.share_timestamps.len() as f64;
        if shares_in_window == 0.0 {
            return None;
        }
        // Compute how long the window actually covers (up to 5 min)
        let window_secs = self
            .share_timestamps
            .front()
            .map(|oldest| oldest.elapsed().as_secs_f64().min(300.0))
            .unwrap_or(300.0);
        if window_secs < 1.0 {
            return None;
        }
        // 2^256 as f64 (approximate)
        let max_target: f64 = 1.157920892373162e77_f64;
        // Target as big-endian u256 → f64
        let target_bytes = self.target.to_be_bytes();
        let mut target_f64: f64 = 0.0;
        for byte in target_bytes.iter() {
            target_f64 = target_f64 * 256.0 + (*byte as f64);
        }
        if target_f64 == 0.0 {
            return None;
        }
        let difficulty = max_target / target_f64;
        Some(difficulty * shares_in_window / window_secs)
    }
}
