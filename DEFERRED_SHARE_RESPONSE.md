# Deferred Share Response Implementation Plan

## Overview

This document outlines the implementation plan for delayed `SubmitSharesSuccess` responses that wait for mint quote completion before responding to downstream miners. This solves the keyset distribution problem by ensuring quote_id and keyset_id are available in share success messages.

## Problem Statement

Currently, the pool sends `SubmitSharesSuccess` immediately after share validation, but the mint quote process is asynchronous. This means:
- Translator receives success messages without quote_id/keyset_id
- Wallet cannot mint ecash tokens due to missing keyset information
- Share accounting and ecash token creation are disconnected

## Solution: SV2-Compliant Delayed Responses

The SV2 specification explicitly supports:
1. **Batching**: `SubmitSharesSuccess` can aggregate multiple shares
2. **Delayed validation**: "In case the upstream is not able to immediately validate the submission, the error is sent as soon as the result is known"
3. **Deferred handling**: The `SendTo::None(Option<Message>)` enum variant allows deferring responses

## Implementation Plan

### **Phase 1: Deferred Response Architecture** ✅ **COMPLETED**

#### **1.1 Create PendingShare Tracking System**

**File**: `roles/pool/src/lib/mining_pool/pending_shares.rs` (new file)
```rust
use tokio::time::Instant;
use mining_sv2::{SubmitSharesExtended, SubmitSharesSuccess, SubmitSharesError};
use std::collections::HashMap;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct PendingShare {
    pub channel_id: u32,
    pub sequence_number: u32,
    pub share_hash: Vec<u8>,
    pub share_data: SubmitSharesExtended<'static>,
    pub created_at: Instant,
}

#[derive(Debug)]
pub struct PendingShareManager {
    pending: Arc<Mutex<HashMap<Vec<u8>, PendingShare>>>,
}

impl PendingShareManager {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new()))
        }
    }
    
    pub async fn add_pending_share(&self, share: PendingShare) {
        let mut pending = self.pending.lock().await;
        pending.insert(share.share_hash.clone(), share);
    }
    
    pub async fn remove_pending_share(&self, hash: &[u8]) -> Option<PendingShare> {
        let mut pending = self.pending.lock().await;
        pending.remove(hash)
    }
    
    pub async fn get_stale_shares(&self, timeout: Duration) -> Vec<PendingShare> {
        let mut pending = self.pending.lock().await;
        let now = Instant::now();
        
        let stale_hashes: Vec<Vec<u8>> = pending
            .iter()
            .filter(|(_, share)| now.duration_since(share.created_at) > timeout)
            .map(|(hash, _)| hash.clone())
            .collect();
            
        stale_hashes
            .into_iter()
            .filter_map(|hash| pending.remove(&hash))
            .collect()
    }
}
```

#### **1.2 Extend Pool Structure** ✅ **COMPLETED**

**File**: `roles/pool/src/lib/mining_pool/mod.rs`
```rust
// ✅ IMPLEMENTED: PendingShareManager added to Pool structure
pub struct Pool {
    // ... existing fields  
    pending_share_manager: PendingShareManager, // ✅ Added and initialized
    // Note: Phase 1 uses direct SendTo::None, broadcast system for Phase 2+
}

impl Pool {
    pub fn new(/* existing params */) -> Self {
        let (delayed_tx, delayed_rx) = async_channel::bounded(1000);
        let pool_arc = Arc::new(Mutex::new(Pool {
            // ... existing initialization
            pending_share_manager: PendingShareManager::new(),
            downstream_connections: Arc::new(Mutex::new(Vec::new())),
            delayed_response_sender: Some(delayed_tx),
        }));
        
        // Spawn background task for delayed responses
        let pool_weak = Arc::downgrade(&pool_arc);
        tokio::spawn(async move {
            if let Some(pool_strong) = pool_weak.upgrade() {
                Self::handle_delayed_responses(delayed_rx, pool_strong).await;
            }
        });
        
        Arc::try_unwrap(pool_arc).unwrap().into_inner()
    }
    
    // PHASE 1/2: Connection management for broadcast approach
    pub fn register_downstream_connection(&self, sender: DownstreamConnectionSender) {
        self.downstream_connections.lock().unwrap().push(sender);
    }
    
    pub fn get_all_downstream_connections(&self) -> Result<Vec<DownstreamConnectionSender>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.downstream_connections.lock()
            .map_err(|e| format!("Failed to lock downstream connections: {}", e))?
            .clone())
    }
    
    pub fn remove_downstream_connection(&self, target_sender: &DownstreamConnectionSender) {
        let mut connections = self.downstream_connections.lock().unwrap();
        // Remove by comparing sender addresses (approximate)
        connections.retain(|sender| !Arc::ptr_eq(&sender.sender, &target_sender.sender));
    }
}
```

