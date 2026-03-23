//! ## Translator Configuration Module
//!
//! Defines [`TranslatorConfig`], the primary configuration structure for the Translator.
//!
//! This module provides the necessary structures to configure the Translator,
//! managing connections and settings for both upstream and downstream interfaces.
//!
//! This module handles:
//! - Upstream server address, port, and authentication key ([`UpstreamConfig`])
//! - Downstream interface address and port ([`DownstreamConfig`])
//! - Supported protocol versions
//! - Downstream difficulty adjustment parameters ([`DownstreamDifficultyConfig`])
use std::path::{Path, PathBuf};

use serde::Deserialize;
use std::net::SocketAddr;
use stratum_apps::{
    config_helpers::opt_path_from_toml,
    key_utils::Secp256k1PublicKey,
    utils::types::{Hashrate, SharesPerMinute},
};

/// CDK wallet configuration for managing ehash tokens.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct WalletConfig {
    pub mnemonic: String,
    pub db_path: String,
    /// Optional locking public key (hex-encoded secp256k1 compressed pubkey).
    pub locking_pubkey: Option<String>,
    /// Optional locking private key (hex-encoded 32-byte secret). If set, derives pubkey.
    pub locking_privkey: Option<String>,
}

impl WalletConfig {
    /// Validate the wallet config and derive pubkey from privkey if needed.
    pub fn initialize(&mut self) -> Result<(), String> {
        match (&self.locking_pubkey, &self.locking_privkey) {
            (None, None) => Err(
                "Either locking_pubkey or locking_privkey must be provided".to_string(),
            ),
            (pubkey_opt, Some(privkey)) => {
                use bitcoin::secp256k1::{Secp256k1, SecretKey};
                let privkey_bytes =
                    hex::decode(privkey).map_err(|_| "Invalid private key hex format")?;
                if privkey_bytes.len() != 32 {
                    return Err("Private key must be 32 bytes".to_string());
                }
                let secp = Secp256k1::new();
                let secret_key = SecretKey::from_slice(&privkey_bytes)
                    .map_err(|_| "Invalid private key")?;
                let public_key = secret_key.public_key(&secp);
                let derived_pubkey = hex::encode(public_key.serialize());
                if let Some(provided_pubkey) = pubkey_opt {
                    if provided_pubkey != &derived_pubkey {
                        return Err(
                            "Provided locking_pubkey does not match locking_privkey".to_string(),
                        );
                    }
                } else {
                    self.locking_pubkey = Some(derived_pubkey);
                }
                Ok(())
            }
            (Some(pubkey), None) => {
                use bitcoin::secp256k1::{PublicKey, Secp256k1};
                let pubkey_bytes =
                    hex::decode(pubkey).map_err(|_| "Invalid public key hex format")?;
                let _secp = Secp256k1::new();
                PublicKey::from_slice(&pubkey_bytes)
                    .map_err(|_| "Invalid public key format")?;
                Ok(())
            }
        }
    }
}

/// Mint service configuration for quote operations.
#[derive(Debug, Deserialize, Clone)]
pub struct MintConfig {
    pub url: String,
}

/// Configuration for the Translator.
#[derive(Debug, Deserialize, Clone)]
pub struct TranslatorConfig {
    pub upstreams: Vec<Upstream>,
    /// The address for the downstream interface.
    pub downstream_address: String,
    /// The port for the downstream interface.
    pub downstream_port: u16,
    /// The maximum supported protocol version for communication.
    pub max_supported_version: u16,
    /// The minimum supported protocol version for communication.
    pub min_supported_version: u16,
    /// The size of the extranonce2 field for downstream mining connections.
    pub downstream_extranonce2_size: u16,
    /// The user identity/username to use when connecting to the pool.
    /// This will be appended with a counter for each mining channel (e.g., username.miner1,
    /// username.miner2).
    pub user_identity: String,
    /// Configuration settings for managing difficulty on the downstream connection.
    pub downstream_difficulty_config: DownstreamDifficultyConfig,
    /// Whether to aggregate all downstream connections into a single upstream channel.
    /// If true, all miners share one channel. If false, each miner gets its own channel.
    pub aggregate_channels: bool,
    /// Protocol extensions that the translator supports (will request if supported by server).
    #[serde(default)]
    pub supported_extensions: Vec<u16>,
    /// Protocol extensions that the translator requires (server must support these).
    /// If the upstream server doesn't support these, the translator will fail over to another
    /// upstream.
    #[serde(default)]
    pub required_extensions: Vec<u16>,
    /// The path to the log file for the Translator.
    #[serde(default, deserialize_with = "opt_path_from_toml")]
    log_file: Option<PathBuf>,
    /// Optional monitoring server bind address
    #[serde(default)]
    monitoring_address: Option<SocketAddr>,
    #[serde(default)]
    monitoring_cache_refresh_secs: Option<u64>,
    // --- Hashpool CDK payment fields ---
    /// CDK wallet configuration (mnemonic, db_path, locking keys).
    #[serde(default)]
    pub wallet: WalletConfig,
    /// Mint service URL. If absent, CDK payment is disabled.
    pub mint: Option<MintConfig>,
    /// Faucet HTTP port for ehash token dispensing.
    #[serde(default = "default_faucet_port")]
    pub faucet_port: u16,
    /// Faucet rate-limit timeout in seconds.
    #[serde(default = "default_faucet_timeout")]
    pub faucet_timeout: u64,
    /// URL of the monitoring REST API (stratum-apps monitoring server).
    /// Used by web-proxy to fetch per-miner stats.
    /// Example: "http://127.0.0.1:9109"
    #[serde(default)]
    pub monitoring_api_url: Option<String>,
}

