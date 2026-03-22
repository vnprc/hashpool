use serde::Deserialize;
use std::{env, fs};

#[derive(Debug, Clone)]
pub struct Config {
    pub metrics_store_url: String,
    pub monitoring_api_url: String,
    pub web_server_address: String,
    pub downstream_address: String,
    pub downstream_port: u16,
    pub upstream_address: String,
    pub upstream_port: u16,
    pub faucet_enabled: bool,
    pub faucet_url: Option<String>,
    pub metrics_query_step_secs: u64,
    pub client_poll_interval_secs: u64,
    pub request_timeout_secs: u64,
    pub pool_idle_timeout_secs: u64,
    pub log_file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TproxyConfig {
    downstream_address: String,
    downstream_port: u16,
    upstream_address: String,
    upstream_port: u16,
}

#[derive(Debug, Deserialize)]
struct WebProxyConfig {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    metrics_store: MetricsStoreConfig,
    #[serde(default)]
    monitoring_api: MonitoringApiConfig,
    #[serde(default)]
    stats_proxy: StatsProxyConfig,
    #[serde(default)]
    http_client: HttpClientConfig,
}

#[derive(Debug, Deserialize)]
struct MonitoringApiConfig {
    url: Option<String>,
}

impl Default for MonitoringApiConfig {
    fn default() -> Self {
        Self {
            url: Some("http://127.0.0.1:9109".to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    listen_address: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_address: Some("127.0.0.1:3030".to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct MetricsStoreConfig {
    url: Option<String>,
}

impl Default for MetricsStoreConfig {
    fn default() -> Self {
        Self {
            url: Some("http://127.0.0.1:9090".to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct StatsProxyConfig {
    url: Option<String>,
}

impl Default for StatsProxyConfig {
    fn default() -> Self {
        Self { url: None }
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

        // Extract log file if provided (for tracing setup in main)
        let log_file = args
            .iter()
            .position(|arg| arg == "-f" || arg == "--log-file")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.clone());

        // Load web-proxy config file (can be overridden via CLI)
        let web_proxy_config_path = args
            .iter()
            .position(|arg| arg == "--web-proxy-config")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .ok_or("Missing required argument: --web-proxy-config")?;

        let web_proxy_config_str = fs::read_to_string(web_proxy_config_path).unwrap_or_default();
        let web_proxy_config: WebProxyConfig = if web_proxy_config_str.is_empty() {
            WebProxyConfig {
                server: ServerConfig::default(),
                metrics_store: MetricsStoreConfig::default(),
                monitoring_api: MonitoringApiConfig::default(),
                stats_proxy: StatsProxyConfig::default(),
                http_client: HttpClientConfig::default(),
            }
        } else {
            toml::from_str(&web_proxy_config_str)?
        };

        // Parse command line arguments (with config file as fallback)
        let metrics_store_url = args
            .iter()
            .position(|arg| arg == "--metrics-store-url" || arg == "-m")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| {
                args.iter()
                    .position(|arg| arg == "--stats-proxy-url" || arg == "-s")
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            })
            .or_else(|| web_proxy_config.metrics_store.url.clone())
            .or_else(|| web_proxy_config.stats_proxy.url.clone())
            .ok_or("Missing required config: metrics_store.url")?;

        let web_server_address = args
            .iter()
            .position(|arg| arg == "--web-address" || arg == "-w")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| web_proxy_config.server.listen_address)
            .ok_or("Missing required config: server.listen_address")?;

        // Load shared miner config to get network topology
        let config_path = args
            .iter()
            .position(|arg| arg == "--config" || arg == "-c")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .ok_or("Missing required argument: --config")?;

        let config_str = fs::read_to_string(config_path)?;
        let shared_network: toml::Value = toml::from_str(&config_str)?;

        let upstream_address = shared_network
            .get("network")
            .and_then(|n| n.get("upstream_address"))
            .and_then(|a| a.as_str())
            .ok_or("Missing required config: network.upstream_address")?
            .to_string();

        let upstream_port = shared_network
            .get("network")
            .and_then(|n| n.get("upstream_port"))
            .and_then(|p| p.as_integer())
            .ok_or("Missing required config: network.upstream_port")? as u16;

        let downstream_address = shared_network
            .get("network")
            .and_then(|n| n.get("downstream_address"))
            .and_then(|a| a.as_str())
            .ok_or("Missing required config: network.downstream_address")?
            .to_string();

        let downstream_port = shared_network
            .get("network")
            .and_then(|n| n.get("downstream_port"))
            .and_then(|p| p.as_integer())
            .ok_or("Missing required config: network.downstream_port")? as u16;

        let tproxy = TproxyConfig {
            downstream_address,
            downstream_port,
            upstream_address,
            upstream_port,
        };

        // Load shared miner config to get faucet configuration
        let shared_config_path = args
            .iter()
            .position(|arg| arg == "--shared-config" || arg == "-g")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .ok_or("Missing required argument: --shared-config")?;

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

        // Extract web_proxy poll intervals (with defaults)
        let metrics_query_step_secs = shared_config
            .get("web_proxy")
            .and_then(|w| {
                w.get("metrics_query_step_secs")
                    .or_else(|| w.get("stats_poll_interval_secs"))
            })
            .and_then(|i| i.as_integer())
            .unwrap_or(15) as u64;

        let client_poll_interval_secs = shared_config
            .get("web_proxy")
            .and_then(|w| w.get("client_poll_interval_secs"))
            .and_then(|i| i.as_integer())
            .unwrap_or(3) as u64;

        let monitoring_api_url = web_proxy_config
            .monitoring_api
            .url
            .unwrap_or_else(|| "http://127.0.0.1:9109".to_string());

        Ok(Config {
            metrics_store_url,
            monitoring_api_url,
            web_server_address,
            downstream_address: tproxy.downstream_address,
            downstream_port: tproxy.downstream_port,
            upstream_address: tproxy.upstream_address,
            upstream_port: tproxy.upstream_port,
            faucet_enabled,
            faucet_url,
            metrics_query_step_secs,
            client_poll_interval_secs,
            request_timeout_secs: web_proxy_config
                .http_client
                .request_timeout_secs
                .unwrap_or(60),
            pool_idle_timeout_secs: web_proxy_config
                .http_client
                .pool_idle_timeout_secs
                .unwrap_or(300),
            log_file,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_web_proxy_config_deserialization() {
        let toml_str = r#"
            [server]
            listen_address = "127.0.0.1:4000"

            [metrics_store]
            url = "http://prometheus.example.com:9090"

            [http_client]
            pool_idle_timeout_secs = 400
            request_timeout_secs = 85
        "#;
        let config: WebProxyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.server.listen_address,
            Some("127.0.0.1:4000".to_string())
        );
        assert_eq!(
            config.metrics_store.url,
            Some("http://prometheus.example.com:9090".to_string())
        );
        assert_eq!(config.http_client.pool_idle_timeout_secs, Some(400));
        assert_eq!(config.http_client.request_timeout_secs, Some(85));
    }

    #[test]
    fn test_tproxy_config_deserialization() {
        let toml_str = r#"
            downstream_address = "192.168.1.1"
            downstream_port = 4444
            upstream_address = "10.0.0.1"
            upstream_port = 5555
        "#;
        let config: TproxyConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.downstream_address, "192.168.1.1");
        assert_eq!(config.downstream_port, 4444);
        assert_eq!(config.upstream_address, "10.0.0.1");
        assert_eq!(config.upstream_port, 5555);
    }
}
