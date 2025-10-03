use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

/// Stats messages that can be sent to pool-stats service
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type")]
pub enum StatsMessage {
    ShareSubmitted {
        downstream_id: u32,
        timestamp: u64,
    },
    QuoteCreated {
        downstream_id: u32,
        amount: u64,
        timestamp: u64,
    },
    ChannelOpened {
        downstream_id: u32,
        channel_id: u32,
    },
    ChannelClosed {
        downstream_id: u32,
        channel_id: u32,
    },
    DownstreamConnected {
        downstream_id: u32,
        flags: u32,
        address: String,
        service_type: Option<String>, // "mint", "jd", "translator", etc.
    },
    DownstreamDisconnected {
        downstream_id: u32,
    },
    PoolInfo {
        listen_address: String,
    },
}

/// Client for sending stats to pool-stats service over TCP
pub struct StatsClient {
    stream: Arc<Mutex<Option<TcpStream>>>,
    server_address: String,
}

impl StatsClient {
    pub fn new(server_address: String) -> Self {
        Self {
            stream: Arc::new(Mutex::new(None)),
            server_address,
        }
    }

    /// Connect to stats server
    async fn ensure_connected(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut stream_guard = self.stream.lock().await;

        if stream_guard.is_none() {
            info!("Connecting to pool-stats server at {}", self.server_address);
            match TcpStream::connect(&self.server_address).await {
                Ok(stream) => {
                    info!("Connected to pool-stats server");
                    *stream_guard = Some(stream);
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to connect to pool-stats server: {}", e);
                    Err(Box::new(e))
                }
            }
        } else {
            Ok(())
        }
    }

    /// Send a stats message to the server
    pub async fn send_stats(&self, msg: StatsMessage) {
        if let Err(e) = self.try_send_stats(msg.clone()).await {
            warn!("Failed to send stats message: {}", e);

            // Try to reconnect and send again
            let mut stream_guard = self.stream.lock().await;
            *stream_guard = None;
            drop(stream_guard);

            // Retry once after reconnecting
            if self.ensure_connected().await.is_ok() {
                let _ = self.try_send_stats(msg).await;
            }
        }
    }

    async fn try_send_stats(&self, msg: StatsMessage) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_connected().await?;

        let json = serde_json::to_vec(&msg)?;
        let mut buffer = Vec::with_capacity(json.len() + 1);
        buffer.extend_from_slice(&json);
        buffer.push(b'\n');

        let mut stream_guard = self.stream.lock().await;
        if let Some(stream) = stream_guard.as_mut() {
            stream.write_all(&buffer).await?;
            stream.flush().await?;
        }

        Ok(())
    }
}

/// Handle for sending stats messages
#[derive(Clone)]
pub struct StatsHandle {
    client: Arc<StatsClient>,
}

impl StatsHandle {
    pub fn new(server_address: String) -> Self {
        Self {
            client: Arc::new(StatsClient::new(server_address)),
        }
    }

    pub fn send_stats(&self, msg: StatsMessage) {
        let client = self.client.clone();
        tokio::spawn(async move {
            client.send_stats(msg).await;
        });
    }
}
