//! # Stratum V2 Stats Protocol Messages
//!
//! SV2 message types for communication between pool/translator and stats services.

// Re-export binary_sv2 as a named module so derive_codec_sv2 1.1.2 generated code
// (which uses `super::binary_sv2::...` paths) can resolve correctly.
pub use binary_sv2;
pub use binary_sv2::*;
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
