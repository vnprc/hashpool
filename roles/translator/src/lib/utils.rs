use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

use stratum_apps::{
    key_utils::Secp256k1PublicKey,
    stratum_core::{
        binary_sv2::{Sv2DataType, U256},
        bitcoin::{
            block::{Header, Version},
            hashes::Hash,
            CompactTarget, Target, TxMerkleNode,
        },
        channels_sv2::{
            merkle_root::merkle_root_from_path,
            target::{bytes_to_hex, u256_to_block_hash},
        },
        sv1_api::{client_to_server, server_to_client::Notify, utils::HexU32Be},
    },
    utils::types::ChannelId,
};

use tracing::debug;

use crate::error::TproxyErrorKind;

/// Channel ID used to broadcast messages to all downstreams in aggregated mode.
/// This sentinel value distinguishes broadcast from a legitimate channel 0.
pub const AGGREGATED_CHANNEL_ID: ChannelId = u32::MAX;

/// Validates an SV1 share against the target difficulty and job parameters.
///
/// This function performs complete share validation by:
/// 1. Finding the corresponding job from the valid jobs storage
/// 2. Constructing the full extranonce from extranonce1 and extranonce2
/// 3. Calculating the merkle root from the coinbase transaction and merkle path
/// 4. Building the block header with the share's nonce and timestamp
/// 5. Hashing the header and comparing against the target difficulty
///
/// # Arguments
/// * `share` - The SV1 submit message containing the share data
/// * `target` - The target difficulty for this share
/// * `extranonce1` - The first part of the extranonce (from server)
/// * `version_rolling_mask` - Optional mask for version rolling
/// * `sv1_server_data` - Reference to shared SV1 server data for accessing valid jobs
/// * `channel_id` - Channel ID for job lookup
///
/// # Returns
/// * `Ok(true)` if the share is valid and meets the target
/// * `Ok(false)` if the share is valid but doesn't meet the target
/// * `Err(TproxyError)` if validation fails due to missing job or invalid data
pub fn validate_sv1_share(
    share: &client_to_server::Submit<'static>,
    target: Target,
    extranonce1: Vec<u8>,
    version_rolling_mask: Option<HexU32Be>,
    job: Notify<'static>,
) -> Result<bool, TproxyErrorKind> {
    let mut full_extranonce = vec![];
    full_extranonce.extend_from_slice(extranonce1.as_slice());
    full_extranonce.extend_from_slice(share.extra_nonce2.0.as_ref());

    let share_version = share
        .version_bits
        .clone()
        .map(|vb| vb.0)
        .unwrap_or(job.version.0);
    let mask = version_rolling_mask.unwrap_or(HexU32Be(0x1FFFE000_u32)).0;
    let version = (job.version.0 & !mask) | (share_version & mask);

    let prev_hash_vec: Vec<u8> = job.prev_hash.clone().into();
    let prev_hash = U256::from_vec_(prev_hash_vec).map_err(TproxyErrorKind::BinarySv2)?;

    // calculate the merkle root from:
    // - job coinbase_tx_prefix
    // - full extranonce
    // - job coinbase_tx_suffix
    // - job merkle_path
    let merkle_root: [u8; 32] = merkle_root_from_path(
        job.coin_base1.as_ref(),
        job.coin_base2.as_ref(),
        full_extranonce.as_ref(),
        job.merkle_branch.as_ref(),
    )
    .ok_or(TproxyErrorKind::InvalidMerkleRoot)?
    .try_into()
    .map_err(|_| TproxyErrorKind::InvalidMerkleRoot)?;

    // create the header for validation
    let header = Header {
        version: Version::from_consensus(version as i32),
        prev_blockhash: u256_to_block_hash(prev_hash),
        merkle_root: TxMerkleNode::from_byte_array(merkle_root),
        time: share.time.0,
        bits: CompactTarget::from_consensus(job.bits.0),
        nonce: share.nonce.0,
    };

    // convert the header hash to a target type for easy comparison
    let hash = header.block_hash();
    let raw_hash: [u8; 32] = *hash.to_raw_hash().as_ref();
    let hash_as_target = Target::from_le_bytes(raw_hash);

    // print hash_as_target and self.target as human readable hex
    let hash_bytes = hash_as_target.to_be_bytes();
    let target_bytes = target.to_be_bytes();

    debug!(
        "share validation \nshare:\t\t{}\ndownstream target:\t{}\n",
        bytes_to_hex(&hash_bytes),
        bytes_to_hex(&target_bytes),
    );
    // check if the share hash meets the downstream target
    if hash_as_target < target {
        /*if self.share_accounting.is_share_seen(hash.to_raw_hash()) {
            return Err(ShareValidationError::DuplicateShare);
        }*/

        return Ok(true);
    }

    Ok(false)
}

/// Calculates the required length of the proxy's extranonce prefix.
///
/// This function determines how many bytes the proxy needs to reserve for its own
/// extranonce prefix, based on the difference between the channel's rollable extranonce
/// size and the downstream miner's rollable extranonce size.
///
/// # Arguments
/// * `channel_rollable_extranonce_size` - Size of the rollable extranonce from the channel
/// * `downstream_rollable_extranonce_size` - Size of the rollable extranonce for downstream
///
/// # Returns
/// The number of bytes needed for the proxy's extranonce prefix
pub fn proxy_extranonce_prefix_len(
    channel_rollable_extranonce_size: usize,
    downstream_rollable_extranonce_size: usize,
) -> usize {
    channel_rollable_extranonce_size - downstream_rollable_extranonce_size
}

/// Tracks the state of the single upstream extended channel in aggregated mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregatedState {
    /// No upstream channel has been opened yet.
    NoChannel = 0,
    /// An `OpenExtendedMiningChannel` has been sent and we are waiting for the success response.
    Pending = 1,
    /// The upstream channel is open and ready to accept new downstream connections directly.
    Connected = 2,
}

/// Atomic wrapper around `UpstreamState` for lock-free state transitions.
#[derive(Clone, Debug)]
pub struct AtomicAggregatedState {
    inner: Arc<AtomicU8>,
}

impl AtomicAggregatedState {
    pub fn new(state: AggregatedState) -> Self {
        Self {
            inner: Arc::new(AtomicU8::new(state as u8)),
        }
    }

    pub fn get(&self) -> AggregatedState {
        match self.inner.load(Ordering::SeqCst) {
            0 => AggregatedState::NoChannel,
            1 => AggregatedState::Pending,
            2 => AggregatedState::Connected,
            v => panic!("Invalid UpstreamState value: {v}"),
        }
    }

    pub fn set(&self, state: AggregatedState) {
        self.inner.store(state as u8, Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub struct UpstreamEntry {
    /// Upstream host — can be an IP address or a hostname (resolved at connection time).
    pub host: String,
    pub port: u16,
    pub authority_pubkey: Secp256k1PublicKey,
    pub tried_or_flagged: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_extranonce_prefix_len() {
        assert_eq!(proxy_extranonce_prefix_len(8, 4), 4);
        assert_eq!(proxy_extranonce_prefix_len(10, 6), 4);
        assert_eq!(proxy_extranonce_prefix_len(4, 4), 0);
    }
}
