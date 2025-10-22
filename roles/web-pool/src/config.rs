use std::env;
use std::fs;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub stats_pool_url: String,
    pub web_server_address: String,
    pub stats_poll_interval_secs: u64,
    pub client_poll_interval_secs: u64,
    pub request_timeout_secs: u64,
    pub pool_idle_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
struct WebPoolConfig {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    stats_pool: StatsPoolConfig,
    #[serde(default)]
    http_client: HttpClientConfig,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    listen_address: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_address: Some("127.0.0.1:8081".to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct StatsPoolConfig {
    url: Option<String>,
}

impl Default for StatsPoolConfig {
    fn default() -> Self {
        Self {
            url: Some("http://127.0.0.1:9084".to_string()),
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

        // Load web-pool config file (can be overridden via CLI)
        let web_pool_config_path = args
            .iter()
            .position(|arg| arg == "--web-pool-config")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config/web-pool.config.toml");

        let web_pool_config_str = fs::read_to_string(web_pool_config_path)
            .unwrap_or_default();
        let web_pool_config: WebPoolConfig = if web_pool_config_str.is_empty() {
            WebPoolConfig {
                server: ServerConfig::default(),
                stats_pool: StatsPoolConfig::default(),
                http_client: HttpClientConfig::default(),
            }
        } else {
            toml::from_str(&web_pool_config_str)?
        };

        // Parse command line arguments (with config file as fallback)
        let stats_pool_url = args
            .iter()
            .position(|arg| arg == "--stats-pool-url" || arg == "-s")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| web_pool_config.stats_pool.url)
            .ok_or("Missing required config: stats_pool.url")?;

        let web_server_address = args
            .iter()
            .position(|arg| arg == "--web-address" || arg == "-w")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| web_pool_config.server.listen_address)
            .ok_or("Missing required config: server.listen_address")?;

        // Load polling intervals from shared pool config
        let shared_config_path = args
            .iter()
            .position(|arg| arg == "--shared-config" || arg == "-g")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config/shared/pool.toml");

        let shared_config_str = fs::read_to_string(shared_config_path)?;
        let shared_config: toml::Value = toml::from_str(&shared_config_str)?;

        // Extract web_pool poll intervals (with defaults)
        let stats_poll_interval_secs = shared_config
            .get("web_pool")
            .and_then(|w| w.get("stats_poll_interval_secs"))
            .and_then(|i| i.as_integer())
            .unwrap_or(3) as u64;

        let client_poll_interval_secs = shared_config
            .get("web_pool")
            .and_then(|w| w.get("client_poll_interval_secs"))
            .and_then(|i| i.as_integer())
            .unwrap_or(3) as u64;

        Ok(Config {
            stats_pool_url,
            web_server_address,
            stats_poll_interval_secs,
            client_poll_interval_secs,
            request_timeout_secs: web_pool_config.http_client.request_timeout_secs.unwrap_or(60),
            pool_idle_timeout_secs: web_pool_config.http_client.pool_idle_timeout_secs.unwrap_or(300),
        })
    }
}
