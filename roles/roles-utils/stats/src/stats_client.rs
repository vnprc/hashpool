use serde::Serialize;
use std::marker::PhantomData;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// TCP client that sends JSON snapshots to stats service
/// Generic over snapshot type
pub struct StatsClient<T> {
    address: String,
    stream: Arc<Mutex<Option<TcpStream>>>,
    _phantom: PhantomData<T>,
}

impl<T> StatsClient<T>
where
    T: Serialize,
{
    /// Create a new stats client
    pub fn new(address: String) -> Self {
        Self {
            address,
            stream: Arc::new(Mutex::new(None)),
            _phantom: PhantomData,
        }
    }

    /// Send a snapshot to the stats service
    /// Uses newline-delimited JSON format
    /// Maintains persistent connection, auto-reconnects on failure
    pub async fn send_snapshot(&self, snapshot: T) -> Result<(), StatsClientError> {
        // Serialize to JSON
        let json = serde_json::to_string(&snapshot)
            .map_err(|e| StatsClientError::SerializationError(e.to_string()))?;

        // Add newline delimiter
        let message = format!("{}\n", json);

        // Try to send using existing connection, reconnect if needed
        match self.try_send(&message).await {
            Ok(_) => {
                debug!("Successfully sent snapshot to {}", self.address);
                Ok(())
            }
            Err(e) => {
                warn!(
                    "Failed to send snapshot to {}: {}",
                    self.address, e
                );
                Err(e)
            }
        }
    }

    async fn try_send(&self, message: &str) -> Result<(), StatsClientError> {
        let mut stream_guard = self.stream.lock().await;

        // Try to use existing connection first
        if let Some(ref mut stream) = *stream_guard {
            match stream.write_all(message.as_bytes()).await {
                Ok(_) => {
                    if let Err(e) = stream.flush().await {
                        warn!("Flush failed, reconnecting: {}", e);
                        *stream_guard = None;
                    } else {
                        return Ok(());
                    }
                }
                Err(e) => {
                    warn!("Write failed, reconnecting: {}", e);
                    *stream_guard = None;
                }
            }
        }

        // Connection doesn't exist or failed, establish new one
        let mut new_stream = TcpStream::connect(&self.address)
            .await
            .map_err(|e| StatsClientError::ConnectionError(e.to_string()))?;

        // Send message on new connection
        new_stream
            .write_all(message.as_bytes())
            .await
            .map_err(|e| StatsClientError::WriteError(e.to_string()))?;

        new_stream
            .flush()
            .await
            .map_err(|e| StatsClientError::WriteError(e.to_string()))?;

        // Store the connection for reuse
        *stream_guard = Some(new_stream);

        Ok(())
    }
}

#[derive(Debug)]
pub enum StatsClientError {
    ConnectionError(String),
    WriteError(String),
    SerializationError(String),
}

impl std::fmt::Display for StatsClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatsClientError::ConnectionError(e) => write!(f, "Connection error: {}", e),
            StatsClientError::WriteError(e) => write!(f, "Write error: {}", e),
            StatsClientError::SerializationError(e) => write!(f, "Serialization error: {}", e),
        }
    }
}

impl std::error::Error for StatsClientError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats_adapter::ProxySnapshot;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_stats_client_sends_json() {
        // Start a mock TCP server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 1024];
            let n = socket.read(&mut buf).await.unwrap();
            let received = String::from_utf8_lossy(&buf[..n]);
            assert!(received.contains("ehash_balance"));
            assert!(received.ends_with('\n'));
        });

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Send snapshot via client
        let client = StatsClient::<ProxySnapshot>::new(addr.to_string());
        let snapshot = ProxySnapshot {
            ehash_balance: 500,
            upstream_pool: None,
            downstream_miners: vec![],
            timestamp: 123456,
        };
        client.send_snapshot(snapshot).await.unwrap();

        // Wait for server to finish
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_stats_client_connection_error() {
        // Try to connect to non-existent server
        let client = StatsClient::<ProxySnapshot>::new("127.0.0.1:1".to_string());
        let snapshot = ProxySnapshot {
            ehash_balance: 100,
            upstream_pool: None,
            downstream_miners: vec![],
            timestamp: 123,
        };
        let result = client.send_snapshot(snapshot).await;
        assert!(result.is_err());
    }
}