#### **1.3 Modify Share Handler** ✅ **COMPLETED**

**File**: `roles/pool/src/lib/mining_pool/message_handler.rs`
```rust
impl ParseDownstreamMiningMessages<(), NullDownstreamMiningSelector, NoRouting> for Downstream {
    fn handle_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtended<'_>,
    ) -> Result<SendTo<()>, Error> {
        let res = self
            .channel_factory
            .safe_lock(|cf| cf.on_submit_shares_extended(m.clone()))
            .map_err(|e| roles_logic_sv2::Error::PoisonLock(e.to_string()))?;
            
        match res {
            Ok(validation_result) => match validation_result {
                OnNewShare::ShareMeetDownstreamTarget => {
                    // Instead of immediate response, defer processing
                    let share_hash = m.hash.inner_as_ref().to_vec();
                    let pending = PendingShare {
                        channel_id: m.channel_id,
                        sequence_number: m.sequence_number,
                        share_hash: share_hash.clone(),
                        share_data: m.into_static(),
                        created_at: Instant::now(),
                    };
                    
                    // Store pending share
                    let manager = self.pool.safe_lock(|p| p.pending_share_manager.clone())?;
                    tokio::spawn(async move {
                        manager.add_pending_share(pending).await;
                    });
                    
                    // Spawn async task to process with mint
                    let pool_clone = self.pool.clone();
                    let m_static = m.into_static();
                    tokio::spawn(async move {
                        Self::process_share_with_mint_quote(pool_clone, m_static).await;
                    });
                    
                    // Defer response - don't send immediately
                    Ok(SendTo::None(None))
                }
                // ... handle other validation results normally
            }
            Err(e) => {
                // Handle validation errors immediately
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: format!("validation-failed: {}", e).try_into()
                        .unwrap_or_else(|_| "validation-failed".try_into().unwrap()),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
        }
    }
}
```

#### **Phase 1 Status: ✅ COMPLETED**

**Implemented Components:**
- ✅ **PendingShareManager**: Full implementation with async methods, stale cleanup, unit tests
- ✅ **Pool Integration**: Added pending_share_manager field to Pool struct  
- ✅ **Deferred Responses**: Modified share handlers to return `SendTo::None(None)`
- ✅ **Share Tracking**: All valid shares added to pending manager before deferring
- ✅ **Background Processing**: Mint quote requests still sent asynchronously
- ✅ **Unit Tests**: 3/3 tests passing for PendingShareManager
- ✅ **Compilation**: Pool and mint both build successfully

**Result**: Pool now defers share responses and tracks pending shares. Ready for Phase 2 integration.

---

### **Phase 2: Mint Quote Integration with Proper Error Handling**

#### **2.1 Async Share Processing**

