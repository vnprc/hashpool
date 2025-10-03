# Hashpool Architectural Refactoring Plan

## Goals

1. **Separate web dashboards into standalone service** - Allow pool/translator restarts without killing website
2. **Extract quote handling to protocols/ehash with callbacks** - Decouple from SRI internals for easier rebasing
3. **Simplify stats collection** - Generic, reusable across services

---

## Current Architecture Issues

### 1. Web Dashboards Tightly Coupled (1,618 total lines)
- **Pool:** 621 lines in `roles/pool/src/lib/web.rs`
- **Translator:** 997 lines in `roles/translator/src/lib/web.rs`
- Stats collection embedded in pool/translator
- Can't restart services without killing websites
- Tight coupling to Pool/Bridge internal structs

### 2. Quote Handling Scattered Across Layers
- Pool: `submit_quote()` function in message handler (`roles/pool/src/lib/mining_pool/message_handler.rs:27-92`)
- Quote building, validation, and dispatch mixed together
- Hub interaction embedded in message handler
- Stats updates in quote submission path
- Hard to port during rebase when SRI changes message handlers

### 3. Tight Coupling to SRI Internals
- Pool message handler knows about MintPoolMessageHub
- Web servers depend on Pool/Bridge structs
- Makes rebase difficult when SRI changes these structs
- Large API surface area to maintain compatibility with

---

## Proposed Architecture

### 1. Extract Web Dashboards → Two Separate Services

**Goal:** Create independent dashboard services that can be deployed separately:
- **Pool Dashboard:** Deployed with pool, jds, bitcoind, mint (server-side infrastructure)
- **Proxy Dashboard:** Deployed with translator, jdc, bitcoind (client-side infrastructure)

**New Directory Structure:**
```
roles/pool-dashboard/
├── Cargo.toml
├── src/
│   ├── main.rs              # Standalone HTTP server for pool
│   ├── stats.rs             # Pool stats aggregation
│   └── web/                 # HTML/CSS templates
│       ├── index.html
│       └── styles.css

roles/proxy-dashboard/
├── Cargo.toml
├── src/
│   ├── main.rs              # Standalone HTTP server for proxy/translator
│   ├── stats.rs             # Translator/miner stats aggregation
│   └── web/                 # HTML/CSS templates
│       ├── index.html
│       └── styles.css
```

**Deployment Model:**
```
Server-side (Pool Infrastructure):
  ┌─────────────┐
  │   bitcoind  │
  └──────┬──────┘
         │
  ┌──────┴──────┬───────────┬─────────┐
  │    Pool     │    JDS    │  Mint   │
  └──────┬──────┴───────────┴─────────┘
         │
  ┌──────┴──────────┐
  │ Pool Dashboard  │ ← Subscribes to pool stats
  └─────────────────┘

Client-side (Miner Infrastructure):
  ┌─────────────┐
  │   bitcoind  │
  └──────┬──────┘
         │
  ┌──────┴──────┬───────────┐
  │ Translator  │    JDC    │
  └──────┬──────┴───────────┘
         │
  ┌──────┴───────────┐
  │ Proxy Dashboard  │ ← Subscribes to translator stats
  └──────────────────┘
```

**Communication Pattern:**
```
Pool → Broadcast Channel → Pool Dashboard
  (publish stats)           (subscribe & aggregate)

Translator → Broadcast Channel → Proxy Dashboard
  (publish stats)                (subscribe & aggregate)
```

**Benefits:**
- ✅ Restart pool/translator without killing dashboards
- ✅ Independent deployment on different hardware
- ✅ No coupling to Pool/Bridge internal structs
- ✅ Stats collection via message passing only
- ✅ Each dashboard tailored to its service

---

### 2. Extract Quote Handling → `protocols/ehash` with Callbacks

**New Files:**
```
protocols/ehash/src/
├── quote.rs          # Existing quote types (keep as-is)
├── quote_handler.rs  # NEW: Generic quote handling logic
└── callbacks.rs      # NEW: Callback trait for quote events
```

