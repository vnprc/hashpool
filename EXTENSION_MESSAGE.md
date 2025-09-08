# Mint Quote Distribution via SV2 Extension Messages

## Overview

This document outlines the implementation plan for distributing mint quote IDs to translators using SV2 extension messages. This maintains protocol compliance while enabling ecash token minting.

## Problem Statement

Currently, the pool sends `SubmitSharesSuccess` immediately after share validation, but the mint quote process is asynchronous. This means:
- Translator receives success messages without quote_id needed for minting
- Modifying core SV2 messages breaks protocol compatibility
- Deferring responses causes miner disconnections

## Solution: Custom SV2 Extension Messages

The SV2 protocol includes an extension mechanism that allows vendors to implement custom message types in the reserved range 0xC0-0xFF without breaking protocol compatibility. This is the standard way to add new functionality while maintaining interoperability.

Our approach leverages this extension system to solve the quote distribution problem:

### **Two-Message Flow**:

1. **Send standard `SubmitSharesSuccess` immediately** 
   - Uses unmodified SV2 protocol message (message type 0x1C)
   - Keeps miners connected and satisfied they got a response
   - Contains standard fields: channel_id, sequence_number, hash, etc.
   - **Critical**: This prevents connection timeouts and translator crashes

2. **Follow with custom `MintQuoteNotification` extension message**
   - Uses custom message type 0xC0 (in vendor extension range)
   - Contains quote_id from mint + correlation data (share_hash)
   - Sent asynchronously after mint quote processing completes
   - **Only understood by our translator** - other SV2 implementations ignore it

3. **Translator processes both messages**
   - Handles standard `SubmitSharesSuccess` normally (share accounting)
   - Stores quote_id from `MintQuoteNotification` for ecash minting
   - Correlates via share_hash to match quote with original submission
   - **Enables ecash token creation** when needed

### **Why This Works**:

- **Protocol Compliance**: Extension messages are part of the SV2 specification
- **Backward Compatibility**: Standard messages work with any SV2 implementation
- **No Breaking Changes**: Existing miners and pools continue to work normally
- **Clean Separation**: Share validation (immediate) vs quote processing (async)
- **Reliable Delivery**: Extension messages use existing TCP connection
- **Failure Resilience**: If extension fails, standard share accounting still works

## Architecture Design

### Message Flow

```
1. Miner → Pool: SubmitSharesExtended (with hash, locking_pubkey)
2. Pool → Miner: SubmitSharesSuccess (immediate, standard SV2)
3. Pool → Mint: MintQuoteRequest (async, includes hash)
4. Mint → Pool: MintQuoteResponse (with quote_id)
5. Pool → Translator: MintQuoteNotification (extension message with quote_id + hash)
6. Translator: Correlates quote_id with share hash for ecash minting
```

### Key Benefits

- **Protocol Compliant**: Standard SV2 messages remain unchanged
- **No Connection Drops**: Immediate responses keep miners connected
- **Async Quote Processing**: Mint quotes processed in background
- **Clean Separation**: Quote distribution separate from share validation
- **Backward Compatible**: Miners unaware of extension messages

## Implementation Plan

### **Phase 1: Define Extension Message Types**

#### **1.1 Create Extension Message Protocol**

