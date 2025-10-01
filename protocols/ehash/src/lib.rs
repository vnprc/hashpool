//! Shared helpers for Hashpool ehash calculations.
//!
//! Keeping these utilities in a dedicated crate minimizes the amount of
//! Cashu-specific logic that needs to live inside the upstream Stratum V2
//! protocol crates.

pub mod work;

pub use work::{calculate_difficulty, calculate_ehash_amount};
