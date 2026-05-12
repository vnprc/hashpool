# **MINT SERVICE INTEGRATION PLAN FOR SRI 1.5.0**

## **Implementation Status**

- ✅ **PHASE 1**: Add PoolMessages & MintQuoteNotification - **COMPLETED (Oct 26)**
- ✅ **PHASE 2**: Proper Noise Handshake - **COMPLETED (Oct 27)**
  - Pool responder: Listens for mint connections with Noise encryption ✅
  - Mint initiator: Connects to pool with Noise encryption ✅
  - SetupConnection handshake: Both sides implemented ✅
  - Message handler: Updated to handle setup responses and mint quote messages ✅
- ✅ **PHASE 3**: Quote Request/Response Flow - **COMPLETED (Oct 27)**
  - Quote poller: HTTP polling of mint paid quotes endpoint every 5s ✅
  - Quote tracking: Pending quotes mapped to channels for routing ✅
  - Notification delivery: MintQuoteNotification extension messages to translators ✅

## **Overview**

This document describes the three-phase plan to get the mint service running with proper SV2 Noise encryption and end-to-end ehash token issuance on SRI 1.5.0.

**Objective**: Enable the ehash smoke test to pass by implementing proper pool ↔ mint ↔ translator communication.

---

## **Architecture: Correct Flow**

### **Share Submission → Quote Creation**

```
Share accepted by pool
  ↓
Pool sends MintQuoteRequest to Mint (fire-and-forget via TCP)
  ├─ Request contains: amount, unit="HASH", header_hash, locking_key
  ├─ Pool does NOT wait for response
  └─ Mint processes independently

Mint Quote Lifecycle:
  ├─ Receives MintQuoteRequest
  ├─ Creates quote in CDK database
  ├─ Sets status based on pool specification:
  │   ├─ PENDING: awaiting block template validation
  │   └─ PAID: immediately (Phase 1) or after block proof (Phase 2+)
  └─ Quote ready for claiming
```

### **Quote Notification → Token Delivery**

```
Pool periodically checks "what quotes are PAID now?"
  ├─ Via HTTP API call to mint: GET /quotes?status=paid
  ├─ Gets back list of paid quote_ids with amounts
  └─ Every ~5 seconds

Pool sends SV2 Extension Message to Translator:
  ├─ Message type: MintQuoteNotification (new extension message)
  ├─ Sent via existing mining protocol connection
  ├─ Contains: quote_id, amount, (optional: status)
  └─ Targets correct downstream/channel

Translator receives extension message:
  ├─ Parses MintQuoteNotification
  ├─ Calls mint HTTP API: POST /mint/quotes/{quote_id}/claim
  ├─ Receives complete Cashu token
  ├─ Stores in wallet database
  └─ User can now redeem the ehash token
```

---

## **PHASE 1: Add PoolMessages & MintQuoteNotification (2 hours)**

### **Goal**: Get code compiling, establish protocol infrastructure for extension messages

### **1.1 Add MintQuoteNotification to Mining Protocol**

**File**: `protocols/v2/subprotocols/mining/src/mint_quote_notification.rs` (NEW)

```rust
use alloc::vec::Vec;
use binary_sv2::{Str0255, U64};
use serde::{Deserialize, Serialize};

/// Notification sent to downstream when a quote becomes payable
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MintQuoteNotification<'a> {
    pub quote_id: Str0255<'a>,
    pub amount: U64<'a>,
}

/// Failure notification if quote cannot be processed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MintQuoteFailure<'a> {
    pub quote_id: Str0255<'a>,
    pub error_code: u32,
    pub error_message: Str0255<'a>,
}
```

**File**: Update `protocols/v2/subprotocols/mining/src/lib.rs`

```rust
pub mod mint_quote_notification;
pub use mint_quote_notification::{MintQuoteNotification, MintQuoteFailure};
```

### **1.2 Add Message Type Constants**

**File**: Update `protocols/v2/const-sv2/src/lib.rs`

```rust
pub const MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION: u8 = 0xC0;
pub const MESSAGE_TYPE_MINT_QUOTE_FAILURE: u8 = 0xC1;
pub const CHANNEL_BIT_MINT_QUOTE_NOTIFICATION: u32 = 1 << 25;
pub const CHANNEL_BIT_MINT_QUOTE_FAILURE: u32 = 1 << 26;
```

### **1.3 Add Variants to Mining Enum**

