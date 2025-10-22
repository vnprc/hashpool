use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{error, info};
use tracing_subscriber;
use stats::stats_adapter::PoolSnapshot;

use web_pool::{SnapshotStorage, config::Config};

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
    info!("Starting web-pool service");
    info!("Stats pool URL: {}", config.stats_pool_url);
    info!("Web server address: {}", config.web_server_address);
    info!("Stats polling interval: {} seconds", config.stats_poll_interval_secs);
    info!("Client polling interval: {} seconds", config.client_poll_interval_secs);

    // Create shared snapshot storage
    let storage = Arc::new(SnapshotStorage::new());

    // Spawn polling loop
    let storage_clone = storage.clone();
    let stats_pool_url = config.stats_pool_url.clone();
    let poll_interval = config.stats_poll_interval_secs;
    let request_timeout = config.request_timeout_secs;
    let pool_idle_timeout = config.pool_idle_timeout_secs;
    tokio::spawn(async move {
        poll_stats_pool(storage_clone, stats_pool_url, poll_interval, request_timeout, pool_idle_timeout).await;
    });

    // Start HTTP server with client polling interval
    start_web_server(config.web_server_address, storage, config.client_poll_interval_secs).await?;

    Ok(())
}

async fn poll_stats_pool(storage: Arc<SnapshotStorage>, stats_pool_url: String, poll_interval_secs: u64, request_timeout_secs: u64, pool_idle_timeout_secs: u64) {
    let client = reqwest::Client::builder()
        .pool_idle_timeout(Duration::from_secs(pool_idle_timeout_secs))
        .pool_max_idle_per_host(1)
        .build()
        .unwrap();
    let mut interval = time::interval(Duration::from_secs(poll_interval_secs));
    let mut last_success = false;

    loop {
        interval.tick().await;

        match client
            .get(format!("{}/api/stats", stats_pool_url))
            .send()
            .await
        {
            Ok(response) => match response.json::<PoolSnapshot>().await {
                Ok(snapshot) => {
                    if !last_success {
                        info!("Successfully fetched snapshot from stats-pool");
                        last_success = true;
                    }
                    storage.update(snapshot);
                }
                Err(e) => {
                    if last_success {
                        error!("Failed to parse snapshot JSON: {}", e);
                        last_success = false;
                    }
                }
            },
            Err(e) => {
                if last_success {
                    error!("Failed to fetch from stats-pool: {}", e);
                    last_success = false;
                }
            }
        }
    }
}

async fn start_web_server(
    address: String,
    storage: Arc<SnapshotStorage>,
    client_poll_interval_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    web_pool::web::run_http_server(address, storage, client_poll_interval_secs).await
}
