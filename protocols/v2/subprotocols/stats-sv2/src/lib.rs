//! # Stratum V2 Stats Protocol Messages
//!
//! SV2 message types for communication between pool/translator and stats services.

pub use binary_sv2::binary_codec_sv2::{self, Decodable as Deserialize, Encodable as Serialize, *};
pub use derive_codec_sv2::{Decodable as Deserialize, Encodable as Serialize};

mod pool_stats;
mod proxy_stats;

// Pool stats messages
pub use pool_stats::{
    ChannelClosed, ChannelOpened, DownstreamConnected, DownstreamDisconnected, QuoteCreated,
    ShareSubmitted,
};

// Proxy stats messages
pub use proxy_stats::{
    MinerConnected, MinerDisconnected, MinerHashrateUpdate, MinerShareSubmitted,
};
