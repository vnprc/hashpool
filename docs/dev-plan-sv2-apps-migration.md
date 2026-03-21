# Dev Plan: Migrate Translator to sv2-apps Architecture

## Motivation

Hashpool's translator was built on the old SRI translator architecture (roles/translator in the
stratum repo). SRI has since moved roles out into the `stratum-mining/sv2-apps` repo and improved
the implementation. The new translator:

- **Has job keepalive** — already implemented (more correctly than a naive re-broadcast),
  better reconnect plumbing via `CancellationToken` + `FallbackCoordinator`
- **Has cleaner shutdown semantics** — `CancellationToken` replaces broadcast `ShutdownMessage`
  channels; `FallbackCoordinator` provides explicit cleanup acknowledgement before reinit
- **Has a monitoring extension trait** — `Sv1ClientsMonitoring` already factored out as a
  trait implemented on `Sv1Server`
- **Has SV2 extension negotiation** — `extensions_message_handler.rs`

The cascade disconnect issue (upstream reconnect drops all miners) is **present in sv2-apps too**
and is tracked separately. This migration does not fix it, but the sv2-apps plumbing makes fixing
it substantially cleaner.

---

## Strategy

Replace `roles/translator/src/` wholesale with the sv2-apps translator source, then layer in
hashpool-specific logic via:

1. **Trait extension points** added to sv2-apps as a PR (share payment hook, worker name hook)
2. **Hashpool-specific implementations** of those traits, kept in hashpool

This is a net deletion of code in hashpool. The sv2-apps translator already covers:
- Job keepalive loop (per-downstream, timestamp-incrementing, keepalive job_id tracking)
- Monitoring trait + HTTP server
- `Sv1ClientsMonitoring` trait on `Sv1Server`

---

## What Lives Where After Migration

### Stays in sv2-apps (upstream)
- `CancellationToken` shutdown plumbing
- `FallbackCoordinator` reconnect coordination
- Job keepalive loop (`spawn_job_keepalive_loop`, keepalive job_id encoding)
- `Sv1ClientsMonitoring` trait + HTTP monitoring server
- `extensions_message_handler.rs` (SV2 extension negotiation)
- Core SV1↔SV2 translation logic

### New in sv2-apps (PR to contribute)
- `SharePaymentProcessor` trait (see §3 below)
- Hook call site in `handle_submit_shares_success`
- Worker name hook call in `handle_authorize`

### Stays in hashpool only
- `CdkPaymentProcessor` implementing `SharePaymentProcessor`
- Quote sweeper (`spawn_quote_sweeper`, `process_stored_quotes`)
- Faucet API (`faucet_api.rs`)
- Hashpool-specific Prometheus metrics (wallet balance, ehash unit)
- CDK wallet init (`create_wallet`, BIP39 seed)
- `cdk`, `cdk-sqlite`, `bip39`, `axum`/`hyper` deps

## Keepalive: sv2-apps Implementation

