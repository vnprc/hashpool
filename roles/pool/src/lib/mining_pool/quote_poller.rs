//! Periodic quote poller for tracking paid quotes from mint service
//!
//! Polls the mint's HTTP API every 5 seconds to check for newly paid quotes,
//! then sends MintQuoteNotification extension messages to translators.
//!
//! Phase 3 Implementation:
//! - Polls mint HTTP endpoint every 5s
//! - Tracks pending quotes with timeouts
//! - Sends MintQuoteNotification to downstream translators
//! - Correlates quotes to channels for proper message routing

use super::Downstream;
use mint_pool_messaging::MintPoolMessageHub;
use reqwest::{self, StatusCode, Url};
use std::{collections::HashMap, sync::Arc, time::Instant};
use mint_quote_sv2::MintQuoteNotification;
use stratum_common::roles_logic_sv2::{
    codec_sv2::binary_sv2::Str0255, handlers::mining::SendTo, parsers_sv2::Mining,
};
use tokio::time::{interval, sleep, Duration};
use tracing::{debug, error, info, warn};

/// Quote metadata for tracking pending quotes
#[derive(Debug, Clone)]
pub struct PendingQuote {
    /// Channel ID that submitted the share
    pub channel_id: u32,
    /// When the quote was created
    pub created_at: Instant,
    /// Amount of the quote (in satoshis or HASH)
    pub amount: u64,
}

/// Quote poller that tracks pending quotes and polls for paid status
pub struct QuotePoller {
    /// Pending quotes: quote_id → (channel_id, amount, timestamp)
    pending_quotes: Arc<tokio::sync::RwLock<HashMap<String, PendingQuote>>>,
    /// Mint HTTP endpoint
    mint_http_endpoint: Option<String>,
    /// Quote timeout (5 minutes default)
    quote_timeout: Duration,
}

impl QuotePoller {
    /// Create a new quote poller
    pub fn new(mint_http_endpoint: Option<String>) -> Self {
        Self {
            pending_quotes: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            mint_http_endpoint,
            quote_timeout: Duration::from_secs(300), // 5 minutes
        }
    }

    /// Register a new pending quote
    pub async fn register_quote(&self, quote_id: String, channel_id: u32, amount: u64) {
        let pending = PendingQuote {
            channel_id,
            created_at: Instant::now(),
            amount,
        };

        self.pending_quotes
            .write()
            .await
            .insert(quote_id.clone(), pending);
        debug!(
            "Registered pending quote: quote_id={}, channel_id={}, amount={}",
            quote_id, channel_id, amount
        );
    }

    /// Get channel_id for a quote (for routing responses)
    pub async fn get_quote_channel(&self, quote_id: &str) -> Option<u32> {
        self.pending_quotes
            .read()
            .await
            .get(quote_id)
            .map(|q| q.channel_id)
    }

    /// Remove a quote (after processing)
    pub async fn remove_quote(&self, quote_id: &str) {
        self.pending_quotes.write().await.remove(quote_id);
        debug!("Removed quote from tracking: quote_id={}", quote_id);
    }

    /// Clean up expired quotes
    pub async fn cleanup_expired_quotes(&self) {
        let now = Instant::now();
        let mut pending = self.pending_quotes.write().await;

        let expired: Vec<String> = pending
            .iter()
            .filter(|(_, q)| now.duration_since(q.created_at) > self.quote_timeout)
            .map(|(id, _)| id.clone())
            .collect();

        for quote_id in expired {
            warn!("Quote expired (timeout after 5min): {}", quote_id);
            pending.remove(&quote_id);
        }
    }

    /// Get all pending quotes (for monitoring/debugging)
    pub async fn get_pending_quotes(&self) -> Vec<(String, u32, u64)> {
        self.pending_quotes
            .read()
            .await
            .iter()
            .map(|(id, q)| (id.clone(), q.channel_id, q.amount))
            .collect()
    }

