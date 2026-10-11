use anyhow::{anyhow, Result};
use std::net::SocketAddr;

/// Validates the epoch admin listener address: a non-loopback bind is
/// refused unless `allow_non_loopback` opts in, since the rotation lever
/// behind it carries no authentication of its own (see the comment at the
/// bind site in main.rs).
pub fn validate_admin_listen(addr: &str, allow_non_loopback: bool) -> Result<SocketAddr> {
    let parsed: SocketAddr = addr.parse().map_err(|e| {
        anyhow!("[hashpool_mint] admin_listen {addr:?} is not a valid address: {e}")
    })?;
    if !parsed.ip().is_loopback() && !allow_non_loopback {
        return Err(anyhow!(
            "[hashpool_mint] admin_listen {addr} is not a loopback address; the unauthenticated \
             epoch rotation lever would be reachable from the network. Set \
             [hashpool_mint] admin_allow_non_loopback = true to opt in."
        ));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_v4_is_ok() {
        assert!(validate_admin_listen("127.0.0.1:3339", false).is_ok());
    }

    #[test]
    fn loopback_v6_is_ok() {
        assert!(validate_admin_listen("[::1]:3339", false).is_ok());
    }

    #[test]
    fn non_loopback_is_refused_and_names_the_setting() {
        let err = validate_admin_listen("0.0.0.0:3339", false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("admin_listen"), "{err}");
    }

    #[test]
    fn non_loopback_is_allowed_with_the_opt_in() {
        assert!(validate_admin_listen("0.0.0.0:3339", true).is_ok());
    }

    #[test]
    fn unparseable_is_refused() {
        assert!(validate_admin_listen("not-an-address", false).is_err());
        assert!(validate_admin_listen("not-an-address", true).is_err());
    }
}
