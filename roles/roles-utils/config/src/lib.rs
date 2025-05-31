use serde::Deserialize;
use config::{Config, ConfigError, File, FileFormat};

#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    pub url: String,
    pub active_keyset_prefix: String,
    pub create_quote_prefix: String,
    pub quote_id_prefix: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MintConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GlobalConfig {
    pub redis: RedisConfig,
    pub mint: MintConfig,
}

impl GlobalConfig {
    pub fn from_path(path: &str) -> Result<Self, ConfigError> {
        Config::builder()
            .add_source(File::new(path, FileFormat::Toml))
            .build()?
            .try_deserialize()
    }
}