**File**: `roles/pool/src/lib/mining_pool/mint_integration.rs` (new file)
```rust
use super::{Pool, PendingShare};
use mining_sv2::{SubmitSharesSuccess, SubmitSharesError};
use mint_quote_sv2::{MintQuoteRequest, MintQuoteResponse};

impl Pool {
    pub async fn process_share_with_mint_quote(
        pool: Arc<Mutex<Self>>,
        share: SubmitSharesExtended<'static>,
    ) {
        let share_hash = share.hash.inner_as_ref().to_vec();
        
        // 1. Send mint quote request and wait for response
        let mint_result = Self::send_mint_quote_and_wait(pool.clone(), &share).await;
        
        // 2. Remove from pending shares
        let pending = {
            let manager = pool.safe_lock(|p| p.pending_share_manager.clone())
                .expect("Failed to get pending share manager");
            manager.remove_pending_share(&share_hash).await
        };
        
        if let Some(pending) = pending {
            match mint_result {
                Ok((quote_id, keyset_id)) => {
                    // Mint accepted - send SUCCESS with quote info
                    let success_message = SubmitSharesSuccess {
                        channel_id: pending.channel_id,
                        last_sequence_number: pending.sequence_number,
                        new_submits_accepted_count: 1,
                        new_shares_sum: Self::calculate_share_value(&share),
                        hash: share.hash,
                        quote_id: quote_id.try_into()
                            .expect("Invalid quote ID format"),
                        keyset_id: keyset_id.try_into()
                            .expect("Invalid keyset ID format"),
                    };
                    Self::send_delayed_response(
                        pool,
                        Mining::SubmitSharesSuccess(success_message)
                    ).await;
                }
                Err(mint_error) => {
                    // Mint rejected - send ERROR (share rejected)
                    tracing::warn!("Mint rejected share {}: {}", 
                                 hex::encode(&share_hash), mint_error);
                    
                    let error_message = SubmitSharesError {
                        channel_id: pending.channel_id,
                        sequence_number: pending.sequence_number,
                        error_code: format!("mint-quote-failed: {}", mint_error)
                            .try_into()
                            .unwrap_or_else(|_| "mint-quote-failed".try_into().unwrap()),
                    };
                    Self::send_delayed_response(
                        pool,
                        Mining::SubmitSharesError(error_message)
                    ).await;
                }
            }
        } else {
            tracing::warn!("Processed mint quote for unknown share: {}", 
                         hex::encode(&share_hash));
        }
    }
    
    async fn send_mint_quote_and_wait(
        pool: Arc<Mutex<Self>>,
        share: &SubmitSharesExtended<'static>
    ) -> Result<(String, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
        // Create mint quote request
        let amount = Self::calculate_share_value(share);
        let request = Self::create_mint_quote_request(share, amount)?;
        
        // Get mint connection
        let mint_sender = {
            let mint_connection = pool.safe_lock(|p| p.get_mint_connection())?;
            mint_connection.ok_or("No active mint connection")?
        };
        
        // Send request
        let pool_message = roles_logic_sv2::parsers::PoolMessages::Minting(
            roles_logic_sv2::parsers::Minting::MintQuoteRequest(request.into_static())
        );
        let sv2_frame: StdFrame = pool_message.try_into()?;
        mint_sender.send(sv2_frame.into()).await?;
        
        // Wait for response with timeout
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            Self::wait_for_mint_response(pool, &share.hash.inner_as_ref().to_vec())
        ).await??;
        
        // Extract quote_id and keyset_id
        let quote_id = std::str::from_utf8(response.quote_id.inner_as_ref())?.to_string();
        let keyset_id = response.keyset_id.inner_as_ref().to_vec();
        
        Ok((quote_id, keyset_id))
    }
    
    fn calculate_share_value(share: &SubmitSharesExtended<'static>) -> u64 {
        // Calculate work/difficulty from share
        let header_hash = bitcoin_hashes::sha256::Hash::from_slice(share.hash.inner_as_ref())
            .expect("Invalid header hash");
        calculate_work(header_hash.to_byte_array())
    }
}
```

#### **2.2 Response Channel Management**

