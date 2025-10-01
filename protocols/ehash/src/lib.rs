//! Shared helpers for Hashpool ehash calculations.
//!
//! Keeping these utilities in a dedicated crate minimizes the amount of
//! Cashu-specific logic that needs to live inside the upstream Stratum V2
//! protocol crates.

pub mod quote;
pub mod keyset;
pub mod work;

pub use quote::{build_mint_quote_request, QuoteBuildError};
pub use keyset::{build_cdk_keyset, calculate_keyset_id, keyset_from_sv2_bytes, signing_keys_from_cdk, signing_keys_to_cdk, KeysetConversionError, KeysetId, SigningKey};
pub use work::{calculate_difficulty, calculate_ehash_amount};
