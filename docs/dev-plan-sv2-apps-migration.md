# Dev Plan: Migrate Translator to sv2-apps Architecture

## Context

Hashpool's translator was built on old SRI translator architecture and has accumulated significant
drift from upstream. The goal is to reduce hashpool to the minimum code necessary to run the CDK
payment layer, with everything else living in upstream library dependencies.

**Privacy architecture (invariant):** Quote creation happens at the pool/mint deployment.
The translator is a passive CDK receiver only — it persists quote IDs received via custom SV2
frames (0xC0/0xC1) and runs a sweeper to mint tokens. This boundary must not be crossed:
the translator never talks to the mint directly.

**Strategic direction:**
1. Vendor sv2-apps translator + stratum-apps into hashpool now (unblocks development)
2. Upstream extension points to sv2-apps (contributes back, reduces patch surface)
3. As stratum-apps ships to crates.io, drop vendor → version dep
4. As SRI vendored crates reach crates.io parity, drop `[patch.crates-io]` entries

Target end state: hashpool contains only CDK integration code (~500 lines), everything else
is a library dep.

**All development work for this plan happens on a feature branch.**

---

## What Lives Where (End State)

### In sv2-apps (already upstream)
- CancellationToken + FallbackCoordinator lifecycle
- Job keepalive loop (timestamp-incrementing, per-downstream)
- Sv1ClientsMonitoring trait + HTTP monitoring server
- extensions_message_handler.rs (SV2 extension negotiation)
- Core SV1↔SV2 translation, extranonce management, vardiff

### New in sv2-apps (PR to contribute)
- `CustomMiningMessageHandler` trait — escape hatch for 0xC0–0xFF custom message types
- `NoopCustomMiningMessageHandler` default (zero behavior change for non-hashpool users)

### Stays in hashpool only
- `CdkQuoteNotificationHandler` implementing `CustomMiningMessageHandler`
  (decodes 0xC0 MintQuoteNotification, calls wallet.fetch_mint_quote)
- Quote sweeper (spawn_quote_sweeper, process_stored_quotes)
- Faucet API (faucet_api.rs)
- Hashpool-specific Prometheus gauges (wallet balance, ehash unit)
- CDK wallet init (create_wallet, BIP39 seed)
- Hashpool-specific TranslatorConfig fields (wallet, mint_url, faucet_port, etc.)
- cdk, cdk-sqlite, bip39, axum/hyper deps

---

## Extension Point: `CustomMiningMessageHandler`

The pool sends `MintQuoteNotification` (0xC0) downstream to the translator when a quote is ready.
sv2-apps has no handler for non-standard Mining message types. The correct extension:

```rust
// In stratum-apps crate
#[async_trait]
pub trait CustomMiningMessageHandler: Send + Sync {
    /// Called for Mining message types outside the standard SV2 range.
    /// `msg_type` is the raw message type byte (e.g., 0xC0).
    /// `payload` is the raw frame payload bytes.
    async fn handle_custom_message(
        &self,
        msg_type: u8,
        payload: &[u8],
    ) -> Result<(), Error>;
}

pub struct NoopCustomMiningMessageHandler;
impl CustomMiningMessageHandler for NoopCustomMiningMessageHandler {
    async fn handle_custom_message(&self, _: u8, _: &[u8]) -> Result<(), Error> { Ok(()) }
}
```

`ChannelManager` gets a field `custom_handler: Arc<dyn CustomMiningMessageHandler>` and calls it
from the frame dispatch path when an unknown message type arrives.

Hashpool implements `CdkQuoteNotificationHandler`:

```rust
pub struct CdkQuoteNotificationHandler {
    wallet: Arc<Wallet>,
}

impl CustomMiningMessageHandler for CdkQuoteNotificationHandler {
    async fn handle_custom_message(&self, msg_type: u8, payload: &[u8]) -> Result<(), Error> {
        match msg_type {
            0xC0 => {
                let notification = MintQuoteNotification::decode(payload)?;
                self.wallet.fetch_mint_quote(
                    &notification.quote_id,
                    Some(PaymentMethod::Custom("ehash")),
                ).await?;
            }
            0xC1 => { /* log MintQuoteFailure */ }
            _ => {}
        }
        Ok(())
    }
}
```

Using raw bytes in the handler means the 0xC0/0xC1 variants in vendored `parsers_sv2` are no
longer needed and can be removed, clearing a path toward unmodified crates.io parsers_sv2.

---

## Dependency Strategy: Vendor sv2-apps, Migrate to crates.io Over Time

### Phase A: Vendor (this plan)

1. Copy `sv2-apps/stratum-apps/` → `hashpool/common/stratum-apps/` (workspace member)
2. Copy `sv2-apps/miner-apps/translator/src/` → `hashpool/roles/translator/src/` (replace wholesale)
3. hashpool's `[patch.crates-io]` covers all transitive SRI deps from stratum-apps cleanly
4. Pin vendor to a specific sv2-apps git commit; track updates manually (record commit hash in
   a comment in `roles/translator/Cargo.toml`)

### Phase B: Upstream extensions (in parallel)

File sv2-apps PR: `CustomMiningMessageHandler` trait + `ChannelManager` wiring (~40 lines).
Once merged: remove local trait definition; import from stratum-apps.

### Phase C: crates.io migration (long-term)

| Dependency | Blocker | Action when ready |
|------------|---------|-------------------|
| stratum-apps | sv2-apps maintainers publish | Drop vendor, add version dep |
| parsers_sv2 | Drop custom Mining variants (Phase A removes them) | Drop `[patch.crates-io]` entry |
| channels_sv2 | ValidWithAcknowledgement variant — pool only, not translator | Can drop patch for translator sooner |
| binary_sv2, codec_sv2, etc. | SRI 1.7.0+ on crates.io | Drop patches when version matches |

