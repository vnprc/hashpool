//! Shared helpers for Hashpool ehash calculations.
//!
//! Keeping these utilities in a dedicated crate minimizes the amount of
//! Cashu-specific logic that needs to live inside the upstream Stratum V2
//! protocol crates.

pub mod keyset;
pub mod quote;
pub mod share;
pub mod sv2;
pub mod work;

pub use keyset::{
    build_cdk_keyset, calculate_keyset_id, keyset_from_sv2_bytes, signing_keys_from_cdk,
    signing_keys_to_cdk, KeysetConversionError, KeysetId, SigningKey,
};
pub use quote::{build_mint_quote_request, QuoteBuildError};
pub use share::{ShareHash, ShareHashError};
pub use sv2::{Sv2KeySet, Sv2KeySetWire, Sv2SigningKey};
pub use work::{calculate_difficulty, calculate_ehash_amount};
