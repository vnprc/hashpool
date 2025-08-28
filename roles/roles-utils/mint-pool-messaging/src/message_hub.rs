use super::*;
use std::collections::HashMap;
use tokio::sync::broadcast;
use tokio::time::{timeout, Duration};

/// Central hub for mint-pool communication using MPSC broadcast streams
pub struct MintPoolMessageHub {
    config: MessagingConfig,
    
    // Pool -> Mint channels
    quote_request_tx: broadcast::Sender<MintQuoteRequest<'static>>,
    quote_request_rx: RwLock<Option<broadcast::Receiver<MintQuoteRequest<'static>>>>,
    
    // Mint -> Pool channels
    quote_response_tx: broadcast::Sender<MintQuoteResponse<'static>>,
    quote_response_rx: RwLock<Option<broadcast::Receiver<MintQuoteResponse<'static>>>>,
    
    // Error channels
    quote_error_tx: broadcast::Sender<MintQuoteError<'static>>,
    quote_error_rx: RwLock<Option<broadcast::Receiver<MintQuoteError<'static>>>>,
    
    // Active connections tracking
    connections: RwLock<HashMap<String, ConnectionInfo>>,
}

#[derive(Debug, Clone)]
struct ConnectionInfo {
    role: Role,
    connected_at: std::time::Instant,
}

impl MintPoolMessageHub {
    /// Create a new message hub with the given configuration
    pub fn new(config: MessagingConfig) -> Arc<Self> {
        let (quote_request_tx, quote_request_rx) = broadcast::channel(config.broadcast_buffer_size);
        let (quote_response_tx, quote_response_rx) = broadcast::channel(config.broadcast_buffer_size);
        let (quote_error_tx, quote_error_rx) = broadcast::channel(config.broadcast_buffer_size);
        
        Arc::new(Self {
            config,
            quote_request_tx,
            quote_request_rx: RwLock::new(Some(quote_request_rx)),
            quote_response_tx,
            quote_response_rx: RwLock::new(Some(quote_response_rx)),
            quote_error_tx,
            quote_error_rx: RwLock::new(Some(quote_error_rx)),
            connections: RwLock::new(HashMap::new()),
        })
    }
    
    /// Register a new connection (pool or mint)
    pub async fn register_connection(&self, connection_id: String, role: Role) {
        let mut connections = self.connections.write().await;
        connections.insert(connection_id.clone(), ConnectionInfo {
            role: role.clone(),
            connected_at: std::time::Instant::now(),
        });
        
        info!("Registered {} connection: {}", 
              if role == Role::Pool { "pool" } else { "mint" }, 
              connection_id);
    }
    
    /// Unregister a connection
    pub async fn unregister_connection(&self, connection_id: &str) {
        let mut connections = self.connections.write().await;
        if connections.remove(connection_id).is_some() {
            info!("Unregistered connection: {}", connection_id);
        }
    }
    
    /// Send a mint quote request (from pool to mint)
    pub async fn send_quote_request(&self, request: MintQuoteRequest<'static>) -> MessagingResult<()> {
        debug!("Sending mint quote request: amount={}", request.amount);
        
        self.quote_request_tx
            .send(request)
            .map_err(|_| MessagingError::ChannelClosed("quote_request".to_string()))?;
            
        Ok(())
    }
    
    /// Send a mint quote response (from mint to pool)
    pub async fn send_quote_response(&self, response: MintQuoteResponse<'static>) -> MessagingResult<()> {
        debug!("Sending mint quote response: quote_id={}", 
               std::str::from_utf8(response.quote_id.inner_as_ref()).unwrap_or("invalid"));
        
        self.quote_response_tx
            .send(response)
            .map_err(|_| MessagingError::ChannelClosed("quote_response".to_string()))?;
            
        Ok(())
    }
    
    /// Send a mint quote error (from mint to pool)
    pub async fn send_quote_error(&self, error: MintQuoteError<'static>) -> MessagingResult<()> {
        debug!("Sending mint quote error: code={}, message={}", 
               error.error_code,
               std::str::from_utf8(error.error_message.inner_as_ref()).unwrap_or("invalid"));
        
        self.quote_error_tx
            .send(error)
            .map_err(|_| MessagingError::ChannelClosed("quote_error".to_string()))?;
            
        Ok(())
    }
    
    /// Subscribe to quote requests (for mint)
    pub async fn subscribe_quote_requests(&self) -> MessagingResult<broadcast::Receiver<MintQuoteRequest<'static>>> {
        Ok(self.quote_request_tx.subscribe())
    }
    
    /// Subscribe to quote responses (for pool)
    pub async fn subscribe_quote_responses(&self) -> MessagingResult<broadcast::Receiver<MintQuoteResponse<'static>>> {
        Ok(self.quote_response_tx.subscribe())
    }
    
    /// Subscribe to quote errors (for pool)
    pub async fn subscribe_quote_errors(&self) -> MessagingResult<broadcast::Receiver<MintQuoteError<'static>>> {
        Ok(self.quote_error_tx.subscribe())
    }
    
    /// Receive a quote request with timeout (for mint)
    pub async fn receive_quote_request(&self) -> MessagingResult<MintQuoteRequest<'static>> {
        let mut rx = self.subscribe_quote_requests().await?;
        
        timeout(
            Duration::from_millis(self.config.timeout_ms),
            rx.recv()
        )
        .await
        .map_err(|_| MessagingError::Timeout)?
        .map_err(|_| MessagingError::ChannelClosed("quote_request".to_string()))
    }
    
    /// Receive a quote response with timeout (for pool)
    pub async fn receive_quote_response(&self) -> MessagingResult<MintQuoteResponse<'static>> {
        let mut rx = self.subscribe_quote_responses().await?;
        
        timeout(
            Duration::from_millis(self.config.timeout_ms),
            rx.recv()
        )
        .await
        .map_err(|_| MessagingError::Timeout)?
        .map_err(|_| MessagingError::ChannelClosed("quote_response".to_string()))
    }
    
    /// Get statistics about the message hub
    pub async fn get_stats(&self) -> MessageHubStats {
        let connections = self.connections.read().await;
        
        MessageHubStats {
            total_connections: connections.len(),
            pool_connections: connections.values().filter(|c| c.role == Role::Pool).count(),
            mint_connections: connections.values().filter(|c| c.role == Role::Mint).count(),
            quote_request_subscribers: self.quote_request_tx.receiver_count(),
            quote_response_subscribers: self.quote_response_tx.receiver_count(),
            quote_error_subscribers: self.quote_error_tx.receiver_count(),
        }
    }
}

/// Statistics about the message hub
#[derive(Debug)]
pub struct MessageHubStats {
    pub total_connections: usize,
    pub pool_connections: usize,
    pub mint_connections: usize,
    pub quote_request_subscribers: usize,
    pub quote_response_subscribers: usize,
    pub quote_error_subscribers: usize,
}