**File**: Update `protocols/v2/roles-logic-sv2/src/parsers.rs`

Add to `Mining<'a>` enum:
```rust
pub enum Mining<'a> {
    // ... existing variants ...

    // Extension messages for mint quote notifications
    #[cfg_attr(feature = "with_serde", serde(borrow))]
    MintQuoteNotification(mining_sv2::MintQuoteNotification<'a>),
    #[cfg_attr(feature = "with_serde", serde(borrow))]
    MintQuoteFailure(mining_sv2::MintQuoteFailure<'a>),
}
```

Update all trait implementations (`into_static()`, `msg_type()`, `channel_bit()`, `From<EncodableField>`, `GetSize`):

```rust
impl<'a> Mining<'a> {
    pub fn into_static(self) -> Mining<'static> {
        match self {
            // ... existing arms ...
            Mining::MintQuoteNotification(m) => Mining::MintQuoteNotification(m.into_static()),
            Mining::MintQuoteFailure(m) => Mining::MintQuoteFailure(m.into_static()),
        }
    }
}

impl<'a> IsSv2Message for Mining<'a> {
    fn msg_type(&self) -> u8 {
        match self {
            // ... existing arms ...
            Self::MintQuoteNotification(_) => MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION,
            Self::MintQuoteFailure(_) => MESSAGE_TYPE_MINT_QUOTE_FAILURE,
        }
    }

    fn channel_bit(&self) -> u32 {
        match self {
            // ... existing arms ...
            Self::MintQuoteNotification(_) => CHANNEL_BIT_MINT_QUOTE_NOTIFICATION,
            Self::MintQuoteFailure(_) => CHANNEL_BIT_MINT_QUOTE_FAILURE,
        }
    }
}

impl<'decoder> From<Mining<'decoder>> for EncodableField<'decoder> {
    fn from(value: Mining<'decoder>) -> Self {
        match value {
            // ... existing arms ...
            Mining::MintQuoteNotification(a) => a.into(),
            Mining::MintQuoteFailure(a) => a.into(),
        }
    }
}

impl GetSize for Mining<'_> {
    fn get_size(&self) -> usize {
        match self {
            // ... existing arms ...
            Mining::MintQuoteNotification(a) => a.get_size(),
            Mining::MintQuoteFailure(a) => a.get_size(),
        }
    }
}
```

### **1.4 Create pool-messages Crate**

**File**: `roles/roles-utils/pool-messages/Cargo.toml` (NEW)

```toml
[package]
name = "pool-messages"
version = "0.1.0"
edition = "2021"

[dependencies]
roles_logic_sv2 = { path = "../../../protocols/v2/roles-logic-sv2" }
```

**File**: `roles/roles-utils/pool-messages/src/lib.rs` (NEW)

```rust
//! Re-export for mint crate compatibility
//! Provides PoolMessages type that mint role expects

pub use roles_logic_sv2::parsers_sv2::PoolMessages;
```

**File**: Update `roles/roles-utils/Cargo.toml`

Add to `members`:
```toml
"pool-messages",
```

### **1.5 Update Mint Crate Imports**

**File**: Update `roles/mint/Cargo.toml`

```toml
[dependencies]
pool-messages = { path = "../../roles-utils/pool-messages" }
# ... remove or comment out: network_helpers_sv2 dependency for plain_connection_tokio
```

**File**: Update `roles/mint/src/lib/sv2_connection/connection.rs`

Comment out PlainConnection (will replace in Phase 2):
```rust
// use network_helpers_sv2::plain_connection_tokio::PlainConnection;  // Replaced with proper Noise in Phase 2
```

**File**: Update `roles/mint/src/lib/sv2_connection/message_handler.rs`

```rust
use pool_messages::PoolMessages;
```

**File**: Update `roles/mint/src/lib/sv2_connection/quote_processing.rs`

```rust
use pool_messages::{PoolMessages, Minting};
```

### **Validation - COMPLETED**

After Phase 1:
- ✅ `cd protocols && cargo build` succeeds
- ✅ `cd roles && cargo build` succeeds (mint imports resolve)
- ✅ `MintQuoteNotification` and `MintQuoteFailure` are available in Mining enum
- ✅ Pool can send extension messages (infrastructure in place)

**Phase 1 Completion Date**: 2025-10-26