**Callback Trait API:**
```rust
// protocols/ehash/src/callbacks.rs

/// Callbacks for quote lifecycle events
pub trait QuoteCallbacks: Send + Sync {
    /// Called when a quote is successfully created from a share
    fn on_quote_created(
        &self,
        share_hash: ShareHash,
        amount: u64,
        channel_id: u32
    );

    /// Called when quote should be dispatched to mint
    fn on_quote_send(
        &self,
        quote: ParsedMintQuoteRequest,
        context: QuoteContext
    );

    /// Called when quote response received from mint
    fn on_quote_response(
        &self,
        response: MintQuoteResponseEvent
    );

    /// Called when quote encounters an error
    fn on_quote_error(
        &self,
        error: QuoteError,
        share_hash: Option<ShareHash>
    );
}

#[derive(Debug, Clone)]
pub struct QuoteContext {
    pub channel_id: u32,
    pub sequence_number: u32,
    pub amount: u64,
}
```

**Quote Handler API:**
```rust
// protocols/ehash/src/quote_handler.rs

use super::callbacks::{QuoteCallbacks, QuoteContext};
use super::quote::{build_parsed_quote_request, QuoteError};
use super::share::ShareHash;
use super::work::calculate_ehash_amount;

pub struct QuoteHandler<C: QuoteCallbacks> {
    hub: Arc<MintPoolMessageHub>,
    callbacks: Arc<C>,
    sv2_config: Option<Sv2MessagingConfig>,
    minimum_difficulty: u32,
}

impl<C: QuoteCallbacks> QuoteHandler<C> {
    pub fn new(
        hub: Arc<MintPoolMessageHub>,
        callbacks: Arc<C>,
        sv2_config: Option<Sv2MessagingConfig>,
        minimum_difficulty: u32,
    ) -> Self {
        Self {
            hub,
            callbacks,
            sv2_config,
            minimum_difficulty,
        }
    }

    /// Main entry point: process a share submission and create quote
    pub fn handle_share_submission(
        &self,
        share: &SubmitSharesExtended,
    ) -> Result<(), QuoteError> {
        // Extract header hash
        let hash = Hash::from_slice(share.hash.inner_as_ref())
            .map_err(|e| QuoteError::InvalidHeaderHash(e.to_string()))?;

        // Calculate ehash amount
        let amount = calculate_ehash_amount(
            hash.to_byte_array(),
            self.minimum_difficulty
        );

        // Notify via callback: quote created
        let share_hash = ShareHash::from_hash(hash)?;
        self.callbacks.on_quote_created(
            share_hash.clone(),
            amount,
            share.channel_id,
        );

        // Build quote request
        let quote = build_parsed_quote_request(
            amount,
            share.hash.inner_as_ref(),
            share.locking_pubkey.clone(),
        ).map_err(|e| {
            self.callbacks.on_quote_error(e.clone(), Some(share_hash.clone()));
            e
        })?;

        // Create context
        let context = QuoteContext {
            channel_id: share.channel_id,
            sequence_number: share.sequence_number,
            amount,
        };

        // Check if messaging enabled
        let messaging_enabled = self.sv2_config
            .as_ref()
            .map(|cfg| cfg.enabled)
            .unwrap_or(true);

        if !messaging_enabled {
            debug!("SV2 messaging disabled; skipping quote for channel {}", share.channel_id);
            return Ok(());
        }

        // Notify via callback: sending quote
        self.callbacks.on_quote_send(quote.clone(), context.clone());

        // Dispatch via hub (async)
        let hub = self.hub.clone();
        tokio::spawn(async move {
            if let Err(e) = hub.send_quote_request(quote, context).await {
                error!("Failed to dispatch quote via hub: {}", e);
            }
        });

        Ok(())
    }
}
```

**Pool Integration Example:**
```rust
// roles/pool/src/lib/mining_pool/quote_callbacks.rs

use ehash::callbacks::{QuoteCallbacks, QuoteContext};
use ehash::quote::ParsedMintQuoteRequest;
use ehash::share::ShareHash;
use super::stats::{StatsHandle, StatsMessage};

pub struct PoolQuoteCallbacks {
    stats_handle: StatsHandle,
}

impl PoolQuoteCallbacks {
    pub fn new(stats_handle: StatsHandle) -> Self {
        Self { stats_handle }
    }
}

impl QuoteCallbacks for PoolQuoteCallbacks {
    fn on_quote_created(&self, _hash: ShareHash, amount: u64, channel_id: u32) {
        // Just update stats - no business logic
        self.stats_handle.send_stats(StatsMessage::QuoteCreated {
            downstream_id: channel_id,
            amount,
        });
    }

    fn on_quote_send(&self, quote: ParsedMintQuoteRequest, ctx: QuoteContext) {
        // Log the dispatch
        let share_hash_hex = hex::encode(quote.share_hash.as_bytes());
        info!(
            "Dispatching quote: share_hash={}, channel_id={}, amount={}",
            share_hash_hex, ctx.channel_id, ctx.amount
        );
    }

    fn on_quote_response(&self, response: MintQuoteResponseEvent) {
        // Handle successful quote response
        info!("Quote response received: {:?}", response);
    }

    fn on_quote_error(&self, error: QuoteError, share_hash: Option<ShareHash>) {
        // Log errors
        warn!("Quote error: {:?} for share: {:?}", error, share_hash);
    }
}
```