**File**: `protocols/v2/subprotocols/mining-extensions/src/mint_quote_notification.rs` (new file)
```rust
use binary_sv2::{Deserialize, Serialize, Str0255, U256};
#[cfg(not(feature = "with_serde"))]
use binary_sv2::binary_codec_sv2;
#[cfg(feature = "with_serde")]
use serde;

/// Custom extension message sent from Pool to Translator after mint quote is ready
/// This is sent AFTER the standard SubmitSharesSuccess to provide quote information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintQuoteNotification<'decoder> {
    /// Channel ID this quote is for
    pub channel_id: u32,
    /// Sequence number of the original share submission
    pub sequence_number: u32,
    /// Share hash (matches the hash in SubmitSharesExtended)
    pub share_hash: U256<'decoder>,
    /// Quote ID from the mint for ecash token creation
    #[cfg_attr(feature = "with_serde", serde(borrow))]
    pub quote_id: Str0255<'decoder>,
    /// Optional: Amount of work/difficulty for this share
    pub amount: u64,
}

/// Error notification when mint quote fails
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintQuoteFailure<'decoder> {
    /// Channel ID this failure is for
    pub channel_id: u32,
    /// Sequence number of the original share submission
    pub sequence_number: u32,
    /// Share hash that failed to get a quote
    pub share_hash: U256<'decoder>,
    /// Error message from mint
    #[cfg_attr(feature = "with_serde", serde(borrow))]
    pub error_message: Str0255<'decoder>,
}

#[cfg(feature = "with_serde")]
impl<'d> GetSize for MintQuoteNotification<'d> {
    fn get_size(&self) -> usize {
        self.channel_id.get_size()
            + self.sequence_number.get_size()
            + self.share_hash.get_size()
            + self.quote_id.get_size()
            + self.amount.get_size()
    }
}

#[cfg(feature = "with_serde")]
impl<'d> GetSize for MintQuoteFailure<'d> {
    fn get_size(&self) -> usize {
        self.channel_id.get_size()
            + self.sequence_number.get_size()
            + self.share_hash.get_size()
            + self.error_message.get_size()
    }
}
```

#### **1.2 Define Message Type Constants**

**File**: `protocols/v2/const-sv2/src/lib.rs` (add to existing)
```rust
// Extension message types (vendor range: 0xC0-0xFF)
pub const MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION: u8 = 0xC0;
pub const MESSAGE_TYPE_MINT_QUOTE_FAILURE: u8 = 0xC1;
```

### **Phase 2: Pool Implementation**

#### **2.1 Share Tracking System**

**File**: `roles/pool/src/lib/mining_pool/pending_shares.rs` (new file)
```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct PendingShare {
    pub channel_id: u32,
    pub sequence_number: u32,
    pub share_hash: Vec<u8>,
    pub locking_pubkey: Vec<u8>,
    pub amount: u64,
    pub created_at: Instant,
}

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
        
        let stale: Vec<_> = pending
            .iter()
            .filter(|(_, share)| now.duration_since(share.created_at) > timeout)
            .map(|(hash, share)| (hash.clone(), share.clone()))
            .collect();
            
        for (hash, _) in &stale {
            pending.remove(hash);
        }
        
        stale.into_iter().map(|(_, share)| share).collect()
    }
}
```

#### **2.2 Modified Share Handler**

**File**: `roles/pool/src/lib/mining_pool/message_handler.rs` (modify existing)
```rust
impl ParseDownstreamMiningMessages for Downstream {
    fn handle_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtended<'_>,
    ) -> Result<SendTo<()>, Error> {
        let res = self
            .channel_factory
            .safe_lock(|cf| cf.on_submit_shares_extended(m.clone()))?;
            
        match res {
            Ok(validation_result) => match validation_result {
                OnNewShare::ShareMeetUpstreamTarget |
                OnNewShare::ShareMeetDownstreamTarget => {
                    // 1. Calculate share value
                    let amount = calculate_work(m.hash.inner_as_ref());
                    
                    // 2. Track share for mint quote
                    let share_hash = m.hash.inner_as_ref().to_vec();
                    let pending = PendingShare {
                        channel_id: m.channel_id,
                        sequence_number: m.sequence_number,
                        share_hash: share_hash.clone(),
                        locking_pubkey: m.locking_pubkey.inner_as_ref().to_vec(),
                        amount,
                        created_at: Instant::now(),
                    };
                    
                    let manager = self.pool.safe_lock(|p| p.pending_share_manager.clone())?;
                    let pool_clone = self.pool.clone();
                    let m_static = m.clone().into_static();
                    
                    tokio::spawn(async move {
                        // Add to pending shares
                        manager.add_pending_share(pending).await;
                        
                        // Send mint quote request asynchronously
                        if let Err(e) = send_mint_quote_request(pool_clone, m_static, amount).await {
                            error!("Failed to send mint quote: {}", e);
                        }
                    });
                    
                    // 3. Send IMMEDIATE standard success response (keeps miner connected)
                    let success = SubmitSharesSuccess {
                        channel_id: m.channel_id,
                        last_sequence_number: m.sequence_number,
                        new_submits_accepted_count: 1,
                        new_shares_sum: amount,
                        hash: m.hash.clone(),
                    };
                    
                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
                }
                // ... handle other cases
            }
        }
    }
}
```

