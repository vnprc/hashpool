# Phase 2: Port Features from Master to SRI 1.5.0

**Objective:** Add back all features from hashpool master into the `migrate/sri-1.5.0` branch
**Duration:** 1-2 weeks
**Approach:** Copy code from master branch, adapt to 1.5.0 APIs
**Focus:** Functionality only - no refactoring, comprehensive testing, or docs

---

## Starting Point

You've completed Phase 1 with:
- ✅ Branch `migrate/sri-1.5.0` created from SRI v1.5.0
- ✅ Minimal hashpool integration (Pool + Translator + Mint) working
- ✅ Miner can connect and submit shares
- ✅ devenv stack running without fatal errors

Phase 2 adds all the additional features from master by selectively copying code.

**What You'll Add:** Stats, web dashboards, wallet, config system, remaining utilities
**What You'll Skip:** Tests, docs, benchmarking (Phase 3 material)

---

## Phase 2a: Quote-to-Sweep Wallet Integration (2-3 days)

### 2a.1: Copy Wallet Code from Master

Get the wallet implementation from current master:

```bash
git show master:roles/translator/src/lib/wallet.rs > /tmp/wallet-ref.rs
```

Integrate into SRI 1.5.0 translator:

```bash
# Add wallet module to translator
cp /tmp/wallet-ref.rs roles/translator/src/lib/wallet.rs

# Add to mod.rs
echo "mod wallet;" >> roles/translator/src/lib/mod.rs
```

Add to Translator struct:

```rust
pub struct Translator {
    // ... existing fields ...
    pub wallet: Option<Arc<Wallet>>,
}
```

Initialize in constructor:

```rust
impl Translator {
    pub async fn new(...) -> Self {
        let wallet = if let Some(config) = wallet_config {
            Some(Arc::new(Wallet::new(config).await?))
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

**Work:** Copy `wallet.rs`, add 4 lines to struct and constructor

### 2a.2: Copy Wallet Poller from Master

```bash
git show master:roles/translator/src/lib/wallet_poller.rs > roles/translator/src/lib/wallet_poller.rs
echo "mod wallet_poller;" >> roles/translator/src/lib/mod.rs
```

Spawn in Translator main:

```rust
// In Translator initialization
if let Some(wallet) = &self.wallet {
    let poller = Arc::new(WalletPoller::new(wallet.clone(), Duration::from_secs(5)));

    let poller_clone = poller.clone();
    tokio::spawn(async move {
        if let Err(e) = poller_clone.run().await {
            eprintln!("Wallet poller error: {:?}", e);
        }
    });
}
```

**Work:** Copy file, add spawn logic

### 2a.3: Copy Extension Message Handler from Master

```bash
git show master:roles/translator/src/lib/upstream_sv2/extension_handler.rs > roles/translator/src/lib/upstream_sv2/extension_handler.rs
```

Add to upstream message loop:

```rust
// In upstream message handling
Message::Unknown(type_id, payload) => {
    if let Err(e) = extension_handler::handle_extension_message(self, *type_id, payload).await {
        eprintln!("Extension message error: {:?}", e);
    }
}
```

**Work:** Copy file, add 3 lines to message handler

### 2a.4: Copy Pool Quote Handling from Master

```bash
git show master:roles/pool/src/lib/mining_pool/extension_message_handler.rs > roles/pool/src/lib/mining_pool/extension_message_handler.rs
```

Add channel tracking to Pool struct:

```rust
pub struct Pool {
    // ... existing fields ...
    pub channel_to_downstream: HashMap<u32, u32>,
    pub pending_quotes: HashMap<String, PendingQuote>,
}
```

Update channel open/close handlers to track ownership:

```rust
// In handle_open_mining_channel
self.channel_to_downstream.insert(channel_id, downstream_id);

// In handle_close_channel
self.channel_to_downstream.remove(&channel_id);
```

Add quote request in share handler:

```rust
// In handle_submit_shares_standard
if pool.config.ehash.enabled {
    if let Err(e) = pool.request_quote(&message).await {
        warn!("Quote request failed: {:?}", e);
    }
}
```

Add quote response handling:

```rust
// In main message loop
Event::MintQuoteResponse(response) => {
    extension_message_handler::handle_mint_quote_response(self, response).await?;
}
```

**Work:** Copy file, add 4-5 line changes to Pool

---

## Phase 2b: Stats Integration (2-3 days)

### 2b.1: Copy Pool Stats from Master

```bash
git show master:roles/pool/src/lib/stats_integration.rs > roles/pool/src/lib/stats_integration.rs
```

Add StatsSnapshotProvider impl to Pool:

```rust
// Add to roles/pool/src/lib/mod.rs
pub mod stats_integration;

