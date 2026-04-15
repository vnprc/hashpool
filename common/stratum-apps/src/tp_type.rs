use crate::{config_helpers::opt_path_from_toml, key_utils::Secp256k1PublicKey};
use std::path::PathBuf;

/// Valid Bitcoin network names (bitcoin-cli / `getblockchaininfo` convention).
/// Covers all values that `network_from_tp_port` and `BitcoinNetwork::as_network_str` can return.
pub const VALID_NETWORKS: &[&str] = &["main", "test", "testnet4", "signet", "regtest"];

/// Maps a well-known sv2-tp default port to a Bitcoin network name.
/// Port assignments from `man sv2-tp`:
///   8442 → main, 18442 → test, 48442 → testnet4,
///   38442 → signet, 18447 → regtest
///
/// Returns `None` for non-standard ports.
pub fn network_from_tp_port(port: u16) -> Option<&'static str> {
    match port {
        8442 => Some("main"),
        18442 => Some("test"),
        48442 => Some("testnet4"),
        38442 => Some("signet"),
        18447 => Some("regtest"),
        _ => None,
    }
}

/// Bitcoin network for determining node.sock location
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BitcoinNetwork {
    Mainnet,
    Testnet4,
    Signet,
    Regtest,
}

impl BitcoinNetwork {
    /// Returns the bitcoin-cli / `getblockchaininfo` network name for this network.
    pub fn as_network_str(&self) -> &'static str {
        match self {
            BitcoinNetwork::Mainnet => "main",
            BitcoinNetwork::Testnet4 => "testnet4",
            BitcoinNetwork::Signet => "signet",
            BitcoinNetwork::Regtest => "regtest",
        }
    }

    /// Returns the subdirectory name for this network.
    /// Mainnet uses the root data directory.
    fn subdir(&self) -> Option<&'static str> {
        match self {
            BitcoinNetwork::Mainnet => None,
            BitcoinNetwork::Testnet4 => Some("testnet4"),
            BitcoinNetwork::Signet => Some("signet"),
            BitcoinNetwork::Regtest => Some("regtest"),
        }
    }
}

/// Returns the default Bitcoin Core data directory for the current OS.
fn default_bitcoin_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        dirs::home_dir().map(|h| h.join(".bitcoin"))
    }
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Application Support/Bitcoin"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Resolves the IPC socket path from network and optional data_dir.
/// Constructs path from network + optional data_dir (or OS default).
///
/// Returns `None` if data_dir cannot be determined (neither provided nor OS default available).
pub fn resolve_ipc_socket_path(
    network: &BitcoinNetwork,
    data_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    let base_dir = data_dir.or_else(default_bitcoin_data_dir)?;

    Some(match network.subdir() {
        Some(subdir) => base_dir.join(subdir).join("node.sock"),
        None => base_dir.join("node.sock"),
    })
}

/// Which type of Template Provider will be used,
/// along with the relevant config parameters for each.
#[derive(Clone, Debug, serde::Deserialize)]
pub enum TemplateProviderType {
    Sv2Tp {
        address: String,
        public_key: Option<Secp256k1PublicKey>,
    },
    BitcoinCoreIpc {
        /// Network for determining socket path subdirectory.
        network: BitcoinNetwork,
        /// Custom Bitcoin data directory. Uses OS default if not set.
        #[serde(default, deserialize_with = "opt_path_from_toml")]
        data_dir: Option<PathBuf>,
        fee_threshold: u64,
        min_interval: u8,
    },
}