**Simplified Pool Message Handler:**
```rust
// roles/pool/src/lib/mining_pool/message_handler.rs

impl Downstream {
    fn handle_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtended<'_>,
    ) -> Result<SendTo<()>, Error> {
        // Existing channel factory logic
        let res = self.channel_factory
            .safe_lock(|cf| cf.on_submit_shares_extended(m.clone()))
            .map_err(|e| roles_logic_sv2::Error::PoisonLock(e.to_string()))?;

        match res {
            Ok(res) => match res {
                OnNewShare::ShareMeetDownstreamTarget | OnNewShare::ShareMeetBitcoinTarget(..) => {
                    // Stats for share submission
                    if let Ok(stats_handle) = self.pool.safe_lock(|p| p.stats_handle.clone()) {
                        stats_handle.send_stats(StatsMessage::ShareSubmitted {
                            downstream_id: self.id
                        });
                    }

                    // Quote handling reduced to ONE LINE:
                    self.quote_handler.handle_share_submission(&m)?;

                    // Build success response
                    let success = SubmitSharesSuccess { /* ... */ };
                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
                },
                OnNewShare::SendErrorDownstream(m) => {
                    Ok(SendTo::Respond(Mining::SubmitSharesError(m)))
                }
                _ => unreachable!(),
            },
            Err(err) => {
                // Error handling
                let submit_error = build_submit_share_error(m.channel_id, m.sequence_number, &err);
                Ok(SendTo::Respond(Mining::SubmitSharesError(submit_error)))
            }
        }
    }
}
```

**Benefits:**
- ✅ Quote logic isolated from SRI message handlers
- ✅ Pool/translator just implement callbacks (stats, logging, etc.)
- ✅ Hub interaction extracted
- ✅ Easy to test quote logic independently
- ✅ Rebase only needs to update SubmitSharesExtended binding, not all quote logic
- ✅ Clear separation: domain logic (ehash) vs infrastructure (pool)

---

### 3. Stats Collection Architecture

**Design:** SV2-style stats service with TCP messaging and persistent time-series storage

**Problems with HTTP approach:**
- ❌ Adds HTTP attack surface to pool/translator
- ❌ No time-series data for graphs
- ❌ Not the SV2 way - we use TCP messages for everything else

**Better Architecture: Dedicated Stats Service**

```
Pool Process:
  ├─ StatsManager (existing)
  ├─ TCP connection to pool-stats service
  └─ Sends stats messages via TCP (SV2-style)

Pool Stats Service (new):
  ├─ TCP server (listens for pool connections)
  ├─ Receives stats messages from pool
  ├─ SQLite database (time-series data)
  ├─ Aggregates: hashrate over time, shares, quotes
  └─ HTTP server (only for dashboard HTML/API)

Pool Dashboard (simple web server):
  ├─ Serves HTML/CSS/JS
  ├─ HTTP API: GET /api/stats → current stats
  ├─ HTTP API: GET /api/hashrate?hours=24 → time series
  └─ Embedded in pool-stats service (same process)

---

Translator Process:
  ├─ MinerTracker (existing)
  ├─ TCP connection to proxy-stats service
  └─ Sends stats messages via TCP (SV2-style)

Proxy Stats Service (new):
  ├─ TCP server (listens for translator connections)
  ├─ Receives stats messages from translator
  ├─ SQLite database (time-series data)
  ├─ Aggregates: miner hashrate over time, shares
  └─ HTTP server (only for dashboard HTML/API)

Proxy Dashboard (simple web server):
  ├─ Serves HTML/CSS/JS
  ├─ HTTP API: GET /api/stats → current stats
  ├─ HTTP API: GET /api/miners?hours=24 → time series
  └─ Embedded in proxy-stats service (same process)
```

