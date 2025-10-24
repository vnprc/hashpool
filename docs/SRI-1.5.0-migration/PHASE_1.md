# Minimal Fast Migration: Hashpool to SRI 1.5.0

**Objective:** Get hashpool running on SRI 1.5.0 in **days, not weeks**
**Strategy:** Modify ONLY what's necessary; defer everything else to Phase 2
**Philosophy:** Better to have working minimum on 1.5.0 than perfect code on old version

---

## The Core Insight: Extension Messages Work!

Hashpool already uses **extension messages** to send `MintQuoteNotification` without modifying the mining protocol. This means:

- ✅ No changes to SRI mining protocol needed
- ✅ No MintQuoteNotification added to mining_sv2
- ✅ Ehash integration is entirely in **hashpool code**, not SRI code
- ✅ We can handle extension messages in Pool and Translator without touching SRI internals

---

## Phase 0: Pre-Flight Check (1 hour)

Before starting the rebase, verify SRI 1.5.0 supports what we need:

```bash
# Switch to SRI 1.5.0 repo
cd /home/evan/work/stratum

# Check: Does translator handle extension messages?
grep -r "extension_message" roles/translator --include="*.rs"

# Check: Does Pool expose message dispatch mechanism?
grep -r "Message::.*message_type\|route.*message" roles/pool --include="*.rs"

# Check: Can we extend Mining protocol with custom message types?
grep -r "impl.*From\|pub enum Message" protocols/v2/subprotocols/mining/src/ | head -10
```

If SRI 1.5.0 doesn't have extension message infrastructure, we may need to add it (but only in hashpool's fork, as a thin wrapper).

---

## Phase 1: Base Branch & Validation (30 min)

```bash
git checkout -b migrate/sri-1.5.0-minimal v1.5.0
cargo build --workspace
cargo test --lib --workspace
```

**Expected result:** Clean build, all tests pass.

---

## Phase 2: Copy Hashpool-Only Crates (45 min)

Copy ONLY the crates that are 100% new (no SRI equivalents):

```bash
git checkout master -- \
  protocols/ehash/ \
  protocols/v2/subprotocols/mint-quote/ \
  protocols/v2/subprotocols/stats-sv2/ \
  roles/mint/ \
  roles/roles-utils/mint-pool-messaging/ \
  roles/roles-utils/stats/ \
  roles/roles-utils/quote-dispatcher/ \
  roles/roles-utils/config/

# Update Cargo.toml workspace members
# Edit protocols/Cargo.toml and roles/Cargo.toml to add new members

cargo build --workspace
cargo test --lib --workspace
```

**Nothing controversial here.** These are entirely new and don't conflict with SRI code.

---

## Phase 3: Minimal Pool Integration (2-3 hours)

**Goal:** Pool can send quotes to Mint and route responses to Translator.
**Strategy:** Add code, don't modify existing SRI structures.

### 3a. Add extension message handler to Pool

Create `roles/pool/src/lib/mining_pool/extension_message_handler.rs`:

```rust
use mining_sv2::MintQuoteNotification;

pub async fn handle_mint_quote_response(
    pool: &mut Pool,
    response: QuoteResponse,
) -> Result<()> {
    // 1. Build MintQuoteNotification from response
    let notification = MintQuoteNotification {
        channel_id: response.channel_id,
        sequence_number: response.sequence_number,
        share_hash: response.share_hash,
        quote_id: response.quote_id,
        amount: response.amount,
    };

    // 2. Find the downstream that owns this channel
    if let Some(downstream_id) = pool.channel_to_downstream.get(&response.channel_id) {
        // 3. Send extension message to downstream
        pool.send_extension_message_to_downstream(
            *downstream_id,
            response.channel_id,
            &notification,
        ).await?;
    }

    Ok(())
}
```

### 3b. Integrate into Pool message loop

Edit `roles/pool/src/lib/mining_pool/mod.rs`:

Add to Pool struct (4 new fields):
```rust
pub struct Pool {
    // ... existing fields ...

    // NEW: Ehash support
    quote_dispatcher: QuoteDispatcher,
    channel_to_downstream: HashMap<u32, u32>,
    pending_quotes: HashMap<String, PendingQuote>,
    mint_connection: Arc<MintConnection>,
}
```