**Files Modified**:
1. `protocols/v2/subprotocols/mining/src/mint_quote_notification.rs` (NEW)
2. `protocols/v2/subprotocols/mining/src/lib.rs` (added constants and exports)
3. `protocols/v2/parsers-sv2/src/lib.rs` (added Mining enum variants and trait impl arms)
4. `roles/roles-utils/pool-messages/` (created adapter crate - commented out pending Phase 2)
5. `roles/Cargo.toml` (added/commented pool-messages member)

---

## **PHASE 2: Proper Noise Handshake (4-5 hours)**

### **Goal**: Replace plaintext TCP with Noise encryption, implement proper SV2 SetupConnection negotiation

### **2.1 Implement Mint Connection with Noise Encryption**

**File**: Rewrite `roles/mint/src/lib/sv2_connection/connection.rs`

```rust
use std::sync::Arc;
use cdk::mint::Mint;
use shared_config::Sv2MessagingConfig;
use tokio::net::TcpStream;
use network_helpers_sv2::noise_connection::Connection;
use codec_sv2::HandshakeRole;
use roles_logic_sv2::common_messages_sv2::{SetupConnection, Protocol};
use pool_messages::PoolMessages;
use tracing::info;
use anyhow::Result;

use super::message_handler::handle_sv2_connection;

/// Connect to pool via SV2 with Noise encryption
pub async fn connect_to_pool_sv2(
    mint: Arc<Mint>,
    sv2_config: Sv2MessagingConfig,
) {
    info!("Connecting to pool SV2 endpoint: {}", sv2_config.mint_listen_address);

    loop {
        match TcpStream::connect(&sv2_config.mint_listen_address).await {
            Ok(stream) => {
                info!("✅ TCP connection established");

                match establish_sv2_connection(stream).await {
                    Ok((receiver, sender)) => {
                        if let Err(e) = handle_sv2_connection(mint.clone(), receiver, sender).await {
                            tracing::error!("SV2 connection error: {}", e);
                        }
                    },
                    Err(e) => {
                        tracing::error!("Failed to establish SV2 connection: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            },
            Err(e) => {
                tracing::warn!(
                    "Failed to connect to pool at {}: {}",
                    sv2_config.mint_listen_address, e
                );
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

/// Establish SV2 connection: Noise handshake + SetupConnection negotiation
async fn establish_sv2_connection(
    stream: TcpStream,
) -> Result<(
    async_channel::Receiver<codec_sv2::StandardEitherFrame<PoolMessages<'static>>>,
    async_channel::Sender<codec_sv2::StandardEitherFrame<PoolMessages<'static>>>,
)> {
    // Create Noise connection (mint is initiator)
    let mut connection = Connection::new(
        stream,
        HandshakeRole::Initiator,
        None,
    )
    .await?;
    info!("🔐 Noise handshake completed");

    // Send SetupConnection message
    let setup_connection = SetupConnection {
        protocol: Protocol::MiningProtocol,
        min_version: 2,
        max_version: 2,
        flags: 0,  // Mint is stateless quote service
        connection_flags: 0,
    };

    let setup_frame: codec_sv2::StandardSv2Frame<PoolMessages> = PoolMessages::Common(
        roles_logic_sv2::common_messages_sv2::CommonMessages::SetupConnection(setup_connection),
    )
    .try_into()?;

    connection.send(setup_frame.into()).await?;
    info!("Sent SetupConnection message");

    // Receive SetupConnectionSuccess or Error
    let response_frame = connection.recv().await?;

    match response_frame {
        codec_sv2::StandardEitherFrame::Sv2(frame) => {
            let msg_type = frame
                .get_header()
                .ok_or_else(|| anyhow::anyhow!("No frame header"))?
                .msg_type();

            match msg_type {
                0x00 => {
                    info!("✅ Pool accepted SetupConnection");
                },
                0x01 => {
                    return Err(anyhow::anyhow!("Pool rejected SetupConnection"));
                },
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unexpected response type: 0x{:02x}",
                        msg_type
                    ));
                }
            }
        },
        _ => {
            return Err(anyhow::anyhow!("Expected SV2 frame"));
        }
    }

    info!("✅ Mint SV2 connection fully established");
    Ok((connection.receiver().clone(), connection.sender().clone()))
}
```

### **2.2 Update Message Handler**

**File**: Update `roles/mint/src/lib/sv2_connection/message_handler.rs`