---

## Migration Steps

### Step 1: Create feature branch

```bash
git checkout -b feat/sv2-apps-translator-migration
```

### Step 2: Vendor stratum-apps

Copy `sv2-apps/stratum-apps/` into `hashpool/common/stratum-apps/`. Add to hashpool's root
workspace `Cargo.toml` as a member. Verify `[patch.crates-io]` redirects cover its SRI deps.

### Step 3: Replace translator source

```bash
cp -r ~/work/sv2-apps/miner-apps/translator/src/ roles/translator/src/
```

Record the sv2-apps commit hash in a comment in `roles/translator/Cargo.toml`.

### Step 4: Update `roles/translator/Cargo.toml`

Remove:
- `cdk`, `cdk-sqlite`, `bip39` (move to payment/ module)
- `mint_quote_sv2`, `shared_config`, `config_helpers_sv2`
- `stratum_translation` (superseded — but first verify `build_sv1_set_difficulty_from_sv2_target`
  SV2 difficulty formula fix is present in sv2-apps equivalent before deleting)

Add:
- `stratum-apps = { path = "../../common/stratum-apps", features = ["translator"] }`
- `tokio-util`, `dashmap` (if not already transitively available)

### Step 5: Add `CustomMiningMessageHandler` trait (interim, in hashpool)

Add to `roles/translator/src/lib/payment/custom_handler.rs` until the sv2-apps PR lands.
Modify `channel_manager.rs` (the one intentional local patch to the vendored source) to wire
`custom_handler: Arc<dyn CustomMiningMessageHandler>` into `ChannelManager`.

### Step 6: Implement `CdkQuoteNotificationHandler`

In `roles/translator/src/lib/payment/cdk_handler.rs`:
- Decodes 0xC0 as MintQuoteNotification, calls `wallet.fetch_mint_quote()`
- Decodes 0xC1 as MintQuoteFailure, logs
- Depends on `mint_quote_sv2` for struct definitions (path dep, not removed)

### Step 7: Remove custom Mining enum variants from vendored parsers_sv2

With raw-bytes custom handling, the 0xC0/0xC1 variants added to vendored `parsers_sv2` are no
longer needed. Remove them. Verify the pool still compiles (pool sends these frames using its own
encoding path).

### Step 8: Port remaining hashpool modules

| Module | Action |
|--------|--------|
| `faucet_api.rs` | Copy to new structure, update imports |
| Quote sweeper | Move to `payment/quote_sweeper.rs` |
| `create_wallet()` | Move to `payment/wallet.rs` |
| Hashpool Prometheus metrics | Extend sv2-apps monitoring hooks |
| Hashpool TranslatorConfig fields | Add to config.rs alongside sv2-apps fields |
| MinerTracker / miner_stats.rs | Evaluate: can `Sv1ClientInfo` replace it? |

### Step 9: Wire hashpool extensions in `TranslatorSv2::start()`

```rust
let wallet = create_wallet(&self.config.wallet).await?;
let custom_handler = Arc::new(CdkQuoteNotificationHandler::new(wallet.clone()));
let channel_manager = ChannelManager::new(..., custom_handler);

// FallbackCoordinator lifecycle contract: every task must call .done() before exiting
let sweeper_handle = fallback_coordinator.register();
task_manager.spawn(QuoteSweeper::spawn(wallet.clone(), sweeper_handle, ...));

let faucet_handle = fallback_coordinator.register();
task_manager.spawn(FaucetApi::spawn(self.config.faucet_port, wallet.clone(), faucet_handle, ...));
```

### Step 10: Build and test

```bash
cargo build -p translator_sv2
devenv up
# Verify:
# - Keepalive notifies fire (logs: "job keepalive loop")
# - 0xC0 frames arrive and wallet.fetch_mint_quote() is called (logs)
# - Sweeper mints tokens after quote payment
# - Faucet API responds on faucet_port
# - /metrics endpoint serves hashpool + sv2-apps metrics
```

### Step 11: File sv2-apps PR

PR: `CustomMiningMessageHandler` trait + `ChannelManager::new()` hook + NoopCustomMiningMessageHandler.
Once merged: import trait from stratum-apps, delete local copy.

### Step 12: Merge feature branch

After smoke test passes and branch is reviewed, merge to master.

---

## Code Reduction Estimate

| Component | Current LOC | After migration |
|-----------|-------------|-----------------|
| translator src/ (sv2 core) | ~3000 | 0 (replaced by sv2-apps) |
| stratum_translation crate | ~800 | 0 (superseded) |
| Custom parsers_sv2 variants | ~20 | 0 (dropped in Step 7) |
| CDK payment layer (new) | 0 | ~500 (hashpool-specific, stays) |
| **Net hashpool code** | | **~-3300 LOC** |

---

## Risk Assessment

**Low risk:**
- sv2-apps translator is in production use
- Keepalive already more correct than hashpool's current approach
- Vendor strategy avoids Cargo version conflict entirely
- Feature branch isolates the work until proven stable

**Medium risk:**
- `build_sv1_set_difficulty_from_sv2_target` SV2 difficulty formula fix must be confirmed
  present in sv2-apps's equivalent before dropping `stratum_translation` (a regression here
  would cause invalid share spam — difficulty would compute as ~0.000778 instead of correct value)
- FallbackCoordinator lifecycle contract for sweeper/faucet — every task must call `.done()` on
  all exit paths or reconnect cycle will deadlock
- MinerTracker replacement: verify Sv1ClientInfo fields are sufficient for per-miner metrics
  before deleting `miner_stats.rs`

**Mitigation:** Full devenv smoke test with a real miner on the feature branch before merge.
