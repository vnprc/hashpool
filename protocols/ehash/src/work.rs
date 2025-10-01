//! Utilities for computing ehash related values.

/// Calculate ehash units using exponential valuation (2^n) where
/// `n = leading_zero_bits - min_leading_zeros`.
///
/// * `hash` - 32-byte header hash produced by the miner.
/// * `min_leading_zeros` - Minimum required leading zero bits that earns one
///   unit of ehash.
///
/// Returns the work value as `2^(leading_zeros - min_leading_zeros)` and caps at
/// `2^63` to stay within `u64`.
pub fn calculate_ehash_amount(hash: [u8; 32], min_leading_zeros: u32) -> u64 {
    let leading_zero_bits = calculate_difficulty(hash);

    if leading_zero_bits < min_leading_zeros {
        return 0;
    }

    let relative_difficulty = leading_zero_bits - min_leading_zeros;

    if relative_difficulty >= 63 {
        1u64 << 63
    } else {
        1u64 << relative_difficulty
    }
}

/// Count the number of leading zero bits in the supplied hash.
pub fn calculate_difficulty(hash: [u8; 32]) -> u32 {
    let mut count = 0u32;

    for byte in hash {
        if byte == 0 {
            count += 8;
        } else {
            count += byte.leading_zeros();
            break;
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::{calculate_difficulty, calculate_ehash_amount};

    const MIN_DIFFICULTY: u32 = 32;

    #[test]
    fn below_threshold_returns_zero() {
        let mut hash = [0xffu8; 32];
        hash[0] = 0x0f; // four leading zero bits (below threshold)
        assert_eq!(calculate_ehash_amount(hash, MIN_DIFFICULTY), 0);
    }

    #[test]
    fn threshold_returns_one() {
        let mut hash = [0xffu8; 32];
        hash[..4].fill(0x00);
        hash[4] = 0x0f; // 32 leading zeros
        assert_eq!(calculate_ehash_amount(hash, MIN_DIFFICULTY), 1);
    }

    #[test]
    fn additional_bits_scale_exponentially() {
        let mut hash = [0xffu8; 32];
        hash[..4].fill(0x00);
        hash[4] = 0x00;
        hash[5] = 0x0f; // 40 leading zeros => 8 above threshold
        assert_eq!(calculate_ehash_amount(hash, MIN_DIFFICULTY), 256);

        let mut hash_48 = [0xffu8; 32];
        hash_48[..6].fill(0x00);
        hash_48[6] = 0x80; // 48 leading zeros => 16 above threshold
        assert_eq!(calculate_ehash_amount(hash_48, MIN_DIFFICULTY), 65_536);
    }

    #[test]
    fn caps_at_maximum_value() {
        let mut hash = [0u8; 32];
        hash[12] = 1; // 96 leading zeros
        assert_eq!(calculate_ehash_amount(hash, MIN_DIFFICULTY), 1u64 << 63);

        assert_eq!(calculate_ehash_amount([0u8; 32], MIN_DIFFICULTY), 1u64 << 63);
    }

    #[test]
    fn difficulty_counts_leading_zeros_correctly() {
        assert_eq!(calculate_difficulty([0u8; 32]), 256);

        let mut one = [0u8; 32];
        one[31] = 1;
        assert_eq!(calculate_difficulty(one), 255);

        let mut sixteen = [0u8; 32];
        sixteen[31] = 0x10; // 4 leading zeros in the last byte
        assert_eq!(calculate_difficulty(sixteen), 251);

        let mut first_bit = [0u8; 32];
        first_bit[0] = 0x80;
        assert_eq!(calculate_difficulty(first_bit), 0);

        let mut second_bit = [0u8; 32];
        second_bit[0] = 0x40;
        assert_eq!(calculate_difficulty(second_bit), 1);
    }

    #[test]
    fn reward_varies_with_threshold() {
        let mut hash = [0u8; 32];
        hash[5] = 0x80; // 40 leading zeros

        assert_eq!(calculate_ehash_amount(hash, 32), 256);
        assert_eq!(calculate_ehash_amount(hash, 35), 32);
        assert_eq!(calculate_ehash_amount(hash, 45), 0);
    }
}
