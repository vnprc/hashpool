use super::*;
use core::convert::TryInto;

/// Miner connected to the proxy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinerConnected<'decoder> {
    pub miner_id: u32,
    pub name: Str0255<'decoder>,
}

/// Miner disconnected from the proxy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinerDisconnected {
    pub miner_id: u32,
}

/// Share submitted by a miner
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinerShareSubmitted {
    pub miner_id: u32,
    /// Difficulty as integer (multiply float by 1000 to preserve precision)
    pub difficulty_millis: u32,
    pub timestamp: u64,
}

/// Hashrate update for a miner
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinerHashrateUpdate {
    pub miner_id: u32,
    /// Hashrate in H/s as integer
    pub hashrate: u64,
    pub timestamp: u64,
}