#### **2.3 Mint Quote Response Handler**

**File**: `roles/pool/src/lib/mining_pool/mint_quote_handler.rs` (new file)
```rust
use mining_extensions::{MintQuoteNotification, MintQuoteFailure};

impl Pool {
    /// Handle mint quote response from mint
    pub async fn handle_mint_quote_response(
        pool: Arc<Mutex<Pool>>,
        quote_id: String,
        header_hash: Vec<u8>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Find pending share by hash
        let manager = pool.safe_lock(|p| p.pending_share_manager.clone())?;
        let pending = manager.remove_pending_share(&header_hash).await;
        
        if let Some(share) = pending {
            // Create extension message with quote info
            let notification = MintQuoteNotification {
                channel_id: share.channel_id,
                sequence_number: share.sequence_number,
                share_hash: header_hash.try_into()?,
                quote_id: quote_id.try_into()?,
                amount: share.amount,
            };
            
            // Send extension message to downstream
            Self::send_extension_message(pool, share.channel_id, notification).await?;
            
            info!("Sent mint quote notification for channel {} seq {}", 
                  share.channel_id, share.sequence_number);
        } else {
            warn!("Received mint quote for unknown share: {}", hex::encode(&header_hash));
        }
        
        Ok(())
    }
    
    /// Send extension message to specific downstream
    async fn send_extension_message<M>(
        pool: Arc<Mutex<Pool>>,
        channel_id: u32,
        message: M,
    ) -> Result<(), Box<dyn std::error::Error>> 
    where M: Serialize + GetSize 
    {
        let downstream = pool.safe_lock(|p| p.downstreams.get(&channel_id).cloned())?
            .ok_or("Downstream not found")?;
            
        // Create SV2 frame with extension message
        let frame = StandardSv2Frame::from_message(
            message,
            M::message_type(),
            SV2_EXTENSION_TYPE,
            true, // channel message
        )?;
        
        // Send via downstream connection
        downstream.safe_lock(|d| d.sender.try_send(frame.into()))?;
        
        Ok(())
    }
}
```

### **Phase 3: Translator Implementation**

#### **3.1 Extension Message Handler**

**File**: `roles/translator/src/lib/upstream_sv2/extension_handler.rs` (new file)
```rust
use mining_extensions::{MintQuoteNotification, MintQuoteFailure};
use std::collections::HashMap;
use tokio::sync::Mutex;

pub struct QuoteTracker {
    // Map share_hash -> quote_id for ecash minting
    quotes: Arc<Mutex<HashMap<Vec<u8>, String>>>,
}

impl QuoteTracker {
    pub fn new() -> Self {
        Self {
            quotes: Arc::new(Mutex::new(HashMap::new()))
        }
    }
    
    pub async fn store_quote(&self, share_hash: Vec<u8>, quote_id: String) {
        let mut quotes = self.quotes.lock().await;
        quotes.insert(share_hash, quote_id);
        
        // Clean old entries if map gets too large
        if quotes.len() > 10000 {
            // Keep only recent 5000 entries (simple FIFO)
            let to_remove: Vec<_> = quotes.keys()
                .take(5000)
                .cloned()
                .collect();
            for key in to_remove {
                quotes.remove(&key);
            }
        }
    }
    
    pub async fn get_quote(&self, share_hash: &[u8]) -> Option<String> {
        let quotes = self.quotes.lock().await;
        quotes.get(share_hash).cloned()
    }
}

impl Upstream {
    /// Handle extension messages from pool
    pub fn handle_extension_message(
        &mut self,
        message_type: u8,
        payload: &[u8],
    ) -> Result<(), Error> {
        match message_type {
            MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION => {
                let notification: MintQuoteNotification = 
                    binary_sv2::from_bytes(payload)?;
                
                let share_hash = notification.share_hash.to_vec();
                let quote_id = String::from_utf8_lossy(
                    notification.quote_id.as_ref()
                ).to_string();
                
                info!("Received mint quote {} for share {}", 
                      quote_id, hex::encode(&share_hash));
                
                // Store quote for later ecash minting
                let tracker = self.quote_tracker.clone();
                tokio::spawn(async move {
                    tracker.store_quote(share_hash, quote_id).await;
                });
                
                Ok(())
            }
            MESSAGE_TYPE_MINT_QUOTE_FAILURE => {
                let failure: MintQuoteFailure = 
                    binary_sv2::from_bytes(payload)?;
                
                warn!("Mint quote failed for share {}: {}", 
                      hex::encode(failure.share_hash.to_vec()),
                      String::from_utf8_lossy(failure.error_message.as_ref()));
                
                Ok(())
            }
            _ => {
                debug!("Unknown extension message type: 0x{:02x}", message_type);
                Ok(())
            }
        }
    }
}
```