impl TemplateProviderType {
    /// Infer the Bitcoin network name from this TP configuration.
    ///
    /// - `Sv2Tp`: maps the address port to a network using `network_from_tp_port`.
    ///   Returns `None` for non-standard ports.
    /// - `BitcoinCoreIpc`: returns the network directly from the `BitcoinNetwork` enum.
    pub fn infer_network(&self) -> Option<&'static str> {
        match self {
            TemplateProviderType::Sv2Tp { address, .. } => address
                .parse::<std::net::SocketAddr>()
                .ok()
                .and_then(|a| network_from_tp_port(a.port())),
            TemplateProviderType::BitcoinCoreIpc { network, .. } => {
                Some(network.as_network_str())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_from_tp_port_known_ports() {
        assert_eq!(network_from_tp_port(8442), Some("main"));
        assert_eq!(network_from_tp_port(18442), Some("test"));
        assert_eq!(network_from_tp_port(48442), Some("testnet4"));
        assert_eq!(network_from_tp_port(38442), Some("signet"));
        assert_eq!(network_from_tp_port(18447), Some("regtest"));
    }

    #[test]
    fn network_from_tp_port_unknown_returns_none() {
        assert_eq!(network_from_tp_port(4444), None);
        assert_eq!(network_from_tp_port(0), None);
        assert_eq!(network_from_tp_port(3333), None);
    }

    #[test]
    fn valid_networks_covers_all_tp_port_outputs() {
        for port in [8442u16, 18442, 48442, 38442, 18447] {
            let name = network_from_tp_port(port).unwrap();
            assert!(
                VALID_NETWORKS.contains(&name),
                "port {port} maps to {name:?} which is not in VALID_NETWORKS"
            );
        }
    }

    #[test]
    fn bitcoin_network_as_network_str() {
        assert_eq!(BitcoinNetwork::Mainnet.as_network_str(), "main");
        assert_eq!(BitcoinNetwork::Testnet4.as_network_str(), "testnet4");
        assert_eq!(BitcoinNetwork::Signet.as_network_str(), "signet");
        assert_eq!(BitcoinNetwork::Regtest.as_network_str(), "regtest");
    }

    #[test]
    fn valid_networks_covers_all_bitcoin_network_outputs() {
        for network in [
            BitcoinNetwork::Mainnet,
            BitcoinNetwork::Testnet4,
            BitcoinNetwork::Signet,
            BitcoinNetwork::Regtest,
        ] {
            let name = network.as_network_str();
            assert!(
                VALID_NETWORKS.contains(&name),
                "{name:?} is not in VALID_NETWORKS"
            );
        }
    }

    #[test]
    fn infer_network_sv2tp_standard_ports() {
        let tp = TemplateProviderType::Sv2Tp {
            address: "127.0.0.1:18447".to_string(),
            public_key: None,
        };
        assert_eq!(tp.infer_network(), Some("regtest"));

        let tp = TemplateProviderType::Sv2Tp {
            address: "127.0.0.1:8442".to_string(),
            public_key: None,
        };
        assert_eq!(tp.infer_network(), Some("main"));
    }

    #[test]
    fn infer_network_sv2tp_nonstandard_port_returns_none() {
        let tp = TemplateProviderType::Sv2Tp {
            address: "127.0.0.1:4444".to_string(),
            public_key: None,
        };
        assert_eq!(tp.infer_network(), None);
    }

    #[test]
    fn infer_network_bitcoin_core_ipc() {
        let tp = TemplateProviderType::BitcoinCoreIpc {
            network: BitcoinNetwork::Regtest,
            data_dir: None,
            fee_threshold: 0,
            min_interval: 5,
        };
        assert_eq!(tp.infer_network(), Some("regtest"));

        let tp = TemplateProviderType::BitcoinCoreIpc {
            network: BitcoinNetwork::Mainnet,
            data_dir: None,
            fee_threshold: 0,
            min_interval: 5,
        };
        assert_eq!(tp.infer_network(), Some("main"));
    }

    #[test]
    fn network_with_data_dir_mainnet() {
        let result =
            resolve_ipc_socket_path(&BitcoinNetwork::Mainnet, Some(PathBuf::from("/data")));
        assert_eq!(result, Some(PathBuf::from("/data/node.sock")));
    }

    #[test]
    fn network_with_data_dir_regtest() {
        let result =
            resolve_ipc_socket_path(&BitcoinNetwork::Regtest, Some(PathBuf::from("/data")));
        assert_eq!(result, Some(PathBuf::from("/data/regtest/node.sock")));
    }

    #[test]
    fn network_with_data_dir_signet() {
        let result = resolve_ipc_socket_path(&BitcoinNetwork::Signet, Some(PathBuf::from("/data")));
        assert_eq!(result, Some(PathBuf::from("/data/signet/node.sock")));
    }

    #[test]
    fn network_with_data_dir_testnet4() {
        let result =
            resolve_ipc_socket_path(&BitcoinNetwork::Testnet4, Some(PathBuf::from("/data")));
        assert_eq!(result, Some(PathBuf::from("/data/testnet4/node.sock")));
    }

    #[test]
    fn missing_data_dir_uses_os_default() {
        // This test verifies behavior when data_dir is None
        // Result depends on OS - will be Some on Linux/macOS, None on unsupported OS
        let result = resolve_ipc_socket_path(&BitcoinNetwork::Regtest, None);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert!(result.is_some());
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        assert!(result.is_none());
    }
}