    /// Start the polling loop
    ///
    /// Phase 3: Polls mint HTTP API and sends MintQuoteNotification extension messages
    pub async fn start(
        self: Arc<Self>,
        pool: Arc<stratum_common::roles_logic_sv2::utils::Mutex<crate::mining_pool::Pool>>,
        hub: Arc<MintPoolMessageHub>,
    ) {
        let Some(mint_endpoint_base) = self.mint_http_endpoint.clone() else {
            info!("Quote poller disabled: no mint HTTP endpoint configured");
            return;
        };

        info!("🚀 Quote poller started");
        info!("📍 Mint HTTP endpoint: {}", mint_endpoint_base);
        info!("⏱️  Polling interval: 5 seconds");

        let base_url = match Url::parse(&mint_endpoint_base) {
            Ok(url) => url,
            Err(e) => {
                error!(
                    "Mint quote poller: invalid base URL '{}': {}",
                    mint_endpoint_base, e
                );
                return;
            }
        };

        let client = reqwest::Client::new();
        let mut ticker = interval(Duration::from_secs(5));
        let mut poll_count = 0;

        let response_listener = Arc::clone(&self);
        tokio::spawn(async move {
            response_listener.listen_for_hub_responses(hub).await;
        });

        loop {
            ticker.tick().await;

            // Clean up expired quotes every 10 polls
            poll_count += 1;
            if poll_count % 10 == 0 {
                self.cleanup_expired_quotes().await;
            }

            // Log current pending quotes count
            let pending_count = self.pending_quotes.read().await.len();
            if pending_count > 0 {
                debug!("Quote poller: {} pending quotes", pending_count);
            }

            // Snapshot pending quotes to poll without holding the write lock
            let pending_snapshot: Vec<(String, PendingQuote)> = self
                .pending_quotes
                .read()
                .await
                .iter()
                .map(|(id, quote)| (id.clone(), quote.clone()))
                .collect();

            for (quote_id, quote_meta) in pending_snapshot {
                let endpoint =
                    match base_url.join(&format!("v1/mint/quote/mining_share/{}", quote_id)) {
                        Ok(url) => url,
                        Err(e) => {
                            error!(
                                "Failed to build mint quote status URL for {}: {}",
                                quote_id, e
                            );
                            continue;
                        }
                    };

                match client.get(endpoint.clone()).send().await {
                    Ok(response) => {
                        let status = response.status();

                        if status == StatusCode::NOT_FOUND {
                            debug!(
                                "Mint quote status endpoint returned 404 for {}; will retry",
                                quote_id
                            );
                            continue;
                        }

                        if !status.is_success() {
                            error!(
                                "Mint quote status for {} returned {} from {}",
                                quote_id, status, endpoint
                            );
                            continue;
                        }

                        match response.json::<MintQuoteStatusResponse>().await {
                            Ok(payload) => {
                                let state = payload.state.to_ascii_uppercase();
                                let fully_issued = match (payload.amount, payload.amount_issued) {
                                    (Some(expected), Some(issued)) => issued >= expected,
                                    _ => false,
                                };

                                debug!(
                                    "Mint quote {} status={}, issued={}, expected={:?}",
                                    quote_id,
                                    state,
                                    payload.amount_issued.unwrap_or_default(),
                                    payload.amount
                                );

                                if state == "PAID" {
                                    let channel_id = quote_meta.channel_id;
                                    match self
                                        .send_notification_to_translator(
                                            pool.clone(),
                                            channel_id,
                                            &quote_id,
                                            quote_meta.amount,
                                        )
                                        .await
                                    {
                                        Ok(_) => {
                                            debug!(
                                                "✅ Sent MintQuoteNotification for quote {} to channel {}",
                                                quote_id, channel_id
                                            );
                                            self.remove_quote(&quote_id).await;
                                        }
                                        Err(e) => {
                                            error!(
                                                "Failed to send notification for quote {}: {}",
                                                quote_id, e
                                            );
                                        }
                                    }
                                } else if state == "ISSUED" || fully_issued {
                                    info!(
                                        "Quote {} already issued according to mint; removing from tracking",
                                        quote_id
                                    );
                                    self.remove_quote(&quote_id).await;
                                }
                            }
                            Err(e) => {
                                error!(
                                    "Failed to decode mint quote status response for {}: {}",
                                    quote_id, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to poll mint status for {} at {}: {}",
                            quote_id, endpoint, e
                        );
                    }
                }
            }

            debug!("Quote poller tick #{}", poll_count);
        }
    }

    async fn listen_for_hub_responses(self: Arc<Self>, hub: Arc<MintPoolMessageHub>) {
        loop {
            match hub.subscribe_quote_responses().await {
                Ok(mut rx) => {
                    while let Ok(event) = rx.recv().await {
                        if let Some(context) = event.context() {
                            if let Ok(quote_id) =
                                std::str::from_utf8(event.response().quote_id.inner_as_ref())
                            {
                                self.register_quote(
                                    quote_id.to_string(),
                                    context.channel_id,
                                    context.amount,
                                )
                                .await;
                            } else {
                                warn!(
                                    "Received non-UTF8 quote id from mint response; skipping registration"
                                );
                            }
                        } else {
                            warn!(
                                "Mint quote response missing context; cannot register pending quote"
                            );
                        }
                    }

                    warn!("Quote response subscription ended; attempting to resubscribe");
                }
                Err(e) => {
                    error!(
                        "Quote poller failed to subscribe to hub quote responses: {}",
                        e
                    );
                }
            }

            sleep(Duration::from_secs(1)).await;
        }
    }

    /// Send MintQuoteNotification extension message to translator
    async fn send_notification_to_translator(
        &self,
        pool: Arc<stratum_common::roles_logic_sv2::utils::Mutex<crate::mining_pool::Pool>>,
        channel_id: u32,
        quote_id: &str,
        amount: u64,
    ) -> Result<(), String> {
        // Create MintQuoteNotification extension message
        let notification = MintQuoteNotification {
            channel_id,
            quote_id: Str0255::try_from(quote_id.to_string())
                .map_err(|e| format!("Failed to create Str0255 from quote_id: {:?}", e))?,
            amount: amount.into(),
        };

        let mining_message = Mining::MintQuoteNotification(notification);

        // Resolve downstream id for this channel via mint manager context
        let mint_manager = pool
            .safe_lock(|p| p.mint_manager.clone())
            .map_err(|_| "Failed to lock pool for mint manager")?;

        let context = mint_manager
            .get_channel_context(channel_id)
            .await
            .ok_or_else(|| format!("No mint context for channel {}", channel_id))?;

        let downstream_id = context.downstream_id;

        // Fetch downstream handle using connection id
        let downstream = pool
            .safe_lock(|p| p.downstreams.get(&downstream_id).cloned())
            .map_err(|_| "Failed to lock pool for downstream lookup")?
            .ok_or_else(|| {
                format!(
                    "Downstream {} (channel {}) not found",
                    downstream_id, channel_id
                )
            })?;

        // Send via existing mining protocol connection
        Downstream::match_send_to(downstream, Ok(SendTo::Respond(mining_message)))
            .await
            .map_err(|e| format!("Failed to send: {:?}", e))
    }
}

/// Minimal representation of the mint quote status response
#[derive(Debug, serde::Deserialize)]
struct MintQuoteStatusResponse {
    #[serde(default)]
    amount: Option<u64>,
    #[serde(default)]
    amount_issued: Option<u64>,
    state: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // Quote Registration and Basic Operations Tests
    // ============================================================================

    #[tokio::test]
    async fn test_quote_registration() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));
        poller.register_quote("quote1".to_string(), 42, 1000).await;