Add to Pool initialization:
```rust
impl Pool {
    pub async fn new(...) -> Self {
        // ... existing init ...

        // NEW: Initialize ehash
        let mint_connection = Arc::new(
            MintConnection::new(config.mint_url).await?
        );

        Self {
            // ... existing fields ...
            quote_dispatcher: QuoteDispatcher::new(),
            channel_to_downstream: HashMap::new(),
            pending_quotes: HashMap::new(),
            mint_connection,
        }
    }
}
```

Add message handling in main loop:
```rust
// In Pool's message handling loop, add:
Event::MintQuoteResponse(response) => {
    extension_message_handler::handle_mint_quote_response(self, response).await?;
}
```

**Lines added:** ~200
**SRI code modified:** 0 (only inside Pool struct which is hashpool code)

### 3c. Add channel tracking

Edit `roles/pool/src/lib/mining_pool/message_handler.rs` to track channel→downstream mapping:

```rust
// When a downstream opens a channel:
fn handle_open_mining_channel(...) {
    // ... existing code ...

    // NEW: Track channel ownership for quote routing
    self.channel_to_downstream.insert(channel_id, downstream_id);
}

// When a channel closes:
fn handle_close_channel(...) {
    // ... existing code ...

    // NEW: Clean up tracking
    self.channel_to_downstream.remove(&channel_id);
}
```

**Lines added:** ~20
**SRI code modified:** 0

**Validation:**
```bash
cargo build -p pool_sv2
cargo test -p pool_sv2 --lib
```

---

## Phase 4: Minimal Translator Integration (2-3 hours)

**Goal:** Translator can receive extension messages from Pool and route quotes to Wallet.
**Strategy:** Add handler module, don't touch core bridge logic yet.

### 4a. Add extension message handler to Translator

Create `roles/translator/src/lib/upstream_sv2/extension_handler.rs`:

```rust
use mining_sv2::MintQuoteNotification;

pub async fn handle_extension_message(
    translator: &mut Translator,
    message_type: u8,
    payload: &[u8],
) -> Result<()> {
    match message_type {
        // Custom type for MintQuoteNotification
        0xF0 => handle_mint_quote_notification(translator, payload).await,
        _ => Err(format!("Unknown extension message type: {}", message_type)),
    }
}

async fn handle_mint_quote_notification(
    translator: &mut Translator,
    payload: &[u8],
) -> Result<()> {
    // Parse the notification
    let notification: MintQuoteNotification = binary_sv2::from_bytes(&mut &payload[..])?;

    // Find which SV1 miner owns this channel
    if let Some(sv1_connection) = translator.get_sv1_downstream(notification.channel_id) {
        // Send quote to miner (via JSON-RPC or custom message)
        let quote_msg = build_quote_message(&notification);
        sv1_connection.send(quote_msg).await?;
    }

    // Track quote in wallet
    if let Some(wallet) = &translator.wallet {
        wallet.add_pending_quote(
            notification.quote_id,
            notification.amount,
            notification.share_hash,
        ).await?;
    }

    Ok(())
}
```

### 4b. Integrate into Translator upstream message loop

Edit `roles/translator/src/lib/upstream_sv2/upstream.rs`:

```rust
// In upstream message handling, add:
Message::Unknown(type_id, payload) => {
    // Route extension messages
    if is_extension_message(*type_id) {
        extension_handler::handle_extension_message(self, *type_id, payload).await?;
    } else {
        // Unknown message type - ignore or log
        eprintln!("Ignoring unknown message type: {}", type_id);
    }
}

fn is_extension_message(type_id: u8) -> bool {
    type_id >= 0xF0  // Reserve 0xF0+ for extension messages
}
```

### 4c. Add minimal wallet integration

Edit `roles/translator/src/lib/mod.rs`:

```rust
pub struct Translator {
    // ... existing fields ...

    // NEW: Ehash wallet (optional)
    wallet: Option<Arc<Wallet>>,
}

impl Translator {
    pub async fn new(..., wallet_config: Option<WalletConfig>) -> Self {
        let wallet = if let Some(cfg) = wallet_config {
            Some(Arc::new(Wallet::new(cfg).await?))
        } else {
            None
        };

        Self {
            // ... existing fields ...
            wallet,
        }
    }
}
```

**Lines added:** ~300
**SRI code modified:** 0

**Validation:**
```bash
cargo build -p translator
cargo test -p translator --lib
```

---

## Phase 5: Minimal Configuration (1 hour)

Add config support for ehash (no SRI changes):

Create `roles/roles-utils/config/src/lib.rs`:

