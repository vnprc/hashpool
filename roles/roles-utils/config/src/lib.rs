use serde::Deserialize;
use config::{Config, ConfigError, File, FileFormat};

#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    pub url: String,
    pub active_keyset_prefix: String,
    pub create_quote_prefix: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MintConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PoolConfig {
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProxyConfig {
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WalletConfig {
    pub mnemonic: String,
    pub db_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MinerGlobalConfig {
    pub mint: MintConfig,
    pub pool: PoolConfig,
    pub proxy: ProxyConfig,
}

impl MinerGlobalConfig {
    pub fn from_path(path: &str) -> Result<Self, ConfigError> {
        Config::builder()
            .add_source(File::new(path, FileFormat::Toml))
            .build()?
            .try_deserialize()
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PoolGlobalConfig {
    pub redis: RedisConfig,
    pub mint: MintConfig,
    pub pool: PoolConfig,
    pub proxy: ProxyConfig,
}

impl PoolGlobalConfig {
    pub fn from_path(path: &str) -> Result<Self, ConfigError> {
        Config::builder()
            .add_source(File::new(path, FileFormat::Toml))
            .build()?
            .try_deserialize()
    }
}