// In stats_integration.rs, implement trait
impl StatsSnapshotProvider for Pool {
    fn get_snapshot(&self) -> PoolSnapshot {
        // Collect current state and return snapshot
    }
}
```

### 2b.2: Copy Translator Stats from Master

```bash
git show master:roles/translator/src/lib/stats_integration.rs > roles/translator/src/lib/stats_integration.rs
```

Add StatsSnapshotProvider impl to Translator:

```rust
impl StatsSnapshotProvider for Translator {
    fn get_snapshot(&self) -> ProxySnapshot {
        // Collect current state and return snapshot
    }
}
```

### 2b.3: Copy Stats Services from Master

```bash
# Stats-pool service
git checkout master -- roles/stats-pool/

# Stats-proxy service
git checkout master -- roles/stats-proxy/

# Update roles/Cargo.toml to include these members
```

Update `roles/Cargo.toml`:

```toml
[workspace]
members = [
    # ... existing members ...
    "stats-pool",
    "stats-proxy",
]
```

**Work:** Copy 2 services, add 3 members to Cargo.toml

---

## Phase 2c: Web Dashboards (2-3 days)

### 2c.1: Copy Web Services from Master

```bash
git checkout master -- roles/web-pool/
git checkout master -- roles/web-proxy/
```

Update `roles/Cargo.toml`:

```toml
[workspace]
members = [
    # ... existing members ...
    "web-pool",
    "web-proxy",
]
```

Verify HTML assets are included:

```bash
# Check assets directory
ls -la roles/web-pool/assets/
ls -la roles/web-proxy/assets/
```

**Work:** Copy 2 services, update Cargo.toml

---

## Phase 2d: Configuration (1-2 days)

### 2d.1: Copy Config Utilities from Master

```bash
git checkout master -- roles/roles-utils/config/
```

Update `roles/Cargo.toml`:

```toml
[workspace]
members = [
    # ... existing members ...
    "roles-utils/config",
]
```

### 2d.2: Copy Config Files from Master

```bash
git checkout master -- config/pool.config.toml
git checkout master -- config/tproxy.config.toml
git checkout master -- config/mint.config.toml
git checkout master -- config/jds.config.toml
git checkout master -- config/jdc.config.toml
git checkout master -- config/shared/
```

Update Pool initialization to load config:

```rust
let config = PoolConfig::from_toml_file("config/pool.config.toml")?;
config.validate()?;
```

Same for Translator:

```rust
let config = TranslatorConfig::from_toml_file("config/tproxy.config.toml")?;
config.validate()?;
```

**Work:** Copy utility crate and config files, add 2-3 lines to initializers

---

## Phase 2e: Supporting Utilities (1-2 days)

### 2e.1: Copy Mint-Pool Messaging

```bash
git checkout master -- roles/roles-utils/mint-pool-messaging/
```

Update `roles/Cargo.toml`:

```toml
members = [
    # ... existing members ...
    "roles-utils/mint-pool-messaging",
]
```

### 2e.2: Copy Quote Dispatcher

```bash
git checkout master -- roles/roles-utils/quote-dispatcher/
```

Update `roles/Cargo.toml`:

```toml
members = [
    # ... existing members ...
    "roles-utils/quote-dispatcher",
]
```

### 2e.3: Copy Web Assets

```bash
git checkout master -- roles/roles-utils/web-assets/
```

Update `roles/Cargo.toml`:

```toml
members = [
    # ... existing members ...
    "roles-utils/web-assets",
]
```

**Work:** Copy 3 utility crates, update Cargo.toml

---

## Phase 2f: Mint Service (1 day)

### 2f.1: Copy Mint Role from Master

```bash
git checkout master -- roles/mint/
```

Update `roles/Cargo.toml`:

```toml
members = [
    # ... existing members ...
    "mint",
]
```

Verify it builds:

```bash
cargo build -p mint
```

**Work:** Copy service, verify build

---

## Phase 2g: devenv & Integration (1-2 days)

### 2g.1: Copy devenv Configuration

```bash
git checkout master -- .devenv/
git checkout master -- flake.nix
git checkout master -- justfile
```

### 2g.2: Copy Protocol Tests

```bash
git checkout master -- roles/tests-integration/
```

Update `roles/Cargo.toml`:

```toml
members = [
    # ... existing members ...
    "tests-integration",
]
```

### 2g.3: Full Build and Basic Smoke Test

```bash
cargo build --workspace
cargo check --all

# Start devenv
devenv shell
devenv up

