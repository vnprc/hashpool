use crate::{config_helpers::opt_path_from_toml, key_utils::Secp256k1PublicKey};
use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

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
