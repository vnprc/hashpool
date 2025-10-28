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

use key_utils::Secp256k1PublicKey;
use serde::Deserialize;
use shared_config::{MintConfig, WalletConfig};

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
    /// Wallet configuration for managing ehash tokens
    pub wallet: WalletConfig,
    /// Mint service configuration for quote operations
    pub mint: Option<MintConfig>,
    /// The path to the log file for the Translator.
    log_file: Option<PathBuf>,
    /// Optional address of the stats service for sending snapshots
    #[serde(default)]
    pub stats_server_address: Option<String>,
    /// Snapshot poll interval in seconds
    #[serde(default = "default_snapshot_poll_interval_secs")]
    pub snapshot_poll_interval_secs: u64,
    /// Whether to redact IP addresses in stats
    #[serde(default = "default_redact_ip")]
    pub redact_ip: bool,
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

/// Default snapshot poll interval (5 seconds)
fn default_snapshot_poll_interval_secs() -> u64 {
    5
}

/// Default IP redaction setting (true for privacy)
fn default_redact_ip() -> bool {
    true
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
        wallet: WalletConfig,
        mint: Option<MintConfig>,
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
            wallet,
            mint,
            log_file: None,
            stats_server_address: None,
            snapshot_poll_interval_secs: 5,
            redact_ip: true,
        }
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
    /// Minimum expected hashrate for an individual downstream miner. Always computed from the
    /// shared minimum difficulty to guarantee submitted shares earn HASH.
    #[serde(skip)]
    pub min_individual_miner_hashrate: f32,
    /// The target number of shares per minute for difficulty adjustment.
    pub shares_per_minute: f32,
    /// Whether to enable variable difficulty adjustment mechanism.
    /// If false, difficulty will be managed by upstream (useful with JDC).
    pub enable_vardiff: bool,
    /// Cached minimum difficulty (in "bits") used to derive `min_individual_miner_hashrate`.
    #[serde(skip)]
    minimum_difficulty_bits: u32,
}

impl DownstreamDifficultyConfig {
    /// Creates a new `DownstreamDifficultyConfig` instance.
    pub fn new(
        min_individual_miner_hashrate: f32,
        shares_per_minute: f32,
        enable_vardiff: bool,
    ) -> Self {
        Self {
            min_individual_miner_hashrate,
            shares_per_minute,
            enable_vardiff,
            minimum_difficulty_bits: 0,
        }
    }

    /// Update `min_individual_miner_hashrate` so that proxy difficulty matches the mint's
    /// minimum ehash requirement.
    pub fn set_min_hashrate_from_difficulty(&mut self, minimum_difficulty: u32) {
        // Avoid division by zero if misconfigured.
        let shares_per_minute = if self.shares_per_minute <= f32::EPSILON {
            1.0
        } else {
            self.shares_per_minute
        } as f64;
        let time_between_shares = 60.0f64 / shares_per_minute;
        // Derive the hashrate required to achieve the requested minimum difficulty while still
        // meeting the configured share cadence.
        let target_hashes = 2_f64.powi(minimum_difficulty as i32);
        let hashrate = target_hashes / time_between_shares;

        self.min_individual_miner_hashrate = hashrate as f32;
        self.minimum_difficulty_bits = minimum_difficulty;
    }

    /// Returns the cached minimum difficulty used for the current configuration.
    pub fn minimum_difficulty_bits(&self) -> u32 {
        self.minimum_difficulty_bits
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
        let mut config = DownstreamDifficultyConfig::new(100.0, 5.0, true);
        config.set_min_hashrate_from_difficulty(32);
        config
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
        assert!(config.min_individual_miner_hashrate > 0.0);
        assert_eq!(config.shares_per_minute, 5.0);
        assert!(config.enable_vardiff);
        assert_eq!(config.minimum_difficulty_bits(), 32);
    }

    #[test]
    fn test_translator_config_creation() {
        use shared_config::WalletConfig;

        let upstreams = vec![create_test_upstream()];
        let difficulty_config = create_test_difficulty_config();
        let wallet = WalletConfig {
            mnemonic: "test mnemonic".to_string(),
            db_path: "/tmp/wallet.db".to_string(),
            locking_pubkey: None,
            locking_privkey: None,
        };

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
            wallet,
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
        assert!(config.log_file.is_none());
    }

    #[test]
    fn test_translator_config_log_dir() {
        use shared_config::WalletConfig;

        let upstreams = vec![create_test_upstream()];
        let difficulty_config = create_test_difficulty_config();
        let wallet = WalletConfig {
            mnemonic: "test mnemonic".to_string(),
            db_path: "/tmp/wallet.db".to_string(),
            locking_pubkey: None,
            locking_privkey: None,
        };

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
            wallet,
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
        use shared_config::WalletConfig;

        let upstream1 = create_test_upstream();
        let mut upstream2 = create_test_upstream();
        upstream2.address = "192.168.1.1".to_string();
        upstream2.port = 5555;

        let upstreams = vec![upstream1, upstream2];
        let difficulty_config = create_test_difficulty_config();
        let wallet = WalletConfig {
            mnemonic: "test mnemonic".to_string(),
            db_path: "/tmp/wallet.db".to_string(),
            locking_pubkey: None,
            locking_privkey: None,
        };

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
            wallet,
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
        use shared_config::WalletConfig;

        let mut difficulty_config = create_test_difficulty_config();
        difficulty_config.enable_vardiff = false;

        let upstreams = vec![create_test_upstream()];
        let wallet = WalletConfig {
            mnemonic: "test mnemonic".to_string(),
            db_path: "/tmp/wallet.db".to_string(),
            locking_pubkey: None,
            locking_privkey: None,
        };

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
            wallet,
            None,
        );

        assert!(!config.downstream_difficulty_config.enable_vardiff);
        assert!(!config.aggregate_channels);
    }
}