**Message Protocol (SV2-style):**
```rust
// Stats messages sent over TCP (pool → pool-stats)
#[derive(Encodable, Decodable)]
pub enum PoolStatsMessage {
    ShareSubmitted { downstream_id: u32, timestamp: u64 },
    QuoteCreated { downstream_id: u32, amount: u64, timestamp: u64 },
    ChannelOpened { downstream_id: u32, channel_id: u32 },
    ChannelClosed { downstream_id: u32, channel_id: u32 },
    DownstreamConnected { downstream_id: u32, flags: u32 },
    DownstreamDisconnected { downstream_id: u32 },
}

// Stats messages sent over TCP (translator → proxy-stats)
#[derive(Encodable, Decodable)]
pub enum ProxyStatsMessage {
    MinerConnected { miner_id: u32, name: String },
    MinerDisconnected { miner_id: u32 },
    ShareSubmitted { miner_id: u32, difficulty: f64, timestamp: u64 },
    HashrateUpdate { miner_id: u32, hashrate: f64, timestamp: u64 },
}
```

**SQLite Schema for Time-Series:**
```sql
-- Pool stats database
CREATE TABLE hashrate_samples (
    timestamp INTEGER NOT NULL,
    downstream_id INTEGER NOT NULL,
    shares_5min INTEGER NOT NULL,
    estimated_hashrate REAL NOT NULL
);
CREATE INDEX idx_hashrate_time ON hashrate_samples(timestamp);

CREATE TABLE quote_history (
    timestamp INTEGER NOT NULL,
    downstream_id INTEGER NOT NULL,
    amount INTEGER NOT NULL
);

-- Proxy stats database
CREATE TABLE miner_hashrate (
    timestamp INTEGER NOT NULL,
    miner_id INTEGER NOT NULL,
    shares_5min INTEGER NOT NULL,
    estimated_hashrate REAL NOT NULL
);
CREATE INDEX idx_miner_hashrate_time ON miner_hashrate(timestamp);
```

**Benefits:**
- ✅ **No HTTP on pool/translator** - Zero attack surface increase
- ✅ **SV2-style messaging** - Consistent with rest of codebase
- ✅ **Time-series storage** - SQLite for graphs and history
- ✅ **Independent deployment** - Stats service can restart without affecting pool
- ✅ **Future-proof** - Easy to add prometheus/grafana later
- ✅ **Clean separation** - Pool/translator just send messages, stats service handles everything
- ✅ **Reconnection logic** - Stats service can reconnect to pool if either restarts

---

## Implementation Plan

### Phase 1: Extract Quote Handler ✅ COMPLETED
**Effort:** 2 hours (actual)
**Impact:** Makes rebase significantly easier by isolating mint quote logic

**Completed Implementation:**
1. Created `roles/roles-utils/quote-dispatcher` crate
   - `QuoteDispatcher` struct handles all quote submission logic
   - `QuoteEventCallback` trait for stats integration
   - Extracted all quote building, validation, and dispatch logic

2. Updated pool to use `QuoteDispatcher`
   - Created `PoolStatsCallback` implementation
   - Added `QuoteDispatcher` field to `Downstream` struct
   - Replaced two `submit_quote()` calls with `dispatcher.submit_quote()`
   - Removed 66-line `submit_quote()` function from message_handler

3. Results
   - Net reduction: 81 lines in message_handler.rs (94 deleted, 13 added)
   - All mint quote logic isolated from SRI pool message handling
   - Builds and runs successfully
   - Zero runtime errors related to quote handling

**Testing:**
- ✅ Compiles successfully
- ✅ Pool starts without errors
- ✅ No quote-related runtime errors in logs
- ✅ Ready for next rebase

---

### Phase 2: Create Stats Message Protocol ✅ COMPLETED
**Effort:** 1 hour (actual)
**Impact:** Enables SV2-style stats services

**Completed Implementation:**
1. Created `protocols/v2/subprotocols/stats-sv2/` crate
   - Defined pool stats message types (Encodable/Decodable):
     - `ShareSubmitted`, `QuoteCreated`
     - `ChannelOpened`, `ChannelClosed`
     - `DownstreamConnected`, `DownstreamDisconnected`
   - Defined proxy stats message types (Encodable/Decodable):
     - `MinerConnected`, `MinerDisconnected`
     - `MinerShareSubmitted`, `MinerHashrateUpdate`
   - All types use SV2 derive macros for encoding
   - Added to protocols workspace

