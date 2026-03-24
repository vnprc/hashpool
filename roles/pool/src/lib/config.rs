//! ## Configuration Module
//!
//! Defines [`PoolConfig`], the configuration structure for the Pool, along with its supporting
//! types.
//!
//! This module handles:
//! - Initializing [`PoolConfig`]
//! - Managing [`TemplateProviderConfig`], [`AuthorityConfig`], [`CoinbaseOutput`], and
//!   [`ConnectionConfig`]
//! - Validating and converting coinbase outputs
use std::path::{Path, PathBuf};

use config_helpers_sv2::CoinbaseRewardScript;
use key_utils::{Secp256k1PublicKey, Secp256k1SecretKey};
use shared_config::Sv2MessagingConfig;

/// Configuration for the Pool, including connection, authority, and coinbase settings.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct PoolConfig {
    listen_address: String,
    tp_address: String,
    tp_authority_public_key: Option<Secp256k1PublicKey>,
    authority_public_key: Secp256k1PublicKey,
    authority_secret_key: Secp256k1SecretKey,
    cert_validity_sec: u64,
    coinbase_reward_script: CoinbaseRewardScript,
    pool_signature: String,
    shares_per_minute: f32,
    share_batch_size: usize,
    log_file: Option<PathBuf>,
    server_id: u16,
    #[serde(default)]
    locking_pubkey: Option<String>,
    #[serde(default)]
    monitoring_address: Option<String>,
    #[serde(default)]
    jd_server_address: Option<String>,
    /// Bitcoin network this pool is operating on.
    /// Exposed via `GET /api/v1/global` in the monitoring REST API.
    /// Values follow bitcoin-cli convention: `"main"`, `"test"`, `"testnet4"`, `"regtest"`,
    /// `"signet"`.
    #[serde(default)]
    network: Option<String>,
    #[serde(skip)]
    sv2_messaging: Option<Sv2MessagingConfig>,
    #[serde(skip)]
    minimum_difficulty: Option<u32>,
    #[serde(skip)]
    minimum_share_difficulty_bits: Option<u32>,
    #[serde(skip)]
    mint_http_url: Option<String>,
    #[serde(skip)]
    min_downstream_hashrate: Option<f32>,
}

impl PoolConfig {
    /// Creates a new instance of the [`PoolConfig`].
    ///
    /// # Panics
    ///
    /// Panics if `coinbase_reward_script` is empty.
    pub fn new(
        pool_connection: ConnectionConfig,
        template_provider: TemplateProviderConfig,
        authority_config: AuthorityConfig,
        coinbase_reward_script: CoinbaseRewardScript,
        shares_per_minute: f32,
        share_batch_size: usize,
        server_id: u16,
    ) -> Self {
        Self {
            listen_address: pool_connection.listen_address,
            tp_address: template_provider.address,
            tp_authority_public_key: template_provider.authority_public_key,
            authority_public_key: authority_config.public_key,
            authority_secret_key: authority_config.secret_key,
            cert_validity_sec: pool_connection.cert_validity_sec,
            coinbase_reward_script,
            pool_signature: pool_connection.signature,
            shares_per_minute,
            share_batch_size,
            log_file: None,
            server_id,
            locking_pubkey: None,
            monitoring_address: None,
            jd_server_address: None,
            network: None,
            sv2_messaging: None,
            minimum_difficulty: None,
            minimum_share_difficulty_bits: None,
            mint_http_url: None,
            min_downstream_hashrate: None,
        }
    }

    /// Returns the coinbase output.
    pub fn coinbase_reward_script(&self) -> &CoinbaseRewardScript {
        &self.coinbase_reward_script
    }

    /// Returns Pool listenining address.
    pub fn listen_address(&self) -> &String {
        &self.listen_address
    }

    /// Returns the authority public key.
    pub fn authority_public_key(&self) -> &Secp256k1PublicKey {
        &self.authority_public_key
    }

    /// Returns the authority secret key.
    pub fn authority_secret_key(&self) -> &Secp256k1SecretKey {
        &self.authority_secret_key
    }

    /// Returns the certificate validity in seconds.
    pub fn cert_validity_sec(&self) -> u64 {
        self.cert_validity_sec
    }

    /// Returns the Pool signature.
    pub fn pool_signature(&self) -> &String {
        &self.pool_signature
    }

    /// Return the Template Provider authority public key.
    pub fn tp_authority_public_key(&self) -> Option<&Secp256k1PublicKey> {
        self.tp_authority_public_key.as_ref()
    }

    /// Returns the Template Provider address.
    pub fn tp_address(&self) -> &String {
        &self.tp_address
    }

    /// Returns the share batch size.
    pub fn share_batch_size(&self) -> usize {
        self.share_batch_size
    }