```rust
pub struct EhashConfig {
    pub enabled: bool,
    pub mint_url: String,
    pub difficulty_multiplier: f64,
}

pub struct Sv2MessagingConfig {
    pub enabled: bool,
    pub mint_listen_address: String,
}

// Parse from TOML
```

Update `config/pool.config.toml`:

```toml
[ehash]
enabled = true
mint_url = "http://127.0.0.1:3012"
difficulty_multiplier = 2.0

[sv2_messaging]
enabled = true
mint_listen_address = "127.0.0.1:34260"
```

**Lines added:** ~150
**SRI code modified:** 0

---

## Phase 6: Minimal Tests & Smoke Test (2-3 hours)

```bash
# Build everything
cargo build --workspace

# Run all unit tests
cargo test --lib --workspace

# Start devenv (even partial)
devenv shell
devenv up

# Verify:
# 1. Pool starts
# 2. Translator starts
# 3. Mint starts
# 4. No fatal errors in logs
# 5. Can connect miner and get shares accepted
```

**Expected:** Shares flow through pool→translator. Mint service ready. No quote flow yet (can add in Phase 2).

---

## What We're DEFERRING to Phase 2

**DO NOT DO in this migration:**

- ❌ Stats snapshots (keep basic logging only)
- ❌ Web dashboards (can use CLI tools)
- ❌ Advanced error handling improvements
- ❌ Performance optimizations
- ❌ Configuration refactoring
- ❌ Tests beyond unit/integration
- ❌ Documentation updates (just ensure code builds)

These can all happen **after** we're on 1.5.0.

---

## Timeline: Days Not Weeks

| Phase | Duration | Output |
|-------|----------|--------|
| Phase 0 | 1 hr | Verify SRI 1.5.0 infrastructure |
| Phase 1 | 30 min | Clean v1.5.0 baseline |
| Phase 2 | 45 min | New hashpool crates added |
| Phase 3 | 3 hrs | Pool integration done |
| Phase 4 | 3 hrs | Translator integration done |
| Phase 5 | 1 hr | Config in place |
| Phase 6 | 2-3 hrs | Smoke test |
| **TOTAL** | **~11 hours over 2-3 days** | **Hashpool on 1.5.0** |

---

## Success Criteria

Minimal viable product succeeds when:

✅ `cargo build --workspace` succeeds
✅ `cargo test --lib --workspace` passes
✅ devenv stack starts without fatal errors
✅ Miner can connect and submit shares
✅ Pool routes shares correctly
✅ No hardcoded SRI code changes (only in hashpool modules)

**NOT required for Phase 1:**
- ❌ Full quote→sweep flow working
- ❌ Web dashboards responsive
- ❌ Stats collection working
- ❌ Advanced reliability features

---

## Git Commits

Keep it simple:

```bash
git add protocols/
git commit -m "Add ehash, mint-quote, stats-sv2 protocols"

git add roles/
git commit -m "Add mint service and utility crates"

git add roles/pool/
git commit -m "Add minimal ehash support to Pool

- Handle mint quote responses
- Route quotes to downstream translators
- Track channel ownership for routing"

git add roles/translator/
git commit -m "Add minimal ehash support to Translator

- Handle extension messages from Pool
- Route quotes to SV1 miners
- Integrate wallet support"

git add roles/roles-utils/config/ config/
git commit -m "Add ehash configuration"

git add .
git commit -m "Add integration tests and validation"
```

---

## Critical: Avoid SRI Code Modifications

**DO NOT:**
- ❌ Modify `roles-logic-sv2` error handling (use wrapper Result type)
- ❌ Modify `Pool` struct from SRI (only add hashpool-specific code)
- ❌ Modify message handlers in SRI core (wrap in new module)
- ❌ Change SRI test files
- ❌ Refactor SRI architecture

**DO:**
- ✅ Add new files/modules for hashpool-specific code
- ✅ Extend SRI types with traits when needed
- ✅ Use composition over modification
- ✅ Keep hashpool code clearly separated

---

## Next: Get on 1.5.0 THEN Improve

Once hashpool works on 1.5.0:

1. Contribute improvements back to SRI
2. Implement proper stats snapshots (with SRI hooks if possible)
3. Build web dashboards
4. Add comprehensive testing
5. Optimize error handling
6. Profile and optimize performance

This way you're **contributing to SRI evolution** instead of patching hashpool in isolation.

---

**Created:** 2025-10-24
**Focus:** Speed, Minimal Footprint, Pragmatism
**Next:** Start Phase 0 verification