2. Results:
   - Clean SV2 message protocol for stats
   - All integer types (no floats) for wire encoding
   - Compiles successfully
   - Ready for stats service implementation

**Note:** TCP connection implementation moved to Phase 3 where stats services are created

---

### Phase 3: Create Stats Services with Dashboards ✅ COMPLETED
**Effort:** 4 hours (actual)
**Impact:** Independent deployment, time-series data, hashrate graphs

**Completed Implementation:**
1. ✅ Created `roles/pool-stats/` crate
   - TCP server implementation (listens on 127.0.0.1:4000)
   - JSON message protocol for stats (simplified from SV2 binary for MVP)
   - SQLite database for time-series storage (hashrate_samples, quote_history, current_stats tables)
   - HTTP server for dashboard on port 8081
   - API endpoints: `/api/stats`, `/api/hashrate?hours=24`
   - Dashboard HTML with live updates (5-second refresh)
   - Compiles successfully

2. ✅ Created `roles/proxy-stats/` crate
   - TCP server implementation (listens on 127.0.0.1:4001)
   - Same JSON message protocol structure
   - SQLite database for miner stats
   - HTTP server for dashboard on port 8082
   - API endpoints: `/api/stats`, `/api/miners?hours=24`
   - Dashboard HTML with live miner stats
   - Compiles successfully

3. ✅ Updated pool
   - Removed web.rs module declaration (not deleted, kept for reference)
   - Created stats_client.rs with TCP client for pool-stats service
   - Added stats_server_address: Option<String> to Configuration
   - Updated all StatsMessage calls to include timestamps where needed
   - Pool now sends stats via JSON/TCP to external pool-stats service
   - Compiles successfully

4. ✅ Updated translator
   - Removed web.rs module declaration (not deleted, kept for reference)
   - Created stats_client.rs with TCP client for proxy-stats service
   - Added stats_server_address: Option<String> to ProxyConfig
   - Translator ready to send stats via JSON/TCP to external proxy-stats service
   - Compiles successfully
   - Note: MinerTracker kept for internal tracking, can be removed in future if not needed

5. ✅ Updated devenv.nix
   - Added pool-stats process (TCP: 4000, HTTP: 8081)
   - Added proxy-stats process (TCP: 4001, HTTP: 8082)
   - Added database path configuration for both services
   - Updated pool to wait for pool-stats before starting
   - Updated translator to wait for proxy-stats before starting
   - Added directory creation for stats databases

6. ✅ Updated configuration files
   - Added `stats_server_address = "127.0.0.1:4000"` to `config/pool.config.toml`
   - Added `stats_server_address = "127.0.0.1:4001"` to `config/tproxy.config.toml`

**Configuration:**
```toml
# config/pool.config.toml
stats_server_address = "127.0.0.1:4000"

# config/tproxy.config.toml
stats_server_address = "127.0.0.1:4001"
```

**devenv.nix Process Configuration:**
```nix
pool-stats = {
  exec = ''
    cargo -C roles/pool-stats -Z unstable-options run -- \
      --tcp-address 127.0.0.1:4000 \
      --http-address 127.0.0.1:8081 \
      --db-path .devenv/state/pool-stats/stats.sqlite
  '';
};

proxy-stats = {
  exec = ''
    cargo -C roles/proxy-stats -Z unstable-options run -- \
      --tcp-address 127.0.0.1:4001 \
      --http-address 127.0.0.1:8082 \
      --db-path .devenv/state/proxy-stats/stats.sqlite
  '';
};
```

**Results:**
- ✅ Both stats services compile successfully
- ✅ Added to roles workspace
- ✅ SQLite schemas defined for time-series data
- ✅ HTTP dashboards with live updates
- ✅ Integrated with pool and translator via TCP clients
- ✅ Configuration files updated
- ✅ devenv.nix updated with stats services
- ✅ **Phase 3 fully complete and ready for testing**