```rust
async fn process_sv2_frame(
    mint: &Arc<Mint>,
    either_frame: StandardEitherFrame<PoolMessages<'static>>,
    sender: &async_channel::Sender<StandardEitherFrame<PoolMessages<'static>>>,
) -> Result<()> {
    match either_frame {
        StandardEitherFrame::Sv2(incoming) => {
            let msg_type = incoming
                .get_header()
                .ok_or_else(|| anyhow::anyhow!("No header"))?
                .msg_type();

            match msg_type {
                // Setup responses (handled during connection, log if received again)
                0x00 | 0x01 => {
                    tracing::debug!("Received setup response");
                    Ok(())
                },
                // Mint quote messages (0x80-0x82)
                0x80..=0x82 => {
                    process_mint_quote_message(
                        mint.clone(),
                        msg_type,
                        incoming.payload(),
                        sender,
                    )
                    .await
                },
                _ => {
                    tracing::warn!("Received unsupported message type: 0x{:02x}", msg_type);
                    Ok(())
                }
            }
        },
        StandardEitherFrame::HandShake(_) => {
            tracing::debug!("Received unexpected handshake frame");
            Ok(())
        },
    }
}
```

### **Validation**

After Phase 2:
- ✅ Mint service connects to pool with Noise encryption
- ✅ Logs show: "TCP connection established" → "Noise handshake completed" → "Sent SetupConnection message" → "Pool accepted SetupConnection"
- ✅ Mint stays connected and listens for MintQuoteRequest
- ✅ Connection drops are logged with clear error messages
- ✅ Production-ready security (Noise encryption, proper protocol negotiation)

---

## **PHASE 3: Quote Request/Response Flow (3-4 hours)**

### **Goal**: Complete end-to-end quote dispatching, poller, and extension message delivery

### **3.1 Update Pool Struct**

**File**: Update `roles/pool/src/lib/mining_pool/mod.rs`

Add fields to `Pool` struct:
```rust
pub struct Pool {
    // ... existing fields ...

    /// Sender for quote requests to be forwarded to mint
    quote_sender: async_channel::Sender<MintQuoteRequest>,

    /// Tracks pending quotes: quote_id → (channel_id, timestamp)
    pending_quotes: Arc<RwLock<HashMap<String, (u32, Instant)>>>,
}
```

### **3.2 Wire Quote Request Dispatch**

**File**: Update `roles/pool/src/lib/mining_pool/message_handler.rs`

When share is accepted:
```rust
// Create MintQuoteRequest from accepted share
let mint_request = MintQuoteRequest {
    amount: share.amount,
    unit: Str0255::try_from("HASH")?,
    header_hash: U256::from(&share.header_hash),
    description: None,
    locking_key: /* from channel context */,
};

// Send to mint (fire-and-forget)
pool.quote_sender.send(mint_request).await?;

// Store in pending quotes for correlation
pending_quotes.insert(
    quote_id.clone(),
    (channel_id, Instant::now()),
);
```

### **3.3 Implement Pool-side Mint Connection**

**File**: Create `roles/pool/src/lib/mint_connection.rs` (NEW)