**File**: `roles/pool/src/lib/mining_pool/response_handler.rs` (new file)
```rust
impl Pool {
    async fn send_delayed_response(
        pool: Arc<Mutex<Self>>,
        response: Mining<'static>
    ) {
        let sender = pool.safe_lock(|p| p.delayed_response_sender.clone())
            .expect("Failed to get delayed response sender");
            
        if let Some(sender) = sender {
            if let Err(e) = sender.send(response).await {
                tracing::error!("Failed to send delayed response: {}", e);
            }
        }
    }
    
    async fn handle_delayed_responses(
        mut receiver: async_channel::Receiver<Mining<'static>>,
        pool: Arc<Mutex<Self>>
    ) {
        while let Ok(response) = receiver.recv().await {
            // PHASE 2: Basic implementation - send to ALL downstream connections
            // This is inefficient but ensures responses are delivered
            match Self::broadcast_to_all_downstreams(pool.clone(), response.clone()).await {
                Ok(sent_count) => {
                    tracing::debug!("Broadcasted delayed response to {} downstreams", sent_count);
                }
                Err(e) => {
                    tracing::error!("Failed to broadcast delayed response: {}", e);
                }
            }
        }
    }
    
    // PHASE 2: Broadcast to all connections (inefficient but functional)
    async fn broadcast_to_all_downstreams(
        pool: Arc<Mutex<Self>>,
        response: Mining<'static>
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let downstream_connections = pool.safe_lock(|p| p.get_all_downstream_connections())??;
        let mut sent_count = 0;
        
        for connection in downstream_connections {
            if let Err(e) = connection.send(response.clone()).await {
                tracing::warn!("Failed to send to downstream: {}", e);
                // Connection might be dead, should be cleaned up
            } else {
                sent_count += 1;
            }
        }
        
        Ok(sent_count)
    }
}
```

### **Phase 3: Enhanced Batching Support**

#### **3.1 Batch Processing**

**File**: `roles/pool/src/lib/mining_pool/batch_processor.rs` (new file)
```rust
impl Pool {
    pub async fn start_batch_processor(
        pool: Arc<Mutex<Self>>,
    ) {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        
        loop {
            interval.tick().await;
            Self::process_pending_batches(pool.clone()).await;
        }
    }
    
    async fn process_pending_batches(pool: Arc<Mutex<Self>>) {
        let manager = pool.safe_lock(|p| p.pending_share_manager.clone())
            .expect("Failed to get pending share manager");
            
        // Check for shares ready to batch (same channel, recent submissions)
        let batchable_shares = Self::find_batchable_shares(&manager).await;
        
        if batchable_shares.len() > 1 {
            Self::send_batched_success(pool, batchable_shares).await;
        }
    }
    
    async fn send_batched_success(
        pool: Arc<Mutex<Self>>,
        shares: Vec<PendingShare>
    ) {
        if shares.is_empty() { return; }
        
        let batched_success = SubmitSharesSuccess {
            channel_id: shares[0].channel_id,
            last_sequence_number: shares.iter()
                .map(|s| s.sequence_number)
                .max()
                .unwrap(),
            new_submits_accepted_count: shares.len() as u32,
            new_shares_sum: shares.iter()
                .map(|s| Self::calculate_share_value(&s.share_data))
                .sum(),
            // For batching, use the most recent share's hash
            hash: shares.last().unwrap().share_data.hash,
            // Batching with multiple quote_ids requires protocol extension
            quote_id: Str0255::from("BATCHED"), 
            keyset_id: U256::from([0u8; 32]), // Use current active keyset
        };
        
        Self::send_delayed_response(
            pool, 
            Mining::SubmitSharesSuccess(batched_success)
        ).await;
    }
}
```

### **Phase 4: Targeted Response Optimization (Replace Broadcast)**

#### **4.1 Enhanced Connection Management**

