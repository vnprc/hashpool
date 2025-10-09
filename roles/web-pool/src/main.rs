use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{error, info};
use tracing_subscriber;
use stats::stats_adapter::PoolSnapshot;

use web_pool::{SnapshotStorage, config::Config};

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
    info!("Starting web-pool service");
    info!("Stats pool URL: {}", config.stats_pool_url);
    info!("Web server address: {}", config.web_server_address);

    // Create shared snapshot storage
    let storage = Arc::new(SnapshotStorage::new());

    // Spawn polling loop
    let storage_clone = storage.clone();
    let stats_pool_url = config.stats_pool_url.clone();
    tokio::spawn(async move {
        poll_stats_pool(storage_clone, stats_pool_url).await;
    });

    // Start HTTP server
    start_web_server(config.web_server_address, storage).await?;

    Ok(())
}

async fn poll_stats_pool(storage: Arc<SnapshotStorage>, stats_pool_url: String) {
    let client = reqwest::Client::builder()
        .pool_idle_timeout(Duration::from_secs(300))
        .pool_max_idle_per_host(1)
        .build()
        .unwrap();
    let mut interval = time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
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
) -> Result<(), Box<dyn std::error::Error>> {
    web_pool::web::run_http_server(address, storage).await
}