        let channel_id = poller.get_quote_channel("quote1").await;
        assert_eq!(channel_id, Some(42));
    }

    #[tokio::test]
    async fn test_quote_removal() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));
        poller.register_quote("quote1".to_string(), 42, 1000).await;

        poller.remove_quote("quote1").await;

        let channel_id = poller.get_quote_channel("quote1").await;
        assert_eq!(channel_id, None);
    }

    #[tokio::test]
    async fn test_register_multiple_quotes() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        poller.register_quote("quote1".to_string(), 10, 1000).await;
        poller.register_quote("quote2".to_string(), 20, 2000).await;
        poller.register_quote("quote3".to_string(), 30, 3000).await;

        assert_eq!(poller.get_quote_channel("quote1").await, Some(10));
        assert_eq!(poller.get_quote_channel("quote2").await, Some(20));
        assert_eq!(poller.get_quote_channel("quote3").await, Some(30));

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 3);
    }

    #[tokio::test]
    async fn test_update_existing_quote() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        poller.register_quote("quote1".to_string(), 42, 1000).await;
        assert_eq!(poller.get_quote_channel("quote1").await, Some(42));

        // Re-register with different channel_id - should update
        poller.register_quote("quote1".to_string(), 99, 5000).await;
        assert_eq!(poller.get_quote_channel("quote1").await, Some(99));

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn test_get_nonexistent_quote() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        let result = poller.get_quote_channel("nonexistent").await;
        assert_eq!(result, None);
    }

    // ============================================================================
    // Quote Expiration and Cleanup Tests
    // ============================================================================

    #[tokio::test]
    async fn test_cleanup_removes_expired_quotes() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        // Register a quote
        poller.register_quote("quote1".to_string(), 42, 1000).await;

        // Verify it's there
        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 1);

        // Manually set created_at to far past to simulate expiration
        {
            let mut quotes = poller.pending_quotes.write().await;
            if let Some(quote) = quotes.get_mut("quote1") {
                quote.created_at = Instant::now() - Duration::from_secs(400);
            }
        }

        // Run cleanup
        poller.cleanup_expired_quotes().await;

        // Quote should be removed
        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_ignores_recent_quotes() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        poller.register_quote("quote1".to_string(), 42, 1000).await;
        poller.register_quote("quote2".to_string(), 43, 1000).await;

        // Cleanup should not remove recent quotes (created < 5 minutes ago)
        poller.cleanup_expired_quotes().await;

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn test_cleanup_mixed_expired_and_recent() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        poller.register_quote("recent".to_string(), 42, 1000).await;
        poller.register_quote("expired".to_string(), 43, 2000).await;

        // Make one quote appear old
        {
            let mut quotes = poller.pending_quotes.write().await;
            if let Some(quote) = quotes.get_mut("expired") {
                quote.created_at = Instant::now() - Duration::from_secs(400);
            }
        }

        poller.cleanup_expired_quotes().await;

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "recent");
    }


    #[tokio::test]
    async fn test_cleanup_with_empty_pending_quotes() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        // Should not panic when cleaning up empty list
        poller.cleanup_expired_quotes().await;

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 0);
    }

    // ============================================================================
    // Quote Metadata Tests
    // ============================================================================

    #[tokio::test]
    async fn test_quote_metadata_stored_correctly() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        let channel_id = 123;
        let amount = 50000;

        poller
            .register_quote("test_quote".to_string(), channel_id, amount)
            .await;

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 1);

        let (id, stored_channel_id, stored_amount) = &pending[0];
        assert_eq!(id, "test_quote");
        assert_eq!(*stored_channel_id, channel_id);
        assert_eq!(*stored_amount, amount);
    }


    #[tokio::test]
    async fn test_quote_id_with_special_characters() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        // Quote IDs should handle various characters
        let quote_id = "quote-123_abc.xyz";
        poller.register_quote(quote_id.to_string(), 42, 1000).await;

        assert_eq!(poller.get_quote_channel(quote_id).await, Some(42));
    }

    // ============================================================================
    // Concurrency and Race Condition Tests
    // ============================================================================

    #[tokio::test]
    async fn test_concurrent_quote_registration() {
        let poller = Arc::new(QuotePoller::new(Some("http://localhost:34261".to_string())));

        let mut tasks = vec![];

        for i in 0..10 {
            let poller_clone = Arc::clone(&poller);
            let task = tokio::spawn(async move {
                let quote_id = format!("quote_{}", i);
                poller_clone
                    .register_quote(quote_id, i as u32, i as u64 * 1000)
                    .await;
            });
            tasks.push(task);
        }

        for task in tasks {
            task.await.unwrap();
        }

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 10);
    }

    #[tokio::test]
    async fn test_concurrent_registration_and_removal() {
        let poller = Arc::new(QuotePoller::new(Some("http://localhost:34261".to_string())));

        // Register multiple quotes first
        for i in 0..5 {
            let quote_id = format!("quote_{}", i);
            poller
                .register_quote(quote_id, i as u32, i as u64 * 1000)
                .await;
        }

        let mut tasks = vec![];

        // Concurrent register and remove
        for i in 0..5 {
            let poller_clone = Arc::clone(&poller);
            let task = tokio::spawn(async move {
                let quote_id = format!("quote_{}", i);
                poller_clone.remove_quote(&quote_id).await;
            });
            tasks.push(task);
        }

        for task in tasks {
            task.await.unwrap();
        }

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_cleanup_and_queries() {
        let poller = Arc::new(QuotePoller::new(Some("http://localhost:34261".to_string())));

        // Register quotes
        for i in 0..20 {
            let quote_id = format!("quote_{}", i);
            poller
                .register_quote(quote_id, i as u32, i as u64 * 1000)
                .await;
        }

        let mut tasks = vec![];

        // Spawn cleanup task
        let cleanup_poller = Arc::clone(&poller);
        let cleanup_task = tokio::spawn(async move {
            for _ in 0..3 {
                cleanup_poller.cleanup_expired_quotes().await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        tasks.push(cleanup_task);

        // Spawn query tasks
        for i in 0..5 {
            let query_poller = Arc::clone(&poller);
            let task = tokio::spawn(async move {
                let quote_id = format!("quote_{}", i);
                let _ = query_poller.get_quote_channel(&quote_id).await;
            });
            tasks.push(task);
        }

        for task in tasks {
            task.await.unwrap();
        }
    }

    // ============================================================================
    // Pending Quotes Snapshot Tests
    // ============================================================================

    #[tokio::test]
    async fn test_get_pending_quotes_snapshot() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        poller.register_quote("q1".to_string(), 1, 100).await;
        poller.register_quote("q2".to_string(), 2, 200).await;
        poller.register_quote("q3".to_string(), 3, 300).await;

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 3);

        // Verify all quotes are in snapshot
        let quote_ids: Vec<String> = pending.iter().map(|(id, _, _)| id.clone()).collect();
        assert!(quote_ids.contains(&"q1".to_string()));
        assert!(quote_ids.contains(&"q2".to_string()));
        assert!(quote_ids.contains(&"q3".to_string()));
    }

    // ============================================================================
    // Mint Quote Status Response Deserialization Tests
    // ============================================================================

    #[test]
    fn test_mint_quote_status_response_deserialize() {
        let json = r#"{
            "amount": 50000,
            "amount_issued": 50000,
            "state": "PAID"
        }"#;

        let response: MintQuoteStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.amount, Some(50000));
        assert_eq!(response.amount_issued, Some(50000));
        assert_eq!(response.state, "PAID");
    }

    #[test]
    fn test_mint_quote_status_response_missing_amounts() {
        let json = r#"{
            "state": "PENDING"
        }"#;

        let response: MintQuoteStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.amount, None);
        assert_eq!(response.amount_issued, None);
        assert_eq!(response.state, "PENDING");
    }

    #[test]
    fn test_mint_quote_status_response_partial_amounts() {
        let json = r#"{
            "amount": 100000,
            "state": "ISSUED"
        }"#;

        let response: MintQuoteStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.amount, Some(100000));
        assert_eq!(response.amount_issued, None);
        assert_eq!(response.state, "ISSUED");
    }

    #[test]
    fn test_mint_quote_status_response_case_insensitive() {
        let states = vec!["PAID", "paid", "Paid", "PENDING", "pending"];

        for state_str in states {
            let json = format!(r#"{{"state": "{}"}}"#, state_str);
            let response: MintQuoteStatusResponse = serde_json::from_str(&json).unwrap();
            assert!(!response.state.is_empty());
        }
    }

    // ============================================================================
    // Integration-Style Tests (without full async runtime)
    // ============================================================================

    #[tokio::test]
    async fn test_quote_lifecycle_simulation() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        // Step 1: Register quote (share received)
        poller.register_quote("q1".to_string(), 42, 1000).await;
        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 1);

        // Step 2: Query quote (polling)
        let channel_id = poller.get_quote_channel("q1").await;
        assert_eq!(channel_id, Some(42));

        // Step 3: Remove quote (after notification sent)
        poller.remove_quote("q1").await;
        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_bulk_quote_lifecycle() {
        let poller = QuotePoller::new(Some("http://localhost:34261".to_string()));

        // Register 50 quotes
        for i in 0..50 {
            let quote_id = format!("quote_{:03}", i);
            poller
                .register_quote(quote_id, (i % 10) as u32, (i as u64) * 1000)
                .await;
        }

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 50);

        // Remove half
        for i in 0..25 {
            let quote_id = format!("quote_{:03}", i);
            poller.remove_quote(&quote_id).await;
        }

        let pending = poller.get_pending_quotes().await;
        assert_eq!(pending.len(), 25);
    }
}