**File**: `roles/pool/src/lib/mining_pool/connection_manager.rs` (new file)
```rust
use std::collections::HashMap;
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub struct DownstreamConnection {
    pub channel_id: u32,
    pub connection_sender: async_channel::Sender<Mining<'static>>,
    pub last_activity: Instant,
    pub connection_info: String, // For debugging
}

pub struct ConnectionManager {
    connections: Arc<Mutex<HashMap<u32, DownstreamConnection>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new()))
        }
    }
    
    pub async fn register_connection(&self, connection: DownstreamConnection) {
        let mut connections = self.connections.lock().await;
        tracing::info!("Registered downstream connection: channel_id={}", connection.channel_id);
        connections.insert(connection.channel_id, connection);
    }
    
    // PHASE 4: Replace broadcast with targeted sends
    pub async fn send_to_downstream(
        &self, 
        channel_id: u32, 
        message: Mining<'static>
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let connection = {
            let connections = self.connections.lock().await;
            connections.get(&channel_id).cloned()
        };
        
        if let Some(conn) = connection {
            conn.connection_sender.send(message).await?;
            tracing::debug!("Sent targeted response to channel_id={}", channel_id);
            Ok(())
        } else {
            Err(format!("Downstream connection {} not found", channel_id).into())
        }
    }
    
    pub async fn cleanup_stale_connections(&self) {
        let mut connections = self.connections.lock().await;
        let now = Instant::now();
        
        connections.retain(|channel_id, conn| {
            let is_active = now.duration_since(conn.last_activity) < Duration::from_secs(300);
            if !is_active {
                tracing::info!("Cleaning up stale connection: {}", channel_id);
            }
            is_active
        });
    }
    
    pub async fn update_activity(&self, channel_id: u32) {
        if let Ok(mut connections) = self.connections.lock() {
            if let Some(conn) = connections.get_mut(&channel_id) {
                conn.last_activity = Instant::now();
            }
        }
    }
}
```

#### **4.2 Timeout and Cleanup**

**File**: `roles/pool/src/lib/mining_pool/cleanup.rs` (new file)
```rust
impl Pool {
    pub async fn start_cleanup_tasks(pool: Arc<Mutex<Self>>) {
        // Cleanup stale pending shares
        let pool_clone = pool.clone();
        tokio::spawn(async move {
            Self::cleanup_stale_pending_shares(pool_clone).await;
        });
        
        // Cleanup stale connections  
        let pool_clone = pool.clone();
        tokio::spawn(async move {
            Self::cleanup_stale_connections(pool_clone).await;
        });
    }
    
    async fn cleanup_stale_pending_shares(pool: Arc<Mutex<Self>>) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        
        loop {
            interval.tick().await;
            
            let manager = pool.safe_lock(|p| p.pending_share_manager.clone())
                .expect("Failed to get pending share manager");
                
            let stale_shares = manager.get_stale_shares(Duration::from_secs(10)).await;
            
            for stale_share in stale_shares {
                tracing::warn!("Share {} timed out waiting for mint response", 
                             hex::encode(&stale_share.share_hash));
                
                let timeout_error = SubmitSharesError {
                    channel_id: stale_share.channel_id,
                    sequence_number: stale_share.sequence_number,
                    error_code: "mint-timeout".try_into().unwrap(),
                };
                
                Self::send_delayed_response(
                    pool.clone(),
                    Mining::SubmitSharesError(timeout_error)
                ).await;
            }
        }
    }
    
    pub async fn shutdown(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("Pool shutting down...");
        
        // 1. Stop accepting new shares (set shutdown flag)
        
        // 2. Wait for pending shares with timeout
        let pending_count = self.pending_share_manager.pending.lock().await.len();
        if pending_count > 0 {
            tracing::info!("Waiting for {} pending shares to complete", pending_count);
            
            let timeout = Duration::from_secs(30);
            let start = Instant::now();
            
            while start.elapsed() < timeout {
                let remaining = self.pending_share_manager.pending.lock().await.len();
                if remaining == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        
        // 3. Send errors for remaining pending shares
        let remaining_shares = self.pending_share_manager.pending.lock().await.drain().collect::<Vec<_>>();
        for (_, share) in remaining_shares {
            let shutdown_error = SubmitSharesError {
                channel_id: share.channel_id,
                sequence_number: share.sequence_number,
                error_code: "pool-shutdown".try_into().unwrap(),
            };
            // Send to downstream
        }
        
        tracing::info!("Pool shutdown complete");
        Ok(())
    }
}
```