# In another terminal:
tail -f logs/pool.log
tail -f logs/translator.log
tail -f logs/mint.log
```

Verify:
- All services start without fatal errors
- Pool and translator connect
- Mint service is ready
- No panic messages in logs

**Work:** Copy configs, run build and basic verification

---

## Implementation Sequence

Follow this order for each phase:

1. Copy files from master using `git show` or `git checkout`
2. Add to `Cargo.toml` workspace members (if needed)
3. Build incrementally: `cargo build -p <crate>`
4. If compilation errors, adapt code (usually just imports or API changes)
5. Move to next phase

---

## Expected Issues & Quick Fixes

### Issue: Compile errors on import paths

**Solution:** Check if import paths changed in SRI 1.5.0
```bash
# Find what changed
git diff master..HEAD -- roles-logic-sv2/src/lib.rs | grep "^[+-].*pub\|^[+-].*use"
```

Update imports accordingly.

### Issue: Trait bounds don't match

**Solution:** Check trait signatures in SRI 1.5.0
```bash
cd /home/evan/work/stratum
grep -A5 "pub trait StatsSnapshotProvider" protocols/
```

Update your impl to match exact signature.

### Issue: Message types don't exist

**Solution:** Check if message type was renamed or moved
```bash
grep -r "MintQuoteNotification\|NewMessage" roles/
```

Use `git show` to compare old definition with what's in 1.5.0.

### Issue: Dependencies missing

**Solution:** Update Cargo.toml with correct versions

```bash
# Check what version is available in 1.5.0
cd /home/evan/work/stratum
grep "cdk = " Cargo.lock
```

Update hashpool's Cargo.lock if needed.

---

## Validation at Each Phase

After each phase, verify:

```bash
# Compile check
cargo check -p <phase_crates>

# Build
cargo build -p <phase_crates>

# If tests exist
cargo test --lib -p <phase_crates>
```

**Stop if compilation fails** and debug before proceeding to next phase.

---

## Git Workflow

Keep commits small and focused:

```bash
git add roles/translator/src/lib/wallet.rs
git commit -m "Port wallet code from master"

git add roles/translator/src/lib/wallet_poller.rs roles/translator/src/lib/mod.rs
git commit -m "Port wallet poller and integrate into translator"

git add roles/pool/src/lib/mining_pool/extension_message_handler.rs
git commit -m "Port pool quote handling from master"

# ... one commit per logical feature ...

git add roles/Cargo.toml
git commit -m "Add stats, web, and utility crates to workspace"

git add .devenv/ flake.nix justfile
git commit -m "Copy devenv configuration from master"
```

---

## Done When

You're done when:

✅ `cargo build --workspace` succeeds
✅ `cargo check --all-targets` passes
✅ All role binaries build: pool, translator, mint, jd-server, jd-client, stats-pool, stats-proxy, web-pool, web-proxy
✅ devenv starts without fatal errors
✅ Miner can connect and submit shares
✅ Pool accepts shares
✅ Web dashboards load and display something

**Not required:**
- Tests passing (can have issues during porting)
- Warnings gone (can address later)
- All features fully working (some may need 1.5.0 API adjustments)

---

## Debugging Strategy

If something doesn't work:

1. **Check compilation errors first** - Fix these before moving on
2. **Check logs** - devenv logs usually show what's wrong
3. **Compare with master** - Look at what changed in 1.5.0
4. **Adapt code** - Usually just API changes, not logic changes
5. **Move on** - If it's a minor feature, defer to later

Don't get stuck on any one thing. If a feature is problematic, comment it out and come back to it later.

---

## Timeline

| Phase | Duration | Output |
|-------|----------|--------|
| 2a | 2-3 days | Quote-to-sweep working |
| 2b | 2-3 days | Stats snapshots working |
| 2c | 2-3 days | Web dashboards working |
| 2d | 1-2 days | Configuration integrated |
| 2e | 1-2 days | Utility crates added |
| 2f | 1 day | Mint service ported |
| 2g | 1-2 days | devenv and integration working |
| **TOTAL** | **1-2 weeks** | **Full feature parity** |

---

## Phase 2 Complete: Feature Parity Achieved

When Phase 2 passes:
- ✅ All roles compile and build
- ✅ devenv stack starts without fatal errors
- ✅ Pool connects to mint
- ✅ Translator connects to pool
- ✅ Miners can submit shares and get quotes
- ✅ Web dashboards accessible and displaying data
- ✅ Stats services running and exporting metrics
- ✅ Wallet sweeping quotes to tokens
- ✅ Full feature parity with master

You now have **hashpool fully running on SRI 1.5.0** with all current functionality.

---

## Next Steps

After Phase 2, consider Phase 3 (docs/SRI-1.5.0-migration/PHASE_3.md) for:
- Enhanced testing and validation
- Performance profiling
- Production deployment readiness

