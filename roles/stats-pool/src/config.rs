use std::env;
use std::fs;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub tcp_address: String,
    pub http_address: String,
    pub staleness_threshold_secs: u64,
    pub request_timeout_secs: u64,
    pub pool_idle_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
struct StatsPoolConfig {
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
            tcp_listen_address: Some("127.0.0.1:9083".to_string()),
            http_listen_address: Some("127.0.0.1:9084".to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SnapshotStorageConfig {
    staleness_threshold_secs: Option<u64>,
}

impl Default for SnapshotStorageConfig {
    fn default() -> Self {
        Self {
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

impl Config {
    pub fn from_args() -> Result<Self, Box<dyn std::error::Error>> {
        let args: Vec<String> = env::args().collect();

        // Load stats-pool config file (can be overridden via CLI)
        let stats_pool_config_path = args
            .iter()
            .position(|arg| arg == "--config" || arg == "-c")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config/stats-pool.config.toml");

        let stats_pool_config_str = fs::read_to_string(stats_pool_config_path)
            .unwrap_or_default();
        let stats_pool_config: StatsPoolConfig = if stats_pool_config_str.is_empty() {
            StatsPoolConfig {
                server: ServerConfig::default(),
                snapshot_storage: SnapshotStorageConfig::default(),
                http_client: HttpClientConfig::default(),
            }
        } else {
            toml::from_str(&stats_pool_config_str)?
        };

        // TCP and HTTP addresses from config file, with CLI overrides
        let tcp_address = args
            .iter()
            .position(|arg| arg == "--tcp-address" || arg == "-t")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| stats_pool_config.server.tcp_listen_address)
            .ok_or("Missing required config: server.tcp_listen_address")?;

        let http_address = args
            .iter()
            .position(|arg| arg == "--http-address" || arg == "-h")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| stats_pool_config.server.http_listen_address)
            .ok_or("Missing required config: server.http_listen_address")?;

        Ok(Config {
            tcp_address,
            http_address,
            staleness_threshold_secs: stats_pool_config.snapshot_storage.staleness_threshold_secs.unwrap_or(15),
            request_timeout_secs: stats_pool_config.http_client.request_timeout_secs.unwrap_or(60),
            pool_idle_timeout_secs: stats_pool_config.http_client.pool_idle_timeout_secs.unwrap_or(300),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_server_config() {
        let config = ServerConfig::default();
        assert_eq!(config.tcp_listen_address, Some("127.0.0.1:9083".to_string()));
        assert_eq!(config.http_listen_address, Some("127.0.0.1:9084".to_string()));
    }

    #[test]
    fn test_default_snapshot_storage_config() {
        let config = SnapshotStorageConfig::default();
        assert_eq!(config.staleness_threshold_secs, Some(15));
    }

    #[test]
    fn test_default_http_client_config() {
        let config = HttpClientConfig::default();
        assert_eq!(config.pool_idle_timeout_secs, Some(300));
        assert_eq!(config.request_timeout_secs, Some(60));
    }

    #[test]
    fn test_server_config_deserialization() {
        let toml_str = r#"
            tcp_listen_address = "0.0.0.0:9090"
            http_listen_address = "0.0.0.0:9091"
        "#;
        let config: ServerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tcp_listen_address, Some("0.0.0.0:9090".to_string()));
        assert_eq!(config.http_listen_address, Some("0.0.0.0:9091".to_string()));
    }

    #[test]
    fn test_snapshot_storage_config_deserialization() {
        let toml_str = r#"
            staleness_threshold_secs = 30
        "#;
        let config: SnapshotStorageConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.staleness_threshold_secs, Some(30));
    }

    #[test]
    fn test_http_client_config_deserialization() {
        let toml_str = r#"
            pool_idle_timeout_secs = 600
            request_timeout_secs = 120
        "#;
        let config: HttpClientConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.pool_idle_timeout_secs, Some(600));
        assert_eq!(config.request_timeout_secs, Some(120));
    }

    #[test]
    fn test_full_config_deserialization() {
        let toml_str = r#"
            [server]
            tcp_listen_address = "127.0.0.1:5555"
            http_listen_address = "127.0.0.1:6666"

            [snapshot_storage]
            staleness_threshold_secs = 20

            [http_client]
            pool_idle_timeout_secs = 400
            request_timeout_secs = 80
        "#;
        let config: StatsPoolConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.tcp_listen_address, Some("127.0.0.1:5555".to_string()));
        assert_eq!(config.server.http_listen_address, Some("127.0.0.1:6666".to_string()));
        assert_eq!(config.snapshot_storage.staleness_threshold_secs, Some(20));
        assert_eq!(config.http_client.pool_idle_timeout_secs, Some(400));
        assert_eq!(config.http_client.request_timeout_secs, Some(80));
    }

    #[test]
    fn test_partial_config_uses_defaults() {
        let toml_str = r#"
            [server]
            tcp_listen_address = "127.0.0.1:7777"
        "#;
        let config: StatsPoolConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.tcp_listen_address, Some("127.0.0.1:7777".to_string()));
        // Other fields should use their defaults when missing
        assert_eq!(config.http_client.pool_idle_timeout_secs, None);
    }

    #[test]
    fn test_empty_config_has_all_defaults() {
        let toml_str = "";
        let config_str = if toml_str.is_empty() {
            StatsPoolConfig {
                server: ServerConfig::default(),
                snapshot_storage: SnapshotStorageConfig::default(),
                http_client: HttpClientConfig::default(),
            }
        } else {
            toml::from_str(toml_str).unwrap()
        };
        assert_eq!(config_str.server.tcp_listen_address, Some("127.0.0.1:9083".to_string()));
        assert_eq!(config_str.server.http_listen_address, Some("127.0.0.1:9084".to_string()));
        assert_eq!(config_str.snapshot_storage.staleness_threshold_secs, Some(15));
    }

    #[test]
    fn test_config_timeout_fallbacks() {
        let config = Config {
            tcp_address: "127.0.0.1:9083".to_string(),
            http_address: "127.0.0.1:9084".to_string(),
            staleness_threshold_secs: 15,
            request_timeout_secs: 60,
            pool_idle_timeout_secs: 300,
        };

        assert_eq!(config.staleness_threshold_secs, 15);
        assert_eq!(config.request_timeout_secs, 60);
        assert_eq!(config.pool_idle_timeout_secs, 300);
    }
}
