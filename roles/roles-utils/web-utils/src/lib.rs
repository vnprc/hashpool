/// Format elapsed time in human-readable format
/// Used in both pool and proxy web dashboards
pub fn format_elapsed_time(now: u64, timestamp: u64) -> String {
    let elapsed = now.saturating_sub(timestamp);
    if elapsed < 60 {
        format!("{}s ago", elapsed)
    } else if elapsed < 3600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86400 {
        format!("{}h ago", elapsed / 3600)
    } else {
        format!("{}d ago", elapsed / 86400)
    }
}

/// Format hashrate with appropriate unit (H/s, KH/s, MH/s, GH/s, TH/s)
pub fn format_hashrate(hashrate: f64) -> String {
    if hashrate >= 1_000_000_000_000.0 {
        format!("{:.1} TH/s", hashrate / 1_000_000_000_000.0)
    } else if hashrate >= 1_000_000_000.0 {
        format!("{:.1} GH/s", hashrate / 1_000_000_000.0)
    } else if hashrate >= 1_000_000.0 {
        format!("{:.1} MH/s", hashrate / 1_000_000.0)
    } else if hashrate >= 1_000.0 {
        format!("{:.1} KH/s", hashrate / 1_000.0)
    } else {
        format!("{:.1} H/s", hashrate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_elapsed_seconds() {
        assert_eq!(format_elapsed_time(100, 50), "50s ago");
    }

    #[test]
    fn test_format_elapsed_minutes() {
        assert_eq!(format_elapsed_time(3590, 0), "59m ago");
    }

    #[test]
    fn test_format_hashrate_hs() {
        assert_eq!(format_hashrate(500.5), "500.5 H/s");
    }

    #[test]
    fn test_format_hashrate_khs() {
        assert_eq!(format_hashrate(1_500_000.0), "1.5 MH/s");
    }

    #[test]
    fn test_format_hashrate_th() {
        assert_eq!(format_hashrate(2_000_000_000_000.0), "2.0 TH/s");
    }
}
