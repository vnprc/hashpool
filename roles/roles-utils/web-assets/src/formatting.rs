/// Format hash units (ehash measured in bits of difficulty) for display
pub fn format_hash_units(value: u64) -> String {
    if value == 0 {
        return "0 bits".to_string();
    }

    // Define thresholds and their corresponding units
    // Note: u64::MAX ≈ 18.4 exabits, so zettabits is unreachable
    const UNITS: &[(u64, &str, f64)] = &[
        (1_000_000_000_000_000_000, "exabits", 1e18),   // 1 exabit
        (1_000_000_000_000_000, "petabits", 1e15),      // 1 petabit
        (1_000_000_000_000, "terabits", 1e12),          // 1 terabit
        (1_000_000_000, "gigabits", 1e9),               // 1 gigabit
        (1_000_000, "megabits", 1e6),                   // 1 megabit
        (1_000, "kilobits", 1e3),                       // 1 kilobit
    ];

    // Find the appropriate unit
    for &(threshold, unit_name, divisor) in UNITS {
        if value >= threshold {
            let divided = value as f64 / divisor;
            // Format with 2 decimal places, remove trailing .00
            let formatted = format!("{:.2}", divided);
            let trimmed = if formatted.ends_with(".00") {
                &formatted[..formatted.len() - 3]
            } else {
                &formatted
            };
            return format!("{} {}", trimmed, unit_name);
        }
    }

    // For values under 1000, just show with commas
    format!("{} bits", value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_hash_units() {
        assert_eq!(format_hash_units(0), "0 bits");
        assert_eq!(format_hash_units(256), "256 bits");
        assert_eq!(format_hash_units(999), "999 bits");
        assert_eq!(format_hash_units(1_000), "1 kilobits");
        assert_eq!(format_hash_units(1_234), "1.23 kilobits");
        assert_eq!(format_hash_units(67_108_864), "67.11 megabits");
        assert_eq!(format_hash_units(2_199_023_255_552), "2.2 terabits");
        assert_eq!(format_hash_units(1_979_123_136_757_853), "1.98 petabits");
    }
}