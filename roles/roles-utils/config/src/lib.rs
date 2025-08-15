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
    pub locking_pubkey: Option<String>,
    pub locking_privkey: Option<String>,
}

impl WalletConfig {
    /// Initialize and validate the wallet config, deriving pubkey from privkey if needed
    pub fn initialize(&mut self) -> Result<(), String> {
        match (&self.locking_pubkey, &self.locking_privkey) {
            (None, None) => Err("Either locking_pubkey or locking_privkey must be provided".to_string()),
            (pubkey_opt, Some(privkey)) => {
                // Derive pubkey from privkey
                use bitcoin::secp256k1::{Secp256k1, SecretKey};
                
                let privkey_bytes = hex::decode(privkey)
                    .map_err(|_| "Invalid private key hex format")?;
                
                if privkey_bytes.len() != 32 {
                    return Err("Private key must be 32 bytes".to_string());
                }
                
                let secp = Secp256k1::new();
                let secret_key = SecretKey::from_slice(&privkey_bytes)
                    .map_err(|_| "Invalid private key")?;
                let public_key = secret_key.public_key(&secp);
                let derived_pubkey = hex::encode(public_key.serialize());
                
                if let Some(provided_pubkey) = pubkey_opt {
                    // Both provided - check they match
                    if provided_pubkey != &derived_pubkey {
                        return Err("Provided locking_pubkey does not match derived pubkey from locking_privkey".to_string());
                    }
                } else {
                    // Only privkey provided - set the derived pubkey
                    self.locking_pubkey = Some(derived_pubkey);
                }
                Ok(())
            },
            (Some(pubkey), None) => {
                // Only pubkey provided - validate it
                use bitcoin::secp256k1::{Secp256k1, PublicKey};
                
                let pubkey_bytes = hex::decode(pubkey)
                    .map_err(|_| "Invalid public key hex format")?;
                
                let _secp = Secp256k1::new();
                PublicKey::from_slice(&pubkey_bytes)
                    .map_err(|_| "Invalid public key format")?;
                
                Ok(())
            },
        }
    }
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