use tracing::info;
use tracing_subscriber;

use web_proxy::{config::Config, prometheus::PrometheusClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration
    let config = Config::from_args()?;

    // Setup tracing with optional file output
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt().with_env_filter(env_filter);

    if let Some(log_file) = &config.log_file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .map_err(|e| format!("Failed to open log file {}: {}", log_file, e))?;
        fmt_layer.with_writer(std::sync::Arc::new(file)).init();
    } else {
        fmt_layer.init();
    }

    info!("Starting web-proxy service");
    info!("Metrics store URL: {}", config.metrics_store_url);
    info!("Web server address: {}", config.web_server_address);
    info!(
        "Metrics query step: {}s",
        config.metrics_query_step_secs
    );
    info!(
        "Client poll interval: {}s",
        config.client_poll_interval_secs
    );

    let prometheus = PrometheusClient::new(
        config.metrics_store_url.clone(),
        config.request_timeout_secs,
        config.pool_idle_timeout_secs,
    )?;

    // Start HTTP server
    web_proxy::web::run_http_server(
        config.web_server_address,
        prometheus,
        config.monitoring_api_url,
        config.faucet_enabled,
        config.faucet_url,
        config.downstream_address,
        config.downstream_port,
        config.upstream_address,
        config.upstream_port,
        config.client_poll_interval_secs,
        config.metrics_query_step_secs,
    )
    .await?;

    Ok(())
}
