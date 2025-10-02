use super::*;
use std::{collections::HashMap, time::Instant};
use tokio::{
    sync::broadcast,
    time::{timeout, Duration},
};

/// Central hub for mint-pool communication using MPSC broadcast streams
pub struct MintPoolMessageHub {
    config: MessagingConfig,

    // Pool -> Mint channels
    quote_request_tx: broadcast::Sender<ParsedMintQuoteRequest>,
    quote_request_rx: RwLock<Option<broadcast::Receiver<ParsedMintQuoteRequest>>>,

    // Mint -> Pool channels
    quote_response_tx: broadcast::Sender<MintQuoteResponseEvent>,
    quote_response_rx: RwLock<Option<broadcast::Receiver<MintQuoteResponseEvent>>>,

    // Error channels
    quote_error_tx: broadcast::Sender<MintQuoteError<'static>>,
    quote_error_rx: RwLock<Option<broadcast::Receiver<MintQuoteError<'static>>>>,

    // Active connections tracking
    connections: RwLock<HashMap<String, ConnectionInfo>>,
    pending_quotes: RwLock<HashMap<ShareHash, PendingQuote>>,
}

#[derive(Debug, Clone)]
struct ConnectionInfo {
    role: Role,
    connected_at: std::time::Instant,
}

#[derive(Debug, Clone)]
pub struct PendingQuoteContext {
    pub channel_id: u32,
    pub sequence_number: u32,
    pub amount: u64,
}

#[derive(Debug, Clone)]
struct PendingQuote {
    parsed: ParsedMintQuoteRequest,
    created_at: Instant,
    context: PendingQuoteContext,
}

#[derive(Debug, Clone)]
pub struct MintQuoteResponseEvent {
    pub response: MintQuoteResponse<'static>,
    pub share_hash: ShareHash,
    pub context: Option<PendingQuoteContext>,
}

impl MintQuoteResponseEvent {
    pub fn new(
        response: MintQuoteResponse<'static>,
        context: Option<PendingQuoteContext>,
    ) -> Result<Self, QuoteConversionError> {
        let share_hash =
            ShareHash::from_u256(&response.header_hash).map_err(QuoteConversionError::ShareHash)?;
        Ok(Self {
            response,
            share_hash,
            context,
        })
    }

    pub fn from_parts(
        response: MintQuoteResponse<'static>,
        share_hash: ShareHash,
        context: Option<PendingQuoteContext>,
    ) -> Self {
        Self {
            response,
            share_hash,
            context,
        }
    }

    pub fn response(&self) -> &MintQuoteResponse<'static> {
        &self.response
    }

    pub fn context(&self) -> Option<&PendingQuoteContext> {
        self.context.as_ref()
    }
}