```rust
use anyhow::Result;
use async_channel::{Receiver, Sender};
use tokio::net::TcpListener;
use network_helpers_sv2::noise_connection::Connection;
use codec_sv2::{HandshakeRole, StandardEitherFrame};
use roles_logic_sv2::common_messages_sv2::{SetupConnection, SetupConnectionSuccess};
use pool_messages::PoolMessages;
use tracing::info;

pub struct MintConnection {
    receiver: Receiver<StandardEitherFrame<PoolMessages<'static>>>,
    sender: Sender<StandardEitherFrame<PoolMessages<'static>>>,
}

impl MintConnection {
    /// Accept incoming mint connection (pool is responder)
    pub async fn accept(address: &str) -> Result<Self> {
        info!("Listening for mint connection on {}", address);

        let listener = TcpListener::bind(address).await?;
        let (stream, peer) = listener.accept().await?;
        info!("✅ Accepted mint connection from {}", peer);

        // Pool is responder (mint initiates)
        let mut connection = Connection::new(
            stream,
            HandshakeRole::Responder,
            None,
        )
        .await?;
        info!("🔐 Noise handshake completed");

        // Receive SetupConnection from mint
        let response_frame = connection.recv().await?;

        if let StandardEitherFrame::Sv2(frame) = response_frame {
            let msg_type = frame
                .get_header()
                .ok_or_else(|| anyhow::anyhow!("No header"))?
                .msg_type();

            if msg_type != 0x00 {
                return Err(anyhow::anyhow!("Expected SetupConnection"));
            }
            info!("Received SetupConnection from mint");
        } else {
            return Err(anyhow::anyhow!("Expected SV2 frame"));
        }

        // Send SetupConnectionSuccess
        let success = SetupConnectionSuccess {
            version_ack: (2 << 8) | 2,
            flags: 0,
            connection_flags: 0,
        };

        let setup_frame: codec_sv2::StandardSv2Frame<PoolMessages> = PoolMessages::Common(
            roles_logic_sv2::common_messages_sv2::CommonMessages::SetupConnectionSuccess(success),
        )
        .try_into()?;

        connection.send(setup_frame.into()).await?;
        info!("✅ Sent SetupConnectionSuccess");

        Ok(Self {
            receiver: connection.receiver().clone(),
            sender: connection.sender().clone(),
        })
    }

    /// Send MintQuoteRequest to mint
    pub async fn send_quote_request(
        &self,
        request: pool_messages::Minting,
    ) -> Result<()> {
        let frame: StandardEitherFrame<PoolMessages> =
            PoolMessages::Minting(request).try_into()?;

        self.sender
            .send(frame)
            .await
            .map_err(|e| anyhow::anyhow!("Send failed: {}", e))
    }

    /// Receive quote response (non-blocking with timeout)
    pub async fn try_recv_quote_response(
        &mut self,
    ) -> Result<Option<pool_messages::Minting>> {
        match tokio::time::timeout(
            std::time::Duration::from_millis(100),
            self.receiver.recv(),
        )
        .await
        {
            Ok(Ok(StandardEitherFrame::Sv2(frame))) => {
                let msg_type = frame
                    .get_header()
                    .ok_or_else(|| anyhow::anyhow!("No header"))?
                    .msg_type();

                // Parse mint quote response (would be implemented fully)
                Ok(Some(pool_messages::Minting::MintQuoteResponse(/* ... */)))
            },
            Ok(Err(e)) => Err(anyhow::anyhow!("Connection error: {}", e)),
            Err(_) => Ok(None),  // Timeout
        }
    }
}
```

### **3.4 Send Extension Messages to Translators**

**File**: Add to `roles/pool/src/lib/mining_pool/mod.rs`

```rust
use roles_logic_sv2::mining_sv2::MintQuoteNotification;
use binary_sv2::Str0255;

/// Send MintQuoteNotification extension message to downstream
pub async fn send_mint_quote_notification_to_downstream(
    self_: Arc<Mutex<Self>>,
    channel_id: u32,
    quote_id: impl AsRef<str>,
    amount: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let notification = MintQuoteNotification {
        quote_id: Str0255::try_from(quote_id.as_ref())?,
        amount: amount.into(),
    };

    let mining_message = roles_logic_sv2::parsers::Mining::MintQuoteNotification(notification);

    // Get downstream and send via existing infrastructure
    let pool = self_
        .safe_lock(|p| p.downstreams.get(&channel_id).cloned())
        .ok()?
        .ok_or("Downstream not found")?;

    Downstream::match_send_to(
        pool,
        Ok(SendTo::Respond(mining_message)),
    )
    .await
    .map_err(|e| format!("Failed to send: {:?}", e).into())
}
```

### **3.5 Periodic Quote Poller**

**File**: Add to `roles/pool/src/lib/mod.rs` (or create `roles/pool/src/lib/quote_poller.rs`)

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info};