#### **4.3 Monitoring and Observability**

**File**: `roles/pool/src/lib/mining_pool/metrics.rs` (new file)
```rust
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct PoolMetrics {
    pub pending_shares_count: AtomicUsize,
    pub mint_quote_success_total: AtomicU64,
    pub mint_quote_error_total: AtomicU64,
    pub delayed_responses_sent: AtomicU64,
    pub batch_responses_sent: AtomicU64,
    pub timeout_errors_sent: AtomicU64,
}

impl PoolMetrics {
    pub fn mint_quote_success_rate(&self) -> f64 {
        let success = self.mint_quote_success_total.load(Ordering::Relaxed) as f64;
        let total = success + self.mint_quote_error_total.load(Ordering::Relaxed) as f64;
        
        if total > 0.0 {
            success / total
        } else {
            0.0
        }
    }
    
    pub fn record_mint_quote_success(&self) {
        self.mint_quote_success_total.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_mint_quote_error(&self) {
        self.mint_quote_error_total.fetch_add(1, Ordering::Relaxed);
    }
}

impl Pool {
    pub fn get_metrics(&self) -> &PoolMetrics {
        &self.metrics
    }
    
    async fn log_periodic_stats(pool: Arc<Mutex<Self>>) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        
        loop {
            interval.tick().await;
            
            let metrics = pool.safe_lock(|p| p.get_metrics())
                .expect("Failed to get pool metrics");
                
            tracing::info!(
                "Pool stats: pending={}, success_rate={:.2}%, delayed_responses={}, timeouts={}",
                metrics.pending_shares_count.load(Ordering::Relaxed),
                metrics.mint_quote_success_rate() * 100.0,
                metrics.delayed_responses_sent.load(Ordering::Relaxed),
                metrics.timeout_errors_sent.load(Ordering::Relaxed)
            );
        }
    }
}
```

## **Updated Pool Structure Evolution**

### **Phase 1/2: Broadcast Implementation**
**File**: `roles/pool/src/lib/mining_pool/mod.rs`
```rust
pub struct Pool {
    // ... existing fields
    pending_share_manager: PendingShareManager,
    // PHASE 1/2: Simple broadcast approach
    downstream_connections: Arc<Mutex<Vec<DownstreamConnectionSender>>>,
    delayed_response_sender: Option<async_channel::Sender<Mining<'static>>>,
    metrics: PoolMetrics,
}

impl Pool {
    pub fn new(/* existing params */) -> Self {
        let (delayed_tx, delayed_rx) = async_channel::bounded(1000);
        let pool = Pool {
            // ... existing initialization
            pending_share_manager: PendingShareManager::new(),
            downstream_connections: Arc::new(Mutex::new(Vec::new())),
            delayed_response_sender: Some(delayed_tx.clone()),
            metrics: PoolMetrics::default(),
        };
        
        // Start basic background tasks
        let pool_arc = Arc::new(Mutex::new(pool));
        
        tokio::spawn(Self::handle_delayed_responses(delayed_rx, pool_arc.clone()));
        tokio::spawn(Self::start_cleanup_tasks(pool_arc.clone()));
        
        Arc::try_unwrap(pool_arc).unwrap().into_inner()
    }
}
```

