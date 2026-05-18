// Re-export the canonical key_utils crate so that stratum_apps::key_utils::Secp256k1PublicKey
// is the same type as key_utils::Secp256k1PublicKey from utils/key-utils.
// This avoids type mismatches in roles that parse authority keys with one import path and pass
// them into stratum-apps helpers through another.
pub use key_utils_impl::*;
