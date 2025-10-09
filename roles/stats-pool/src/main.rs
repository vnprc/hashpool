use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tracing::{error, info};

mod config;
mod api;

use config::Config;
use stats_pool::db::StatsData;
use stats_pool::stats_handler::StatsHandler;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_args()?;
    info!("Starting pool-stats service");
    info!("TCP server: {}", config.tcp_address);
    info!("HTTP server: {}", config.http_address);

    let stats = Arc::new(StatsData::new());

    let tcp_listener = TcpListener::bind(&config.tcp_address).await?;
    info!("TCP server listening on {}", config.tcp_address);

    // HTTP API server exposes snapshots to web services
    let http_address = config.http_address.clone();
    let stats_for_http = stats.clone();
    tokio::spawn(async move {
        if let Err(e) = api::run_http_server(http_address, stats_for_http).await {
            error!("HTTP server error: {}", e);
        }
    });

    loop {
        match tcp_listener.accept().await {
            Ok((stream, addr)) => {
                info!("New pool connection from {}", addr);
                let stats_clone = stats.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_pool_connection(stream, addr, stats_clone).await {
                        error!("Error handling pool connection from {}: {}", addr, e);
                    }
                });
            }
            Err(e) => {
                error!("Error accepting connection: {}", e);
            }
        }
    }
}

async fn handle_pool_connection(
    mut stream: TcpStream,
    addr: SocketAddr,
    stats: Arc<StatsData>,
) -> Result<(), Box<dyn std::error::Error>> {
    let handler = StatsHandler::new(stats);
    let mut buffer = vec![0u8; 8192];
    let mut leftover = Vec::new();

    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => {
                info!("Pool connection from {} closed", addr);
                break;
            }
            Ok(n) => {
                leftover.extend_from_slice(&buffer[..n]);

                while let Some(newline_pos) = leftover.iter().position(|&b| b == b'\n') {
                    let line = &leftover[..newline_pos];

                    if !line.is_empty() {
                        if let Err(e) = handler.handle_message(line).await {
                            error!("Error processing message from {}: {}", addr, e);
                        }
                    }

                    leftover.drain(..=newline_pos);
                }
            }
            Err(e) => {
                error!("Error reading from {}: {}", addr, e);
                break;
            }
        }
    }

    Ok(())
}
