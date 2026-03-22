// Re-export the canonical key_utils crate so that stratum_apps::key_utils::Secp256k1PublicKey
// is the same type as key_utils::Secp256k1PublicKey from utils/key-utils.
// This avoids type mismatches when other crates (e.g. integration tests, pool, jd-server)
// import Secp256k1PublicKey from key_utils and pass it to translator_sv2::config::Upstream.
pub use key_utils_impl::*;
