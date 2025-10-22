use key_utils::Secp256k1PublicKey;
use serde::Deserialize;
use shared_config::{MintConfig, WalletConfig};

#[derive(Debug, Deserialize, Clone)]
pub struct ProxyConfig {
    pub upstream_address: String,
    pub upstream_port: u16,
    pub upstream_authority_pubkey: Secp256k1PublicKey,
    pub downstream_address: String,
    pub downstream_port: u16,
    pub max_supported_version: u16,
    pub min_supported_version: u16,
    pub min_extranonce2_size: u16,
    pub downstream_difficulty_config: DownstreamDifficultyConfig,
    pub upstream_difficulty_config: UpstreamDifficultyConfig,
    pub mint: Option<MintConfig>,
    pub wallet: WalletConfig,
    #[serde(default = "default_web_port")]
    pub web_port: u16,
    pub stats_server_address: Option<String>,
    #[serde(default = "default_redact_ip")]
    pub redact_ip: bool,
    #[serde(default = "default_snapshot_poll_interval_secs")]
    pub snapshot_poll_interval_secs: u64,
}

fn default_redact_ip() -> bool {
    true
}

fn default_web_port() -> u16 {
    3030
}

fn default_snapshot_poll_interval_secs() -> u64 {
    5
}

pub struct UpstreamConfig {
    address: String,
    port: u16,
    authority_pubkey: Secp256k1PublicKey,
    difficulty_config: UpstreamDifficultyConfig,
}

impl UpstreamConfig {
    pub fn new(
        address: String,
        port: u16,
        authority_pubkey: Secp256k1PublicKey,
        difficulty_config: UpstreamDifficultyConfig,
    ) -> Self {
        Self {
            address,
            port,
            authority_pubkey,
            difficulty_config,
        }
    }
}

pub struct DownstreamConfig {
    address: String,
    port: u16,
    difficulty_config: DownstreamDifficultyConfig,
}

impl DownstreamConfig {
    pub fn new(address: String, port: u16, difficulty_config: DownstreamDifficultyConfig) -> Self {
        Self {
            address,
            port,
            difficulty_config,
        }
    }
}

impl ProxyConfig {
    pub fn new(
        upstream: UpstreamConfig,
        downstream: DownstreamConfig,
        wallet: WalletConfig,
        max_supported_version: u16,
        min_supported_version: u16,
        min_extranonce2_size: u16,
    ) -> Self {
        Self {
            upstream_address: upstream.address,
            upstream_port: upstream.port,
            upstream_authority_pubkey: upstream.authority_pubkey,
            downstream_address: downstream.address,
            downstream_port: downstream.port,
            max_supported_version,
            min_supported_version,
            min_extranonce2_size,
            downstream_difficulty_config: downstream.difficulty_config,
            upstream_difficulty_config: upstream.difficulty_config,
            mint: None,
            wallet,
            web_port: default_web_port(),
            stats_server_address: None,
            snapshot_poll_interval_secs: default_snapshot_poll_interval_secs(),
            redact_ip: default_redact_ip(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DownstreamDifficultyConfig {
    /// Minimum hashrate for individual miners - automatically calculated from minimum_difficulty
    ///
    /// This field is NOT read from config due to `#[serde(skip)]`. It is always calculated
    /// from `minimum_difficulty` in the shared config to ensure proxy difficulty matches
    /// ehash requirements. If this field appears in a config file, it will be ignored.
    /// See `set_min_hashrate_from_difficulty()` for the calculation.
    #[serde(skip)]
    pub min_individual_miner_hashrate: f32,
    pub shares_per_minute: f32,
    #[serde(default = "u32::default")]
    pub submits_since_last_update: u32,
    #[serde(default = "u64::default")]
    pub timestamp_of_last_update: u64,
}

impl DownstreamDifficultyConfig {
    pub fn new(
        min_individual_miner_hashrate: f32,
        shares_per_minute: f32,
        submits_since_last_update: u32,
        timestamp_of_last_update: u64,
    ) -> Self {
        Self {
            min_individual_miner_hashrate,
            shares_per_minute,
            submits_since_last_update,
            timestamp_of_last_update,
        }
    }

    /// Calculate and set min_individual_miner_hashrate from minimum_difficulty
    ///
    /// The minimum hashrate is derived from the ehash minimum difficulty requirement
    /// using the formula: hashrate = 2^minimum_difficulty / (60 / shares_per_minute)
    ///
    /// This ensures that shares meeting the proxy's difficulty target will always
    /// earn at least 1 ehash unit (no more 0 ehash shares).
    pub fn set_min_hashrate_from_difficulty(&mut self, minimum_difficulty: u32) {
        // Calculate hashrate needed to produce shares at minimum_difficulty
        // Formula: hashrate = 2^difficulty / time_between_shares
        let time_between_shares = 60.0 / self.shares_per_minute as f64;
        let target_hashes = 2_f64.powi(minimum_difficulty as i32);
        let hashrate = target_hashes / time_between_shares;

        self.min_individual_miner_hashrate = hashrate as f32;
    }
}
impl PartialEq for DownstreamDifficultyConfig {
    fn eq(&self, other: &Self) -> bool {
        other.min_individual_miner_hashrate.round() as u32
            == self.min_individual_miner_hashrate.round() as u32
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpstreamDifficultyConfig {
    pub channel_diff_update_interval: u32,
    pub channel_nominal_hashrate: f32,
    #[serde(default = "u64::default")]
    pub timestamp_of_last_update: u64,
    #[serde(default = "bool::default")]
    pub should_aggregate: bool,
}

impl UpstreamDifficultyConfig {
    pub fn new(
        channel_diff_update_interval: u32,
        channel_nominal_hashrate: f32,
        timestamp_of_last_update: u64,
        should_aggregate: bool,
    ) -> Self {
        Self {
            channel_diff_update_interval,
            channel_nominal_hashrate,
            timestamp_of_last_update,
            should_aggregate,
        }
    }
}