### **Phase 4: Optimized Implementation** 
**File**: `roles/pool/src/lib/mining_pool/mod.rs`
```rust
pub struct Pool {
    // ... existing fields
    pending_share_manager: PendingShareManager,
    // PHASE 4: Replace Vec<Sender> with ConnectionManager
    connection_manager: ConnectionManager,
    delayed_response_sender: Option<async_channel::Sender<Mining<'static>>>,
    metrics: PoolMetrics,
    shutdown_flag: Arc<AtomicBool>,
}

impl Pool {
    // PHASE 4: Upgrade handle_delayed_responses to use targeted routing
    async fn handle_delayed_responses_targeted(
        mut receiver: async_channel::Receiver<Mining<'static>>,
        pool: Arc<Mutex<Self>>
    ) {
        while let Ok(response) = receiver.recv().await {
            // Extract channel_id from response to route correctly
            let channel_id = Self::extract_channel_id_from_response(&response);
            
            let connection_manager = pool.safe_lock(|p| p.connection_manager.clone())
                .expect("Failed to get connection manager");
                
            match connection_manager.send_to_downstream(channel_id, response).await {
                Ok(_) => {
                    tracing::debug!("Successfully sent targeted delayed response");
                }
                Err(e) => {
                    tracing::error!("Failed to send targeted delayed response: {}", e);
                    // Could fallback to broadcast for reliability
                }
            }
        }
    }
    
    fn extract_channel_id_from_response(response: &Mining<'static>) -> u32 {
        match response {
            Mining::SubmitSharesSuccess(success) => success.channel_id,
            Mining::SubmitSharesError(error) => error.channel_id,
            _ => 0, // Default channel
        }
    }
}
```

## **Implementation Order (Updated)**

1. **Phase 1**: Basic deferred response architecture
   - Implement `PendingShareManager`
   - Modify `handle_submit_shares_extended` to return `SendTo::None`
   - Add simple downstream connection tracking (Vec<Sender>)
   - Add basic delayed response channel

2. **Phase 2**: Mint integration with **working** broadcast delivery
   - Implement async mint quote processing
   - Handle success → `SubmitSharesSuccess` with quote_id/keyset_id
   - Handle error → `SubmitSharesError` (reject share)
   - **Key Fix**: Implement broadcast delivery to ensure responses reach miners
   - **Limitation**: Only suitable for single-miner testing

3. **Phase 3**: Batching optimization (optional)
   - Implement batch detection and processing
   - Add batched success message handling
   - Still uses broadcast delivery

4. **Phase 4**: Targeted routing optimization (**critical for multi-miner**)
   - Replace Vec<Sender> with ConnectionManager
   - Replace broadcast with targeted channel_id routing
   - Add enhanced connection lifecycle management
   - Add cleanup and timeout handling
   - Add metrics and monitoring
   - Implement graceful shutdown

## **Key Benefits (Updated)**

- **Phase 1+2**: **Working MVP** for single-miner setups
- **SV2 Compliant**: Uses official delayed validation capabilities
- **Proper Error Handling**: Mint failures result in share rejection
- **Complete Quote Information**: Success messages include quote_id and keyset_id
- **Incremental Deployment**: Can deploy Phase 2 for testing, upgrade to Phase 4 for production
- **Maintains Performance**: Non-blocking share validation
- **Phase 4**: **Production Ready** for multi-miner pools

## **Testing Strategy (Updated)**

### **Phase 2 Testing (Single Miner)**
1. **Unit Tests**: Test each phase component independently
2. **Integration Tests**: Test complete mint quote flow with one miner
3. **Error Scenarios**: Test mint failures, timeouts with broadcast delivery
4. **Basic Functionality**: Verify quote_id/keyset_id reach translator

### **Phase 4 Testing (Multi-Miner Production)**
1. **Multi-Miner Integration**: Test with multiple concurrent miners
2. **Targeted Routing**: Verify responses reach correct miner only
3. **Load Testing**: Test with high share submission rates from multiple miners
4. **Connection Lifecycle**: Test miner disconnection/reconnection scenarios
5. **Failover Testing**: Test mint unavailability with proper error routing

## **Migration Path**

- **Deploy Phase 2**: Single-miner testing and validation
- **Validate Functionality**: Confirm quote_id/keyset_id flow works
- **Upgrade to Phase 4**: Multi-miner production deployment
- **Monitor and Optimize**: Use metrics to tune performance

This approach provides a **working solution faster** while maintaining a clear upgrade path to full production capabilities.