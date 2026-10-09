use anyhow::{anyhow, Result};
use std::str::FromStr;

/// Validate a pool identity key: 33-byte compressed secp256k1 public key, hex.
/// Returns the normalized lowercase hex form used in unit names.
pub fn validate_pool_pubkey(hex_str: &str) -> Result<String> {
    let normalized = hex_str.trim().to_lowercase();
    bitcoin::secp256k1::PublicKey::from_str(&normalized)
        .map_err(|e| anyhow!("invalid pool_pubkey (need compressed secp256k1 hex): {e}"))?;
    if normalized.len() != 66 {
        return Err(anyhow!(
            "pool_pubkey must be a 33-byte compressed key (66 hex chars), got {} chars",
            normalized.len()
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

    // secp256k1 generator point: a well-known valid compressed pubkey.
    const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    #[test]
    fn accepts_valid_compressed_key_and_normalizes_case() {
        let upper = G.to_uppercase();
        assert_eq!(validate_pool_pubkey(&upper).unwrap(), G);
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
    fn unit_names_are_deterministic_with_suffixes() {
        assert_eq!(unit_name(G, 905123, 0), format!("hash_{G}_905123"));
        assert_eq!(unit_name(G, 905123, 2), format!("hash_{G}_905123_2"));
    }
}
