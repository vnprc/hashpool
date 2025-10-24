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
    pub staleness_threshold_secs: u64,
    pub request_timeout_secs: u64,
    pub pool_idle_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
struct StatsProxyConfig {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    snapshot_storage: SnapshotStorageConfig,
    #[serde(default)]
    http_client: HttpClientConfig,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    tcp_listen_address: Option<String>,
    http_listen_address: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tcp_listen_address: Some("127.0.0.1:8082".to_string()),
            http_listen_address: Some("127.0.0.1:8084".to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SnapshotStorageConfig {
    db_path: Option<PathBuf>,
    staleness_threshold_secs: Option<u64>,
}

impl Default for SnapshotStorageConfig {
    fn default() -> Self {
        Self {
            db_path: Some(PathBuf::from(".devenv/state/stats-proxy.db")),
            staleness_threshold_secs: Some(15),
        }
    }
}

#[derive(Debug, Deserialize)]
struct HttpClientConfig {
    pool_idle_timeout_secs: Option<u64>,
    request_timeout_secs: Option<u64>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            pool_idle_timeout_secs: Some(300),
            request_timeout_secs: Some(60),
        }
    }
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

        // Load stats-proxy config file (can be overridden via CLI)
        let stats_proxy_config_path = args
            .iter()
            .position(|arg| arg == "--config" || arg == "-c")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config/stats-proxy.config.toml");

        let stats_proxy_config_str = fs::read_to_string(stats_proxy_config_path)
            .unwrap_or_default();
        let stats_proxy_config: StatsProxyConfig = if stats_proxy_config_str.is_empty() {
            StatsProxyConfig {
                server: ServerConfig::default(),
                snapshot_storage: SnapshotStorageConfig::default(),
                http_client: HttpClientConfig::default(),
            }
        } else {
            toml::from_str(&stats_proxy_config_str)?
        };

        // TCP and HTTP addresses from config file, with CLI overrides
        let tcp_address = args
            .iter()
            .position(|arg| arg == "--tcp-address" || arg == "-t")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| stats_proxy_config.server.tcp_listen_address)
            .ok_or("Missing required config: server.tcp_listen_address")?;

        let http_address = args
            .iter()
            .position(|arg| arg == "--http-address" || arg == "-h")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| stats_proxy_config.server.http_listen_address)
            .ok_or("Missing required config: server.http_listen_address")?;

        let db_path = args
            .iter()
            .position(|arg| arg == "--db-path" || arg == "-d")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .map(PathBuf::from)
            .or_else(|| stats_proxy_config.snapshot_storage.db_path)
            .ok_or("Missing required config: snapshot_storage.db_path")?;

        // Load tproxy config to get downstream connection info
        let tproxy_config_path = args
            .iter()
            .position(|arg| arg == "--tproxy-config")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config/tproxy.config.toml");

        let tproxy_str = fs::read_to_string(tproxy_config_path)?;
        let tproxy: TproxyConfig = toml::from_str(&tproxy_str)?;

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
            db_path,
            downstream_address: tproxy.downstream_address,
            downstream_port: tproxy.downstream_port,
            redact_ip: tproxy.redact_ip,
            faucet_enabled,
            faucet_url,
            staleness_threshold_secs: stats_proxy_config.snapshot_storage.staleness_threshold_secs.unwrap_or(15),
            request_timeout_secs: stats_proxy_config.http_client.request_timeout_secs.unwrap_or(60),
            pool_idle_timeout_secs: stats_proxy_config.http_client.pool_idle_timeout_secs.unwrap_or(300),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_stats_proxy_config_deserialization() {
        let toml_str = r#"
            [server]
            tcp_listen_address = "127.0.0.1:4444"
            http_listen_address = "127.0.0.1:4445"

            [snapshot_storage]
            db_path = "/tmp/stats.db"
            staleness_threshold_secs = 20

            [http_client]
            pool_idle_timeout_secs = 400
            request_timeout_secs = 75
        "#;
        let config: StatsProxyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.tcp_listen_address, Some("127.0.0.1:4444".to_string()));
        assert_eq!(config.server.http_listen_address, Some("127.0.0.1:4445".to_string()));
        assert_eq!(config.snapshot_storage.db_path, Some(PathBuf::from("/tmp/stats.db")));
        assert_eq!(config.snapshot_storage.staleness_threshold_secs, Some(20));
        assert_eq!(config.http_client.pool_idle_timeout_secs, Some(400));
        assert_eq!(config.http_client.request_timeout_secs, Some(75));
    }

    #[test]
    fn test_tproxy_config_deserialization() {
        let toml_str = r#"
            downstream_address = "127.0.0.1"
            downstream_port = 3333
            redact_ip = true
        "#;
        let config: TproxyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.downstream_address, "127.0.0.1");
        assert_eq!(config.downstream_port, 3333);
        assert_eq!(config.redact_ip, true);
    }
}