impl MintPoolMessageHub {
    /// Create a new message hub with the given configuration
    pub fn new(config: MessagingConfig) -> Arc<Self> {
        let (quote_request_tx, quote_request_rx) = broadcast::channel(config.broadcast_buffer_size);
        let (quote_response_tx, quote_response_rx) =
            broadcast::channel(config.broadcast_buffer_size);
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
            pending_quotes: RwLock::new(HashMap::new()),
        })
    }

    /// Register a new connection (pool or mint)
    pub async fn register_connection(&self, connection_id: String, role: Role) {
        let mut connections = self.connections.write().await;
        connections.insert(
            connection_id.clone(),
            ConnectionInfo {
                role: role.clone(),
                connected_at: std::time::Instant::now(),
            },
        );

        info!(
            "Registered {} connection: {}",
            if role == Role::Pool { "pool" } else { "mint" },
            connection_id
        );
    }

    /// Unregister a connection
    pub async fn unregister_connection(&self, connection_id: &str) {
        let mut connections = self.connections.write().await;
        if connections.remove(connection_id).is_some() {
            info!("Unregistered connection: {}", connection_id);
        }
    }

    /// Track a pending quote request so responses can be correlated back to the originating share.
    /// Send a mint quote request (from pool to mint)
    pub async fn send_quote_request(
        &self,
        request: ParsedMintQuoteRequest,
        context: PendingQuoteContext,
    ) -> MessagingResult<()> {
        debug!(
            "Sending mint quote request: amount={} share_hash={}",
            request.request.amount, request.share_hash
        );

        {
            let mut guard = self.pending_quotes.write().await;
            guard.insert(
                request.share_hash,
                PendingQuote {
                    parsed: request.clone(),
                    created_at: Instant::now(),
                    context,
                },
            );
        }

        self.quote_request_tx
            .send(request)
            .map_err(|_| MessagingError::ChannelClosed("quote_request".to_string()))?;

        Ok(())
    }

    /// Send a mint quote response (from mint to pool) and return the dispatched event
    pub async fn send_quote_response(
        &self,
        response: MintQuoteResponse<'static>,
    ) -> MessagingResult<MintQuoteResponseEvent> {
        let share_hash = ShareHash::from_u256(&response.header_hash)
            .map_err(|e| MessagingError::Decoding(format!("invalid share hash: {e}")))?;

        let context = {
            let mut guard = self.pending_quotes.write().await;
            guard.remove(&share_hash).map(|pending| pending.context)
        };

        if context.is_none() {
            warn!(
                "Received mint quote response with no pending context for share hash {}",
                share_hash
            );
        }

        let event = MintQuoteResponseEvent {
            share_hash,
            context,
            response,
        };

        debug!(
            "Sending mint quote response: quote_id={} share_hash={}",
            std::str::from_utf8(event.response.quote_id.inner_as_ref()).unwrap_or("invalid"),
            event.share_hash
        );

        self.quote_response_tx
            .send(event.clone())
            .map_err(|_| MessagingError::ChannelClosed("quote_response".to_string()))?;

        Ok(event)
    }

    /// Send a mint quote error (from mint to pool)
    pub async fn send_quote_error(&self, error: MintQuoteError<'static>) -> MessagingResult<()> {
        debug!(
            "Sending mint quote error: code={}, message={}",
            error.error_code,
            std::str::from_utf8(error.error_message.inner_as_ref()).unwrap_or("invalid")
        );

        self.quote_error_tx
            .send(error)
            .map_err(|_| MessagingError::ChannelClosed("quote_error".to_string()))?;

        Ok(())
    }

    /// Subscribe to quote requests (for mint)
    pub async fn subscribe_quote_requests(
        &self,
    ) -> MessagingResult<broadcast::Receiver<ParsedMintQuoteRequest>> {
        Ok(self.quote_request_tx.subscribe())
    }

    /// Subscribe to quote responses (for pool)
    pub async fn subscribe_quote_responses(
        &self,
    ) -> MessagingResult<broadcast::Receiver<MintQuoteResponseEvent>> {
        Ok(self.quote_response_tx.subscribe())
    }

    /// Subscribe to quote errors (for pool)
    pub async fn subscribe_quote_errors(
        &self,
    ) -> MessagingResult<broadcast::Receiver<MintQuoteError<'static>>> {
        Ok(self.quote_error_tx.subscribe())
    }

    /// Receive a quote request with timeout (for mint)
    pub async fn receive_quote_request(&self) -> MessagingResult<ParsedMintQuoteRequest> {
        let mut rx = self.subscribe_quote_requests().await?;

        timeout(Duration::from_millis(self.config.timeout_ms), rx.recv())
            .await
            .map_err(|_| MessagingError::Timeout)?
            .map_err(|_| MessagingError::ChannelClosed("quote_request".to_string()))
    }

    /// Receive a quote response with timeout (for pool)
    pub async fn receive_quote_response(&self) -> MessagingResult<MintQuoteResponseEvent> {
        let mut rx = self.subscribe_quote_responses().await?;

        timeout(Duration::from_millis(self.config.timeout_ms), rx.recv())
            .await
            .map_err(|_| MessagingError::Timeout)?
            .map_err(|_| MessagingError::ChannelClosed("quote_response".to_string()))
    }

    /// Get statistics about the message hub
    pub async fn get_stats(&self) -> MessageHubStats {
        let connections = self.connections.read().await;
        let pending = self.pending_quotes.read().await;
        let now = Instant::now();
        let oldest_pending_ms = pending
            .values()
            .map(|p| now.duration_since(p.created_at).as_millis() as u64)
            .max();

        MessageHubStats {
            total_connections: connections.len(),
            pool_connections: connections
                .values()
                .filter(|c| c.role == Role::Pool)
                .count(),
            mint_connections: connections
                .values()
                .filter(|c| c.role == Role::Mint)
                .count(),
            quote_request_subscribers: self.quote_request_tx.receiver_count(),
            quote_response_subscribers: self.quote_response_tx.receiver_count(),
            quote_error_subscribers: self.quote_error_tx.receiver_count(),
            pending_quotes: pending.len(),
            oldest_pending_ms,
        }
    }

    /// Retrieve a tracked pending quote by share hash.
    pub async fn pending_quote(&self, share_hash: ShareHash) -> Option<ParsedMintQuoteRequest> {
        self.pending_quotes
            .read()
            .await
            .get(&share_hash)
            .map(|entry| entry.parsed.clone())
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
    pub pending_quotes: usize,
    pub oldest_pending_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_sv2::{CompressedPubKey, Deserialize, Str0255};

    fn locking_key() -> CompressedPubKey<'static> {
        let mut bytes = [0u8; 33];
        bytes[0] = 0x02;
        CompressedPubKey::from_bytes(&mut bytes)
            .expect("valid compressed key")
            .into_static()
    }

    #[test]
    fn build_parsed_quote_request_round_trip() {
        let hash = [0x11u8; 32];
        let parsed = crate::build_parsed_quote_request(42, &hash, locking_key()).unwrap();
        assert_eq!(parsed.request.amount, 42);
        assert_eq!(parsed.share_hash.as_bytes(), &hash);
    }

    #[tokio::test]
    async fn pending_quotes_track_round_trip() {
        let hub = MintPoolMessageHub::new(MessagingConfig::default());
        let mut req_rx = hub.subscribe_quote_requests().await.unwrap();
        let mut resp_rx = hub.subscribe_quote_responses().await.unwrap();

        let hash = [0xAAu8; 32];
        let parsed = crate::build_parsed_quote_request(7, &hash, locking_key()).unwrap();
        let context = PendingQuoteContext {
            channel_id: 1,
            sequence_number: 42,
            amount: 7,
        };

        hub.send_quote_request(parsed.clone(), context.clone())
            .await
            .unwrap();
        assert!(hub.pending_quote(parsed.share_hash).await.is_some());

        let received = req_rx.recv().await.unwrap();
        assert_eq!(received.share_hash, parsed.share_hash);

        let quote_id = Str0255::try_from("QUOTE".to_string()).unwrap();
        let header_hash = parsed.share_hash.into_u256().unwrap();
        let response = MintQuoteResponse {
            quote_id,
            header_hash,
        };
        let event = hub.send_quote_response(response).await.unwrap();

        let received_event = resp_rx.recv().await.unwrap();
        assert_eq!(received_event.share_hash, parsed.share_hash);
        assert!(received_event.context.is_some());
        let received_context = received_event.context.unwrap();
        assert_eq!(received_context.channel_id, context.channel_id);
        assert_eq!(received_context.sequence_number, context.sequence_number);
        assert_eq!(received_context.amount, context.amount);
        assert!(hub.pending_quote(parsed.share_hash).await.is_none());

        let stats = hub.get_stats().await;
        assert_eq!(stats.pending_quotes, 0);
    }
}