/// Periodically poll mint for paid quotes and notify downstreams
pub async fn run_quote_poller(
    pool: Arc<Mutex<Pool>>,
    mint_http_endpoint: &str,
) {
    let mut ticker = interval(Duration::from_secs(5));
    let client = reqwest::Client::new();

    loop {
        ticker.tick().await;

        // Poll mint for PAID quotes
        match client
            .get(format!("{}/quotes?status=paid", mint_http_endpoint))
            .send()
            .await
        {
            Ok(response) => {
                if let Ok(quotes) = response.json::<Vec<QuoteInfo>>().await {
                    for quote in quotes {
                        debug!("Found paid quote: id={}, amount={}", quote.id, quote.amount);

                        // Look up which channel/downstream this quote belongs to
                        if let Some((channel_id, _)) = find_pending_quote(&quote.id).await {
                            // Send MintQuoteNotification to translator
                            if let Err(e) = Pool::send_mint_quote_notification_to_downstream(
                                pool.clone(),
                                channel_id,
                                &quote.id,
                                quote.amount,
                            )
                            .await
                            {
                                error!("Failed to send notification: {}", e);
                            } else {
                                info!(
                                    "✅ Sent MintQuoteNotification for quote {} to channel {}",
                                    quote.id, channel_id
                                );
                            }
                        }
                    }
                }
            },
            Err(e) => {
                error!("Failed to poll mint: {}", e);
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct QuoteInfo {
    id: String,
    amount: u64,
    #[serde(default)]
    status: String,
}
```

### **Validation**

After Phase 3:
- ✅ Pool accepts share from translator
- ✅ Pool sends `MintQuoteRequest` to mint via SV2 connection
- ✅ Mint receives and creates quote in database (status: PAID immediately in Phase 1)
- ✅ Pool's quote poller runs every 5 seconds
- ✅ Poller finds newly PAID quotes via HTTP API
- ✅ Pool sends `MintQuoteNotification` (extension message) to translator
- ✅ Translator receives notification and calls mint to claim token
- ✅ Translator stores complete Cashu token in wallet
- ✅ Dashboard shows complete ehash flow working
- ✅ Smoke test passes: `devenv up` produces valid ehash tokens

---

## **Quote Status in Pool**

The pool should communicate quote status when creating MintQuoteRequest:

### **Possible Status Values**

```rust
pub enum QuoteStatus {
    /// Quote is pending block template validation
    Pending,

    /// Quote is immediately payable (no template validation required)
    Paid,

    /// Quote cannot be processed
    Failed,
}
```

### **How Pool Specifies Status**

**Option 1: In MintQuoteRequest message**
```rust
pub struct MintQuoteRequest<'a> {
    pub amount: u64,
    pub unit: Str0255<'a>,
    pub header_hash: U256<'a>,
    pub description: Sv2Option<'a, Str0255<'a>>,
    pub locking_key: CompressedPubKey<'a>,
    pub status: u8,  // 0=pending, 1=paid, 2=failed
}
```

**Option 2: Via separate QuoteStatusRequest message**
```rust
pub struct SetQuoteStatus<'a> {
    pub quote_id: Str0255<'a>,
    pub status: u8,
}
```

**Option 3: In config - hardcoded behavior**
```toml
[mint]
quote_status = "paid"  # Phase 1: always paid
# quote_status = "pending"  # Phase 2+: await block validation
```

**Recommendation**: Use **Option 3** (config) for Phase 1 simplicity, migrate to Option 1 (in message) for Phase 2+ when block validation is implemented.

---

## **Summary: Timeline & Deliverables**

| Phase | Duration | Key Deliverables | Validation |
|-------|----------|------------------|-----------|
| **1** | 2 hours | MintQuoteNotification in Mining enum, pool-messages crate, mint imports fixed | Code compiles |
| **2** | 4-5 hours | Noise encryption, SetupConnection handshake, proper Connection setup | Mint connects securely |
| **3** | 3-4 hours | Quote dispatcher, poller, extension messages, translator delivery | Smoke test passes |

**Total: ~9-11 hours**

---

## **Testing Checklist**

### **Phase 1 Complete**
- [ ] `cd protocols && cargo build` passes
- [ ] `cd roles && cargo build` passes
- [ ] Mint crate imports resolve without errors
- [ ] MintQuoteNotification available in Mining enum

### **Phase 2 Complete**
- [ ] Mint service starts without panics
- [ ] Logs show: "TCP connection established" → "Noise handshake completed" → "SetupConnection accepted"
- [ ] Mint remains connected and listening
- [ ] Connection loss is logged clearly

### **Phase 3 Complete**
- [ ] Pool starts and listens for mint connection
- [ ] Share submission triggers MintQuoteRequest to mint
- [ ] Quote poller runs every 5 seconds
- [ ] Paid quotes are found via HTTP API
- [ ] MintQuoteNotification sent to translator via extension message
- [ ] Translator receives and processes notification
- [ ] `devenv up` produces valid ehash tokens in wallet
- [ ] Dashboard shows complete flow

---

## **References**

- Working implementation: commit `b534e6be` (implement SV2 extension messages)
- Quote dispatcher: commit `2a067dc1` (add quote dispatcher to pool)
- Translator Upstream example: `roles/translator/src/lib/upstream_sv2/upstream.rs`
- JD-Server example: `roles/jd-server/src/lib/job_declarator/mod.rs`