sv2-apps has a more correct keepalive than a naive re-broadcast:
- Tracks `last_job_received_time` **per downstream**
- **Increments the block timestamp** by `keepalive_interval_secs` on each keepalive
- Caps timestamp at `base_time + 2h` (Bitcoin's `MAX_FUTURE_BLOCK_TIME`)
- Assigns a new `job_id = "{original}#{counter}"` so shares submitted against keepalive jobs
  are routed back to the correct upstream job
- Stores keepalive jobs in `valid_sv1_jobs` for share validation
- Configurable via `job_keepalive_interval_secs` in `DownstreamDifficultyConfig`

A naive re-broadcast of the exact same notify may not reset the stale-work timer on all
firmware — some miners check whether the notify changed. The sv2-apps approach handles this.

---

## Extension Points to Add to sv2-apps

### `SharePaymentProcessor` trait

The only hashpool-specific hook needed in the channel manager is: "a valid share was accepted
by the upstream pool, trigger payment logic."

```rust
// In stratum-apps crate or sv2-apps translator as an optional trait

/// Optional hook called when the upstream pool acknowledges a share submission.
/// Implementors use this to trigger payment accounting (e.g., mint quote creation).
#[async_trait]
pub trait SharePaymentProcessor: Send + Sync {
    /// Called when `SubmitSharesSuccess` is received from upstream.
    ///
    /// `channel_id` — the upstream extended channel ID
    /// `new_shares_sum` — difficulty-weighted share count from this acknowledgement
    async fn on_shares_accepted(&self, channel_id: u32, new_shares_sum: f64);
}

/// No-op default for deployments without payment processing.
pub struct NoopPaymentProcessor;

#[async_trait]
impl SharePaymentProcessor for NoopPaymentProcessor {
    async fn on_shares_accepted(&self, _channel_id: u32, _new_shares_sum: f64) {}
}
```

Hook site in `mining_message_handler.rs::handle_submit_shares_success`:

```rust
async fn handle_submit_shares_success(&mut self, ..., m: SubmitSharesSuccess, ...) {
    // ... existing share accounting ...
    ch.on_share_acknowledgement(m.new_submits_accepted_count, m.new_shares_sum as f64);

    // Payment hook (no-op by default)
    self.payment_processor
        .on_shares_accepted(m.channel_id, m.new_shares_sum as f64)
        .await;
}
```

`ChannelManager` gains a field:
```rust
pub struct ChannelManager {
    // ... existing fields ...
    payment_processor: Arc<dyn SharePaymentProcessor>,
}
```

### Worker name hook in `handle_authorize`

The miner tracker needs to record the worker name when `mining.authorize` is received. This
maps naturally to an optional callback on `Sv1Server`.

```rust
// Already partially covered by Sv1ClientsMonitoring — extend Sv1ClientInfo to include
// authorized_worker_name (already present in the sv2-apps struct).
// The hook fires when authorize succeeds; sv2-apps handle_authorize already returns bool.
// No additional trait needed — just update Sv1ClientInfo.authorized_worker_name
// in the existing downstream_data when authorize succeeds.
```

sv2-apps's `handle_authorize` already sets the worker name in `DownstreamData`. The
`Sv1ClientsMonitoring::get_sv1_clients()` implementation in `sv1_monitoring.rs` already
exposes it. **No additional hook needed here** — hashpool's miner tracker can be driven
from `get_sv1_clients()` polling rather than a push callback.

---

## Migration Steps

### Step 1: Replace translator source with sv2-apps

Copy sv2-apps translator into hashpool:
```
cp -r ~/work/sv2-apps/miner-apps/translator/src/ roles/translator/src/
```

This brings in:
- `CancellationToken` + `FallbackCoordinator` shutdown plumbing
- sv2-apps job keepalive
- `Sv1ClientsMonitoring` monitoring trait
- Extensions handler
- `monitoring` feature flag (Axum HTTP monitoring server)

### Step 3: Update `roles/translator/Cargo.toml`

Remove hashpool-specific deps that are no longer in the translator module:
- `cdk`, `cdk-sqlite`, `bip39` (move to hashpool-specific extension module)
- `mint_quote_sv2`, `shared_config`, `config_helpers_sv2`

Add sv2-apps deps:
- `stratum-apps` (the shared library crate from sv2-apps)
- `tokio-util` (for `CancellationToken`)
- `dashmap` (sv2-apps uses it for concurrent downstreams map)

### Step 4: Add `SharePaymentProcessor` trait

**Option A (preferred):** Contribute the trait to sv2-apps as a PR. `ChannelManager::new()`
accepts `Arc<dyn SharePaymentProcessor>` with a default of `Arc::new(NoopPaymentProcessor)`.

**Option B (interim):** Define the trait in a new hashpool crate
`protocols/translator-extensions/` and use a feature flag to inject it.

For the initial migration, use Option B to avoid blocking on upstream PR review. File the
sv2-apps PR in parallel; once merged, switch to Option A.

### Step 5: Implement `CdkPaymentProcessor`

In a new hashpool module `roles/translator/src/lib/payment/`:

```rust
pub struct CdkPaymentProcessor {
    wallet: Arc<Wallet>,
    channel_manager_data: Arc<Mutex<ChannelManagerData>>,
    locking_privkey: SecretKey,
}

#[async_trait]
impl SharePaymentProcessor for CdkPaymentProcessor {
    async fn on_shares_accepted(&self, channel_id: u32, new_shares_sum: f64) {
        // Existing logic from handle_submit_shares_success:
        // - Look up pending mint quote for this channel
        // - Accumulate shares_sum
        // - Mark quote ready for sweeping
    }
}
```

### Step 6: Port remaining hashpool-specific modules

These modules move largely unchanged into the new structure:

| Module | Action |
|--------|--------|
| `faucet_api.rs` | Copy as-is, update imports |
| Quote sweeper logic | Move into `payment/quote_sweeper.rs` |
| `create_wallet()` | Move into `payment/wallet.rs` |
| Hashpool Prometheus metrics | Extend `monitoring.rs` with ehash-specific gauges |
| Hashpool `TranslatorConfig` fields | Merge into sv2-apps `TranslatorConfig` (wallet, mint, faucet_port, etc.) |

### Step 7: Update `TranslatorSv2::start()` to wire hashpool extensions

```rust
// Hashpool additions to sv2-apps start():
let wallet = TranslatorSv2::create_wallet(&self.config.wallet).await?;
let payment_processor = Arc::new(CdkPaymentProcessor::new(wallet.clone(), ...));

// Inject into ChannelManager::new() (Step 4)
let channel_manager = Arc::new(ChannelManager::new(
    ...,
    payment_processor,
));

// Spawn hashpool background tasks
task_manager.spawn(QuoteSweeper::spawn(wallet.clone(), ...));
task_manager.spawn(FaucetApi::spawn(self.config.faucet_port, wallet.clone(), ...));
```

### Step 8: Update monitoring

sv2-apps already has `Sv1ClientsMonitoring` trait + HTTP server. Extend it with:
- Wallet balance gauge (`hashpool_translator_wallet_balance_ehash`)
- Ehash-specific per-miner labels

Implement by adding hashpool-specific metrics alongside the existing sv2-apps metrics route.

### Step 9: Build and test

```bash
cargo build -p translator_sv2
devenv up
# Wait for first block, verify:
# - Keepalive notifies fire (check logs for "job keepalive loop")
# - Shares accepted → mint quotes created
# - Faucet API responds
# - /metrics endpoint serves hashpool + sv2-apps metrics
```

---

## sv2-apps PR Scope

File a PR to `stratum-mining/sv2-apps` with:

1. `SharePaymentProcessor` trait definition in `stratum-apps/src/payment.rs`
2. `ChannelManager::new()` accepts `Arc<dyn SharePaymentProcessor>`
3. Hook call in `handle_submit_shares_success`
4. `NoopPaymentProcessor` as default (zero behavior change for non-hashpool users)

This is ~60 lines of code. No behavior change for existing users. Frame it as:
"Optional payment processing hook for pools that need to trigger payment accounting on
confirmed share submissions."

---

## Files Changed Summary

| File | Action |
|------|--------|
| `roles/translator/src/` | Replace wholesale with sv2-apps source |
| `roles/translator/src/lib/payment/mod.rs` | New: CdkPaymentProcessor, QuoteSweeper |
| `roles/translator/src/lib/faucet_api.rs` | Port from current hashpool (minimal changes) |
| `roles/translator/Cargo.toml` | Swap deps (drop cdk from here, add stratum-apps) |
| sv2-apps PR | SharePaymentProcessor trait + hook |

---

## What This Does NOT Fix

- **Cascade disconnect on upstream reconnect** — tracked in `docs/dev-plan-upstream-reconnect.md`.
  The sv2-apps plumbing makes fixing it cleaner (CancellationToken scoping) but does not fix it.
  Address as a follow-on after this migration lands.

---

## Risk Assessment

**Low risk:**
- sv2-apps and hashpool already share the same architecture — this is a code swap, not a
  design change
- sv2-apps translator is in production use (sv2-apps integration tests pass)
- Keepalive is included and more correct than a naive re-broadcast

**Medium risk:**
- CDK wiring changes due to new `TranslatorSv2::start()` structure — needs careful porting
- `stratum-apps` crate version pinning in Cargo.toml (depends on sv2-apps release cadence)

**Mitigation:** Do the migration on a branch, run full `devenv up` smoke test with a real miner
before merging.
