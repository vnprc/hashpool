use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::AsyncReadExt;
use tracing::{error, info};

mod config;
mod web;

use config::Config;
use stats_proxy::db::StatsData;
use stats_proxy::stats_handler::StatsHandler;

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
    info!("Starting proxy-stats service");
    info!("TCP server: {}", config.tcp_address);
    info!("HTTP server: {}", config.http_address);

    // Initialize in-memory stats data storage
    let db = Arc::new(StatsData::new());
    info!("Stats data storage initialized");

    // Start TCP server for receiving stats messages
    let tcp_listener = TcpListener::bind(&config.tcp_address).await?;
    info!("TCP server listening on {}", config.tcp_address);

    // Start HTTP server for dashboard
    let http_address = config.http_address.clone();
    let downstream_address = config.downstream_address.clone();
    let downstream_port = config.downstream_port;
    let redact_ip = config.redact_ip;
    let faucet_enabled = config.faucet_enabled;
    let faucet_url = config.faucet_url.clone();
    let db_clone = db.clone();
    tokio::spawn(async move {
        if let Err(e) = web::run_http_server(http_address, db_clone, downstream_address, downstream_port, redact_ip, faucet_enabled, faucet_url).await {
            error!("HTTP server error: {}", e);
        }
    });

    // Accept TCP connections
    loop {
        match tcp_listener.accept().await {
            Ok((stream, addr)) => {
                info!("New pool connection from {}", addr);
                let db_clone = db.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_pool_connection(stream, addr, db_clone).await {
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
    db: Arc<StatsData>,
) -> Result<(), Box<dyn std::error::Error>> {
    let handler = StatsHandler::new(db);
    let mut buffer = vec![0u8; 8192];
    let mut leftover = Vec::new();

    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => {
                info!("Pool connection from {} closed", addr);
                break;
            }
            Ok(n) => {
                // Append new data to leftover from previous read
                leftover.extend_from_slice(&buffer[..n]);

                // Process newline-delimited JSON messages
                while let Some(newline_pos) = leftover.iter().position(|&b| b == b'\n') {
                    let line = &leftover[..newline_pos];

                    if !line.is_empty() {
                        if let Err(e) = handler.handle_message(line).await {
                            error!("Error processing message from {}: {}", addr, e);
                        }
                    }

                    // Remove processed line including newline
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