fn default_faucet_port() -> u16 {
    8083
}

fn default_faucet_timeout() -> u64 {
    3
}

#[derive(Debug, Deserialize, Clone)]
pub struct Upstream {
    /// The address of the upstream server.
    pub address: String,
    /// The port of the upstream server.
    pub port: u16,
    /// The Secp256k1 public key used to authenticate the upstream authority.
    pub authority_pubkey: Secp256k1PublicKey,
}

impl Upstream {
    /// Creates a new `UpstreamConfig` instance.
    pub fn new(address: String, port: u16, authority_pubkey: Secp256k1PublicKey) -> Self {
        Self {
            address,
            port,
            authority_pubkey,
        }
    }
}

impl TranslatorConfig {
    /// Creates a new `TranslatorConfig` instance with the specified upstream and downstream
    /// configurations and version constraints.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        upstreams: Vec<Upstream>,
        downstream_address: String,
        downstream_port: u16,
        downstream_difficulty_config: DownstreamDifficultyConfig,
        max_supported_version: u16,
        min_supported_version: u16,
        downstream_extranonce2_size: u16,
        user_identity: String,
        aggregate_channels: bool,
        supported_extensions: Vec<u16>,
        required_extensions: Vec<u16>,
        monitoring_address: Option<SocketAddr>,
        monitoring_cache_refresh_secs: Option<u64>,
    ) -> Self {
        Self {
            upstreams,
            downstream_address,
            downstream_port,
            max_supported_version,
            min_supported_version,
            downstream_extranonce2_size,
            user_identity,
            downstream_difficulty_config,
            aggregate_channels,
            supported_extensions,
            required_extensions,
            log_file: None,
            monitoring_address,
            monitoring_cache_refresh_secs,
            wallet: WalletConfig::default(),
            mint: None,
            faucet_port: 8083,
            faucet_timeout: 3,
            monitoring_api_url: None,
        }
    }

    /// Returns the monitoring server bind address (if enabled)
    pub fn monitoring_address(&self) -> Option<SocketAddr> {
        self.monitoring_address
    }

    /// Returns the monitoring cache refresh interval in seconds.
    pub fn monitoring_cache_refresh_secs(&self) -> Option<u64> {
        self.monitoring_cache_refresh_secs
    }

    pub fn set_log_dir(&mut self, log_dir: Option<PathBuf>) {
        if let Some(dir) = log_dir {
            self.log_file = Some(dir);
        }
    }
    pub fn log_dir(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }
}

/// Configuration settings for managing difficulty adjustments on the downstream connection.
#[derive(Debug, Deserialize, Clone)]
pub struct DownstreamDifficultyConfig {
    /// The minimum hashrate expected from an individual miner on the downstream connection.
    pub min_individual_miner_hashrate: Hashrate,
    /// The target number of shares per minute for difficulty adjustment.
    pub shares_per_minute: SharesPerMinute,
    /// Whether to enable variable difficulty adjustment mechanism.
    /// If false, difficulty will be managed by upstream (useful with JDC).
    pub enable_vardiff: bool,
    /// Interval in seconds for sending keepalive jobs to downstream miners.
    /// The translator will send periodic mining.notify messages with updated time
    /// to prevent SV1 miners from timing out when the upstream doesn't send new jobs
    /// frequently enough (e.g., due to low Bitcoin mempool activity).
    /// Set to 0 to disable keepalive jobs.
    pub job_keepalive_interval_secs: u16,
}

