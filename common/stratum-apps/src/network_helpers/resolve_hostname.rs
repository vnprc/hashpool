use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tracing::{debug, info};

/// Maximum time to wait for a DNS lookup before giving up.
/// DNS resolution should complete in milliseconds on a healthy network;
/// 5 seconds is generous enough for slow links while still failing fast
/// when DNS is broken.
const DNS_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors that can occur during address resolution.
#[derive(Debug)]
pub enum ResolveError {
    /// DNS lookup returned no results for the given hostname.
    NoResults(String),
    /// DNS lookup failed with an IO error.
    LookupFailed(std::io::Error),
    /// DNS lookup did not complete within the timeout.
    Timeout(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NoResults(host) => {
                write!(f, "DNS resolution returned no results for '{host}'")
            }
            ResolveError::LookupFailed(e) => write!(f, "DNS resolution failed: {e}"),
            ResolveError::Timeout(host) => {
                write!(
                    f,
                    "DNS resolution for '{host}' timed out after {}s",
                    DNS_TIMEOUT.as_secs()
                )
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolves a host string and port to a [`SocketAddr`].
///
/// This function first attempts to parse the host as an IP address (fast path, no DNS).
/// If that fails, it performs an async DNS lookup via [`tokio::net::lookup_host`].
///
/// This should be called at connection time (not config parse time) so that DNS changes
/// are picked up on reconnection attempts.
///
/// # Examples
///
/// ```ignore
/// // IP address (fast path)
/// let addr = resolve_host("127.0.0.1", 3333).await?;
///
/// // Hostname (DNS lookup)
/// let addr = resolve_host("pool.example.com", 3333).await?;
/// ```
pub async fn resolve_host(host: &str, port: u16) -> Result<SocketAddr, ResolveError> {
    // Fast path: try parsing as an IP address directly (no DNS needed)
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }

    // Slow path: perform async DNS resolution
    info!("Resolving hostname '{host}' via DNS...");
    let lookup = format!("{host}:{port}");
    let addr = tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host(&lookup))
        .await
        .map_err(|_| ResolveError::Timeout(host.to_string()))?
        .map_err(ResolveError::LookupFailed)?
        // DNS can return multiple addresses; take the first one
        .next()
        .ok_or_else(|| ResolveError::NoResults(host.to_string()))?;

    debug!("Resolved '{host}' -> {addr}");
    Ok(addr)
}

/// Resolves a `"host:port"` string to a [`SocketAddr`].
///
/// Accepts both IP addresses and hostnames in the `"host:port"` format.
/// For hostnames, performs async DNS resolution via [`tokio::net::lookup_host`].
///
/// # Examples
///
/// ```ignore
/// // IP address (fast path)
/// let addr = resolve_host_port("127.0.0.1:3333").await?;
///
/// // Hostname (DNS lookup)
/// let addr = resolve_host_port("pool.example.com:3333").await?;
/// ```
pub async fn resolve_host_port(addr: &str) -> Result<SocketAddr, ResolveError> {
    // Fast path: try parsing as a SocketAddr directly (no DNS needed)
    if let Ok(socket) = addr.parse::<SocketAddr>() {
        return Ok(socket);
    }

    // Slow path: perform async DNS resolution
    info!("Resolving address '{addr}' via DNS...");
    let resolved = tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host(addr))
        .await
        .map_err(|_| ResolveError::Timeout(addr.to_string()))?
        .map_err(ResolveError::LookupFailed)?
        // DNS can return multiple addresses; take the first one
        .next()
        .ok_or_else(|| ResolveError::NoResults(addr.to_string()))?;

    debug!("Resolved '{addr}' -> {resolved}");
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_ipv4_address() {
        let addr = resolve_host("127.0.0.1", 3333).await.unwrap();
        assert_eq!(addr, SocketAddr::new("127.0.0.1".parse().unwrap(), 3333));
    }

    #[tokio::test]
    async fn resolve_ipv6_address() {
        let addr = resolve_host("::1", 3333).await.unwrap();
        assert_eq!(addr, SocketAddr::new("::1".parse().unwrap(), 3333));
    }

    #[tokio::test]
    async fn resolve_localhost_hostname() {
        let addr = resolve_host("localhost", 3333).await.unwrap();
        // localhost can resolve to either 127.0.0.1 or ::1 depending on the system
        assert_eq!(addr.port(), 3333);
        assert!(addr.ip().is_loopback());
    }

    #[tokio::test]
    async fn resolve_invalid_hostname_fails() {
        let result = resolve_host("this.hostname.definitely.does.not.exist.invalid", 3333).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_host_port_ipv4() {
        let addr = resolve_host_port("127.0.0.1:3333").await.unwrap();
        assert_eq!(addr, SocketAddr::new("127.0.0.1".parse().unwrap(), 3333));
    }

    #[tokio::test]
    async fn resolve_host_port_localhost() {
        let addr = resolve_host_port("localhost:3333").await.unwrap();
        assert_eq!(addr.port(), 3333);
        assert!(addr.ip().is_loopback());
    }

    #[tokio::test]
    async fn resolve_host_port_invalid_fails() {
        let result =
            resolve_host_port("this.hostname.definitely.does.not.exist.invalid:3333").await;
        assert!(result.is_err());
    }
}