**Testing Instructions:**
To test the new stats infrastructure:
1. Start devenv: `devenv up`
2. Pool-stats dashboard: `http://localhost:8081`
3. Proxy-stats dashboard: `http://localhost:8082`
4. API endpoints:
   - Pool stats: `curl http://localhost:8081/api/stats`
   - Pool hashrate (24h): `curl http://localhost:8081/api/hashrate?hours=24`
   - Proxy stats: `curl http://localhost:8082/api/stats`
   - Proxy miners (24h): `curl http://localhost:8082/api/miners?hours=24`
5. Verify stats are being collected when miners submit shares
6. Test resilience: Restart pool while pool-stats is running → verify reconnection

---

## Impact on Rebase Difficulty

| Component | Current Complexity | With Refactor | Improvement |
|-----------|-------------------|---------------|-------------|
| **Quote handling** | Embedded in message handler (hard to port) | Self-contained in protocols/ehash (easy to port) | 🟢 **Major** |
| **Web dashboards** | Coupled to Pool/Bridge structs (breaks on struct changes) | Separate service (no coupling) | 🟢 **Major** |
| **Stats collection** | Mixed with business logic | Generic, reusable | 🟢 **Medium** |
| **Pool message handler** | 500+ lines, multiple concerns | ~200 lines, single concern | 🟢 **Major** |
| **SRI API surface** | Large (Pool, Bridge, web, stats, quotes) | Small (just share submission) | 🟢 **Major** |

**Estimated Rebase Time:**
- Before refactor: 10-20 hours
- After refactor: 6-12 hours
- **Savings: 4-8 hours**

**Refactoring Time:** 8-12 hours

**Net Impact:** Break even on time, but gain:
- ✅ Cleaner architecture
- ✅ Independent services
- ✅ Easier future maintenance
- ✅ Better testability

---

## Execution Strategy

**Approach:** Refactor on v1.2.1 first, then reapply clean code to v1.5.0

**Why:**
- Cleaner, modular code is easier to reapply
- Test refactor on known-good codebase
- Reapplication becomes straightforward with decoupled components

**Steps:**
1. Execute Phases 1-3 on current master (v1.2.1 base)
2. Test each phase using devenv environment
3. Once refactor complete, use git worktree approach from REAPPLY_PLAN.md
4. Reapply clean, modular code to v1.5.0

---

## Files to Create

### Phase 1: Quote Handler
- `protocols/ehash/src/callbacks.rs`
- `protocols/ehash/src/quote_handler.rs`
- `roles/pool/src/lib/mining_pool/quote_callbacks.rs`

### Phase 2: Stats
- `roles/roles-utils/stats/Cargo.toml`
- `roles/roles-utils/stats/src/lib.rs`
- `roles/roles-utils/stats/src/manager.rs`
- `roles/roles-utils/stats/src/messages.rs`
- `roles/roles-utils/stats/src/publisher.rs`

### Phase 3: Dashboard Services
- `roles/pool-dashboard/Cargo.toml`
- `roles/pool-dashboard/src/main.rs`
- `roles/pool-dashboard/src/stats.rs`
- `roles/pool-dashboard/src/web/index.html`
- `roles/proxy-dashboard/Cargo.toml`
- `roles/proxy-dashboard/src/main.rs`
- `roles/proxy-dashboard/src/stats.rs`
- `roles/proxy-dashboard/src/web/index.html`

### Files to Modify

**Phase 1:**
- `protocols/ehash/src/lib.rs` - export new modules
- `roles/pool/src/lib/mining_pool/message_handler.rs` - use QuoteHandler
- `roles/pool/src/lib/mining_pool/mod.rs` - add QuoteHandler field

**Phase 2:**
- `roles/pool/src/lib/mod.rs` - use stats crate
- `roles/translator/src/lib/mod.rs` - use stats crate
- Move `roles/pool/src/lib/stats.rs` → `roles/roles-utils/stats/src/pool_stats.rs`

**Phase 3:**
- `roles/pool/src/lib/mod.rs` - remove web server
- `roles/translator/src/lib/mod.rs` - remove web server
- Delete: `roles/pool/src/lib/web.rs`
- Delete: `roles/translator/src/lib/web.rs`
- `devenv.nix` - add pool-dashboard and proxy-dashboard processes
- `roles/Cargo.toml` - add pool-dashboard and proxy-dashboard members

### Files to Delete

**Phase 3:**
- `roles/pool/src/lib/web.rs` (621 lines)
- `roles/translator/src/lib/web.rs` (997 lines)
- `roles/pool/src/lib/stats.rs` (after moving to stats crate)
