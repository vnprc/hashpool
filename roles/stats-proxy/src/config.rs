use std::env;
use std::path::PathBuf;
use std::fs;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub tcp_address: String,
    pub http_address: String,
    pub db_path: PathBuf,
    pub downstream_address: String,
    pub downstream_port: u16,
    pub redact_ip: bool,
    pub faucet_enabled: bool,
    pub faucet_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TproxyConfig {
    downstream_address: String,
    downstream_port: u16,
    #[serde(default)]
    redact_ip: bool,
}

#[derive(Debug, Deserialize)]
struct FaucetConfig {
    enabled: bool,
    port: u16,
}

impl Config {
    pub fn from_args() -> Result<Self, Box<dyn std::error::Error>> {
        let args: Vec<String> = env::args().collect();

        // Parse command line arguments - fail fast if not provided
        let tcp_address = args
            .iter()
            .position(|arg| arg == "--tcp-address" || arg == "-t")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --tcp-address")?;

        let http_address = args
            .iter()
            .position(|arg| arg == "--http-address" || arg == "-h")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --http-address")?;

        let db_path = args
            .iter()
            .position(|arg| arg == "--db-path" || arg == "-d")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --db-path")?;

        // Load tproxy config to get downstream connection info
        let config_path = args
            .iter()
            .position(|arg| arg == "--config" || arg == "-c")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config/tproxy.config.toml");

        let config_str = fs::read_to_string(config_path)?;
        let tproxy: TproxyConfig = toml::from_str(&config_str)?;

        // Load shared miner config to get faucet port
        let shared_config_path = args
            .iter()
            .position(|arg| arg == "--shared-config" || arg == "-s")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config/shared/miner.toml");

        let shared_config_str = fs::read_to_string(shared_config_path)?;
        let shared_config: toml::Value = toml::from_str(&shared_config_str)?;

        // Extract faucet configuration (optional, defaults to disabled)
        let faucet_enabled = shared_config
            .get("faucet")
            .and_then(|f| f.get("enabled"))
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        let faucet_url = if faucet_enabled {
            let faucet_host = shared_config
                .get("faucet")
                .and_then(|f| f.get("host"))
                .and_then(|h| h.as_str())
                .ok_or("Missing required config: faucet.host in shared config (required when faucet.enabled=true)")?;

            let faucet_port = shared_config
                .get("faucet")
                .and_then(|f| f.get("port"))
                .and_then(|p| p.as_integer())
                .ok_or("Missing required config: faucet.port in shared config (required when faucet.enabled=true)")? as u16;

            Some(format!("http://{}:{}", faucet_host, faucet_port))
        } else {
            None
        };

        Ok(Config {
            tcp_address,
            http_address,
            db_path: PathBuf::from(db_path),
            downstream_address: tproxy.downstream_address,
            downstream_port: tproxy.downstream_port,
            redact_ip: tproxy.redact_ip,
            faucet_enabled,
            faucet_url,
        })
    }
}
