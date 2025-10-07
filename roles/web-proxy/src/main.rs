use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{error, info};
use tracing_subscriber;
use stats::stats_adapter::ProxySnapshot;

use web_proxy::{SnapshotStorage, config::Config};

const POLL_INTERVAL_SECS: u64 = 5;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load configuration
    let config = Config::from_args()?;
    info!("Starting web-proxy service");
    info!("Stats proxy URL: {}", config.stats_proxy_url);
    info!("Web server address: {}", config.web_server_address);

    // Create shared snapshot storage
    let storage = Arc::new(SnapshotStorage::new());

    // Spawn polling loop
    let storage_clone = storage.clone();
    let stats_proxy_url = config.stats_proxy_url.clone();
    tokio::spawn(async move {
        poll_stats_proxy(storage_clone, stats_proxy_url).await;
    });

    // Start HTTP server
    start_web_server(
        config.web_server_address,
        storage,
        config.faucet_enabled,
        config.faucet_url,
        config.downstream_address,
        config.downstream_port,
    )
    .await?;

    Ok(())
}

async fn poll_stats_proxy(storage: Arc<SnapshotStorage>, stats_proxy_url: String) {
    let client = reqwest::Client::new();
    let mut interval = time::interval(Duration::from_secs(POLL_INTERVAL_SECS));

    loop {
        interval.tick().await;

        match client
            .get(format!("{}/api/stats", stats_proxy_url))
            .send()
            .await
        {
            Ok(response) => match response.json::<ProxySnapshot>().await {
                Ok(snapshot) => {
                    info!("Successfully fetched snapshot from stats-proxy");
                    storage.update(snapshot);
                }
                Err(e) => {
                    error!("Failed to parse snapshot JSON: {}", e);
                }
            },
            Err(e) => {
                error!("Failed to fetch from stats-proxy: {}", e);
            }
        }
    }
}

async fn start_web_server(
    address: String,
    storage: Arc<SnapshotStorage>,
    faucet_enabled: bool,
    faucet_url: Option<String>,
    downstream_address: String,
    downstream_port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    web_proxy::web::run_http_server(
        address,
        storage,
        faucet_enabled,
        faucet_url,
        downstream_address,
        downstream_port,
    )
    .await
}