#### **3.2 Modified Share Success Handler**

**File**: `roles/translator/src/lib/upstream_sv2/upstream.rs` (modify existing)
```rust
impl ParseUpstreamMiningMessages for Upstream {
    fn handle_submit_shares_success(
        &mut self,
        m: SubmitSharesSuccess,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        let share_hash = m.hash.to_vec();
        let amount = calculate_work(m.hash.inner_as_ref());
        
        info!("Share accepted: hash={}, amount={}", 
              hex::encode(&share_hash), amount);
        
        // Quote will arrive later via extension message
        // For now, just acknowledge the success
        
        // Later, when minting ecash:
        // let quote_id = self.quote_tracker.get_quote(&share_hash).await;
        // if let Some(quote_id) = quote_id {
        //     self.wallet.mint_tokens(quote_id, amount).await?;
        // }
        
        Ok(SendTo::None(None))
    }
}
```

### **Phase 4: Testing and Deployment**

#### **4.1 Integration Testing**

1. **Unit Tests**: Test each component independently
   - Extension message serialization/deserialization
   - PendingShareManager operations
   - QuoteTracker storage and retrieval

2. **Integration Tests**: 
   - Full flow from share submission to quote notification
   - Timeout handling for stale shares
   - Error cases (mint failures, disconnections)

3. **Load Testing**:
   - High volume share submissions
   - Memory usage of pending share tracking
   - Extension message delivery performance

#### **4.2 Deployment Steps**

1. **Deploy Protocol Changes**:
   - Update protocols with extension messages
   - Ensure backward compatibility

2. **Deploy Pool**:
   - Add PendingShareManager
   - Enable extension message sending
   - Monitor for memory leaks

3. **Deploy Translator**:
   - Add QuoteTracker
   - Enable extension message handling
   - Test ecash minting integration

## Benefits of This Approach

### **Technical Benefits**

- **Protocol Compliant**: Uses SV2 extension mechanism properly
- **No Breaking Changes**: Standard messages unchanged
- **Clean Separation**: Quote distribution separate from validation
- **Async Processing**: Non-blocking mint quote handling
- **Reliable Delivery**: Extension messages follow established connection

### **Operational Benefits**

- **Gradual Rollout**: Can deploy pool first, translator later
- **Monitoring**: Clear visibility into quote distribution
- **Debugging**: Extension messages easily logged/traced
- **Performance**: No impact on share validation speed

## Potential Issues and Mitigations

### **Issue 1: Translator Disconnection**
**Problem**: Translator disconnects before receiving extension message
**Mitigation**: 
- PendingShareManager tracks undelivered quotes
- Resend on reconnection or store in persistent queue

### **Issue 2: Memory Growth**
**Problem**: PendingShareManager grows unbounded
**Mitigation**:
- Periodic cleanup of stale shares (>30s old)
- Max size limits with FIFO eviction
- Move to Redis for persistence if needed

### **Issue 3: Extension Message Support**
**Problem**: Not all SV2 implementations support extensions
**Mitigation**:
- Extension messages are optional
- Fallback to side-channel (Redis) if needed
- Detect capability during handshake

## Conclusion

This extension message approach provides a clean, protocol-compliant solution for distributing mint quote IDs to translators without breaking existing SV2 implementations. The immediate standard responses keep miners connected while asynchronous processing handles mint integration efficiently.

