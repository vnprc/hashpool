use super::*;
use core::convert::TryInto;

/// Share submitted by a downstream connection
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareSubmitted {
    pub downstream_id: u32,
    pub timestamp: u64,
}

/// Quote created for a share
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteCreated {
    pub downstream_id: u32,
    pub amount: u64,
    pub timestamp: u64,
}

/// Mining channel opened
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelOpened {
    pub downstream_id: u32,
    pub channel_id: u32,
}

/// Mining channel closed
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelClosed {
    pub downstream_id: u32,
    pub channel_id: u32,
}

/// Downstream connection established
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownstreamConnected {
    pub downstream_id: u32,
    pub flags: u32,
}

/// Downstream connection closed
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownstreamDisconnected {
    pub downstream_id: u32,
}
