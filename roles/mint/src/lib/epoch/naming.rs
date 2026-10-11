use anyhow::{anyhow, Result};
use std::str::FromStr;

/// secp256k1 generator point: the well-known placeholder copied from example
/// configs, refused here as a pool identity.
const GENERATOR_POINT: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

/// Validate a pool identity key: 33-byte compressed secp256k1 public key, hex.
/// Returns the normalized lowercase hex form used in unit names.
pub fn validate_pool_pubkey(hex_str: &str) -> Result<String> {
    let normalized = hex_str.trim().to_lowercase();
    bitcoin::secp256k1::PublicKey::from_str(&normalized)
        .map_err(|e| anyhow!("invalid [hashpool_mint] pool_pubkey (need compressed secp256k1 hex): {e}"))?;
    if normalized.len() != 66 {
        return Err(anyhow!(
            "[hashpool_mint] pool_pubkey must be a 33-byte compressed key (66 hex chars), got {} chars",
            normalized.len()
        ));
    }
    if normalized == GENERATOR_POINT {
        return Err(anyhow!(
            "[hashpool_mint] pool_pubkey is the secp256k1 generator point, a well-known placeholder \
             value copied from an example config, not a real pool identity; set it to your pool's own key"
        ));
    }
    Ok(normalized)
}

/// Epoch unit name: `hash_<pool>_<height>`, with a deterministic numeric suffix
/// (`_1`, `_2`, ...) when the base name is already taken (repeat rotation at the
/// same height, or a derivation-index collision reported by the mint).
pub fn unit_name(pool_pubkey_hex: &str, height: u64, suffix: u32) -> String {
    if suffix == 0 {
        format!("hash_{pool_pubkey_hex}_{height}")
    } else {
        format!("hash_{pool_pubkey_hex}_{height}_{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // secp256k1 generator point: a well-known valid compressed pubkey, but
    // refused as a pool identity (see GENERATOR_POINT).
    const G: &str = GENERATOR_POINT;
    // Dev placeholder pool identity: pubkey of sha256("hashpool dev pool
    // identity"). Not G, so it exercises the normal accept path. Pinned
    // against the committed dev config below.
    const DEV_KEY: &str = "034dcb73610a1af09a68b17e69d4e2ac8e1a05536264c2a805f027839ffd7de66a";

    #[test]
    fn accepts_valid_compressed_key_and_normalizes_case() {
        let upper = DEV_KEY.to_uppercase();
        assert_eq!(validate_pool_pubkey(&upper).unwrap(), DEV_KEY);
    }

    #[test]
    fn rejects_invalid_and_uncompressed_keys() {
        assert!(validate_pool_pubkey("02deadbeef").is_err());
        assert!(validate_pool_pubkey("not hex at all").is_err());
        // 65-byte uncompressed form must be rejected even though secp parses it.
        let uncompressed = "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8";
        assert!(validate_pool_pubkey(uncompressed).is_err());
    }

    #[test]
    fn rejects_the_generator_point_but_accepts_other_valid_keys() {
        let err = validate_pool_pubkey(G).unwrap_err().to_string();
        assert!(err.contains("pool_pubkey"), "{err}");

        assert!(validate_pool_pubkey(DEV_KEY).is_ok());
        assert!(validate_pool_pubkey(
            "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
        )
        .is_ok());
    }

    #[test]
    fn unit_names_are_deterministic_with_suffixes() {
        assert_eq!(unit_name(DEV_KEY, 905123, 0), format!("hash_{DEV_KEY}_905123"));
        assert_eq!(unit_name(DEV_KEY, 905123, 2), format!("hash_{DEV_KEY}_905123_2"));
    }

    /// The dev config's `pool_pubkey` is the pubkey of
    /// `sha256("hashpool dev pool identity")`: a key nobody's miner locks to
    /// (unlike G) but still a committed, reproducible value. Pins the
    /// derivation against the committed config so the two cannot drift.
    #[test]
    fn dev_config_pool_pubkey_is_the_dev_identity_pubkey() {
        use bitcoin::hashes::{sha256, Hash};

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/mint.config.toml");
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let configured = contents
            .lines()
            .find_map(|l| {
                let l = l.trim();
                l.strip_prefix("pool_pubkey")
                    .and_then(|rest| rest.trim_start().strip_prefix('='))
                    .map(|rest| rest.trim().trim_matches('"').to_string())
            })
            .expect("pool_pubkey must be present in the dev config");

        let digest = sha256::Hash::hash(b"hashpool dev pool identity");
        let sk = bitcoin::secp256k1::SecretKey::from_slice(digest.as_byte_array()).unwrap();
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let pk = bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &sk);
        let expected = hex::encode(pk.serialize());

        assert_eq!(configured, expected);
        assert_eq!(configured, DEV_KEY);
    }
}