impl DownstreamDifficultyConfig {
    /// Creates a new `DownstreamDifficultyConfig` instance.
    pub fn new(
        min_individual_miner_hashrate: Hashrate,
        shares_per_minute: SharesPerMinute,
        enable_vardiff: bool,
        job_keepalive_interval_secs: u16,
    ) -> Self {
        Self {
            min_individual_miner_hashrate,
            shares_per_minute,
            enable_vardiff,
            job_keepalive_interval_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn create_test_upstream() -> Upstream {
        // Use a valid base58-encoded public key from the key-utils test cases
        let pubkey_str = "9bDuixKmZqAJnrmP746n8zU1wyAQRrus7th9dxnkPg6RzQvCnan";
        let pubkey = Secp256k1PublicKey::from_str(pubkey_str).unwrap();
        Upstream::new("127.0.0.1".to_string(), 4444, pubkey)
    }

    fn create_test_difficulty_config() -> DownstreamDifficultyConfig {
        DownstreamDifficultyConfig::new(100.0, 5.0, true, 60)
    }

    #[test]
    fn test_upstream_creation() {
        let upstream = create_test_upstream();
        assert_eq!(upstream.address, "127.0.0.1");
        assert_eq!(upstream.port, 4444);
    }

    #[test]
    fn test_downstream_difficulty_config_creation() {
        let config = create_test_difficulty_config();
        assert_eq!(config.min_individual_miner_hashrate, 100.0);
        assert_eq!(config.shares_per_minute, 5.0);
        assert!(config.enable_vardiff);
    }

    #[test]
    fn test_translator_config_creation() {
        let upstreams = vec![create_test_upstream()];
        let difficulty_config = create_test_difficulty_config();

        let config = TranslatorConfig::new(
            upstreams,
            "0.0.0.0".to_string(),
            3333,
            difficulty_config,
            2,
            1,
            4,
            "test_user".to_string(),
            true,
            vec![],
            vec![],
            None,
            None,
        );

        assert_eq!(config.upstreams.len(), 1);
        assert_eq!(config.downstream_address, "0.0.0.0");
        assert_eq!(config.downstream_port, 3333);
        assert_eq!(config.max_supported_version, 2);
        assert_eq!(config.min_supported_version, 1);
        assert_eq!(config.downstream_extranonce2_size, 4);
        assert_eq!(config.user_identity, "test_user");
        assert!(config.aggregate_channels);
        assert!(config.supported_extensions.is_empty());
        assert!(config.required_extensions.is_empty());
        assert!(config.log_file.is_none());
    }

    #[test]
    fn test_translator_config_log_dir() {
        let upstreams = vec![create_test_upstream()];
        let difficulty_config = create_test_difficulty_config();

        let mut config = TranslatorConfig::new(
            upstreams,
            "0.0.0.0".to_string(),
            3333,
            difficulty_config,
            2,
            1,
            4,
            "test_user".to_string(),
            false,
            vec![],
            vec![],
            None,
            None,
        );

        assert!(config.log_dir().is_none());

        let log_path = PathBuf::from("/tmp/logs");
        config.set_log_dir(Some(log_path.clone()));
        assert_eq!(config.log_dir(), Some(log_path.as_path()));

        config.set_log_dir(None);
        assert_eq!(config.log_dir(), Some(log_path.as_path())); // Should remain unchanged
    }

    #[test]
    fn test_multiple_upstreams() {
        let upstream1 = create_test_upstream();
        let mut upstream2 = create_test_upstream();
        upstream2.address = "192.168.1.1".to_string();
        upstream2.port = 5555;

        let upstreams = vec![upstream1, upstream2];
        let difficulty_config = create_test_difficulty_config();

        let config = TranslatorConfig::new(
            upstreams,
            "0.0.0.0".to_string(),
            3333,
            difficulty_config,
            2,
            1,
            4,
            "test_user".to_string(),
            true,
            vec![],
            vec![],
            None,
            None,
        );

        assert_eq!(config.upstreams.len(), 2);
        assert_eq!(config.upstreams[0].address, "127.0.0.1");
        assert_eq!(config.upstreams[0].port, 4444);
        assert_eq!(config.upstreams[1].address, "192.168.1.1");
        assert_eq!(config.upstreams[1].port, 5555);
    }

    #[test]
    fn test_vardiff_disabled_config() {
        let mut difficulty_config = create_test_difficulty_config();
        difficulty_config.enable_vardiff = false;

        let upstreams = vec![create_test_upstream()];
        let config = TranslatorConfig::new(
            upstreams,
            "0.0.0.0".to_string(),
            3333,
            difficulty_config,
            2,
            1,
            4,
            "test_user".to_string(),
            false,
            vec![],
            vec![],
            None,
            None,
        );

        assert!(!config.downstream_difficulty_config.enable_vardiff);
        assert!(!config.aggregate_channels);
    }
}