    /// Sets the coinbase output.
    pub fn set_coinbase_reward_script(&mut self, coinbase_output: CoinbaseRewardScript) {
        self.coinbase_reward_script = coinbase_output;
    }

    /// Returns the shares per minute.
    pub fn shares_per_minute(&self) -> f32 {
        self.shares_per_minute
    }

    /// Change TP address.
    pub fn set_tp_address(&mut self, tp_address: String) {
        self.tp_address = tp_address;
    }

    /// Sets the log directory.
    pub fn set_log_dir(&mut self, log_dir: Option<PathBuf>) {
        if let Some(dir) = log_dir {
            self.log_file = Some(dir);
        }
    }
    /// Returns the log directory.
    pub fn log_dir(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }

    /// Returns the server id.
    pub fn server_id(&self) -> u16 {
        self.server_id
    }

    /// Returns the locking pubkey (compressed public key as hex string for quote attribution).
    pub fn locking_pubkey(&self) -> Option<&str> {
        self.locking_pubkey.as_deref()
    }

    /// Sets the locking pubkey (from global config or other sources).
    pub fn set_locking_pubkey(&mut self, pubkey: String) {
        self.locking_pubkey = Some(pubkey);
    }

    /// Returns an optional SV2 messaging configuration loaded from shared config.
    pub fn sv2_messaging(&self) -> Option<&Sv2MessagingConfig> {
        self.sv2_messaging.as_ref()
    }

    /// Sets the SV2 messaging configuration (from shared config).
    pub fn set_sv2_messaging(&mut self, messaging: Option<Sv2MessagingConfig>) {
        self.sv2_messaging = messaging;
    }

    /// Returns the optional minimum ehash difficulty override.
    pub fn minimum_difficulty(&self) -> Option<u32> {
        self.minimum_difficulty
    }

    /// Sets the minimum difficulty override (from shared config).
    pub fn set_minimum_difficulty(&mut self, minimum_difficulty: Option<u32>) {
        self.minimum_difficulty = minimum_difficulty;
    }

    /// Returns the optional minimum share difficulty bits.
    pub fn minimum_share_difficulty_bits(&self) -> Option<u32> {
        self.minimum_share_difficulty_bits
    }

    /// Sets the minimum share difficulty bits (from shared config).
    pub fn set_minimum_share_difficulty_bits(&mut self, bits: Option<u32>) {
        self.minimum_share_difficulty_bits = bits;
    }

    /// Returns the optional mint HTTP endpoint used by the quote poller.
    pub fn mint_http_url(&self) -> Option<&str> {
        self.mint_http_url.as_deref()
    }

    /// Sets the mint HTTP endpoint used by the quote poller.
    pub fn set_mint_http_url(&mut self, mint_http_url: Option<String>) {
        self.mint_http_url = mint_http_url;
    }

    /// Returns the optional minimum downstream hashrate (in H/s) for channel creation policy.
    pub fn min_downstream_hashrate(&self) -> Option<f32> {
        self.min_downstream_hashrate
    }

    /// Sets the minimum downstream hashrate (from shared config).
    pub fn set_min_downstream_hashrate(&mut self, hashrate: Option<f32>) {
        self.min_downstream_hashrate = hashrate;
    }

    /// Returns the optional monitoring HTTP address for Prometheus scraping.
    pub fn monitoring_address(&self) -> Option<&str> {
        self.monitoring_address.as_deref()
    }

    /// Returns the optional JD-Server address (Job Declarator Server).
    pub fn jd_server_address(&self) -> Option<&str> {
        self.jd_server_address.as_deref()
    }

    /// Returns the Bitcoin network name (e.g. `"regtest"`, `"main"`).
    pub fn network(&self) -> Option<&str> {
        self.network.as_deref()
    }
}

/// Configuration for connecting to a Template Provider.
pub struct TemplateProviderConfig {
    address: String,
    authority_public_key: Option<Secp256k1PublicKey>,
}

impl TemplateProviderConfig {
    pub fn new(address: String, authority_public_key: Option<Secp256k1PublicKey>) -> Self {
        Self {
            address,
            authority_public_key,
        }
    }
}

/// Pool's authority public and secret keys.
pub struct AuthorityConfig {
    pub public_key: Secp256k1PublicKey,
    pub secret_key: Secp256k1SecretKey,
}

impl AuthorityConfig {
    pub fn new(public_key: Secp256k1PublicKey, secret_key: Secp256k1SecretKey) -> Self {
        Self {
            public_key,
            secret_key,
        }
    }
}

/// Connection settings for the Pool listener.
pub struct ConnectionConfig {
    listen_address: String,
    cert_validity_sec: u64,
    signature: String,
}

impl ConnectionConfig {
    pub fn new(listen_address: String, cert_validity_sec: u64, signature: String) -> Self {
        Self {
            listen_address,
            cert_validity_sec,
            signature,
        }
    }
}
