use std::env;
use std::fs;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub stats_proxy_url: String,
    pub web_server_address: String,
    pub downstream_address: String,
    pub downstream_port: u16,
    pub faucet_enabled: bool,
    pub faucet_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TproxyConfig {
    downstream_address: String,
    downstream_port: u16,
}

impl Config {
    pub fn from_args() -> Result<Self, Box<dyn std::error::Error>> {
        let args: Vec<String> = env::args().collect();

        // Parse command line arguments
        let stats_proxy_url = args
            .iter()
            .position(|arg| arg == "--stats-proxy-url" || arg == "-s")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --stats-proxy-url")?;

        let web_server_address = args
            .iter()
            .position(|arg| arg == "--web-address" || arg == "-w")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --web-address")?;

        // Load tproxy config to get downstream connection info
        let config_path = args
            .iter()
            .position(|arg| arg == "--config" || arg == "-c")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config/tproxy.config.toml");

        let config_str = fs::read_to_string(config_path)?;
        let tproxy: TproxyConfig = toml::from_str(&config_str)?;

        // Load shared miner config to get faucet configuration
        let shared_config_path = args
            .iter()
            .position(|arg| arg == "--shared-config" || arg == "-g")
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
            stats_proxy_url,
            web_server_address,
            downstream_address: tproxy.downstream_address,
            downstream_port: tproxy.downstream_port,
            faucet_enabled,
            faucet_url,
        })
    }
}
