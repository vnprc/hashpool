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
}

#[derive(Debug, Deserialize)]
struct TproxyConfig {
    downstream_address: String,
    downstream_port: u16,
    #[serde(default)]
    redact_ip: bool,
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

        Ok(Config {
            tcp_address,
            http_address,
            db_path: PathBuf::from(db_path),
            downstream_address: tproxy.downstream_address,
            downstream_port: tproxy.downstream_port,
            redact_ip: tproxy.redact_ip,
        })
    }
}
