# Hashpool Translator Migration: Upgrade to sv2-apps v0.3.0 + Monitoring Fix

## Context

The `feat/sv2-apps-translator-migration` branch replaced the old hashpool translator with
the upstream sv2-apps translator, plus a thin CDK payment layer. However, the vendored
`common/stratum-apps/` is pinned to commit `4014d48d` (2026-03-11), 8 days before the
sv2-apps v0.3.0 stable release (2026-03-19). That release introduced breaking API changes.

Additionally, commit `092a7666` applied a hacky partial monitoring fix (hardcoded IP,
hardcoded 1.0 shares/minute, no per-miner tracking) that needs to be properly replaced.

**Goal:** Reach full feature parity with the old hashpool codebase on a clean, maintainable
foundation that is easily upgradeable to future sv2-apps releases.

## Decision: Upgrade In Place (Not Replay)

**Rationale for not replaying from scratch:**
- The CDK payment layer (`payment/`, `faucet_api.rs`) is correct and well-designed
- The `common/stratum-apps/` vendor strategy is sound
- The delta from `4014d48d` to v0.3.0 is small (8 days, identified concretely below)
- Replaying wastes effort and loses meaningful git history

## Breaking Changes: 4014d48d → v0.3.0

### In `stratum-apps` library (affects our vendored copy):
1. `ServerExtendedChannelInfo.shares_accepted` → `shares_acknowledged` + new `shares_rejected: u32`
2. `stratum-core` dependency moved from git branch → crates.io v0.2.1
3. CoinbaseOutputConstraints: new `_with_offset()` variant for P2TR safety margins

### In translator source (affects our fork at `roles/translator/src/`):
1. **Config API**: `monitoring_cache_refresh_secs: u64` → `Option<u64>`; constructor gains
   two new params: `monitoring_address: Option<SocketAddr>`, `monitoring_cache_refresh_secs: Option<u64>`
2. **Monitoring fields**: `shares_accepted` → `shares_acknowledged`; `shares_submitted` now
   uses `share_accounting.get_validated_shares()` (not manual counter); new `shares_rejected`
3. **Share rejection tracking**: `mining_message_handler.rs` now calls `ch.on_share_rejection()`
   in `handle_share_error()`

## Implementation Plan

### Phase 1: Revert the hacky monitoring commit

```
git revert 092a7666
```

This removes the hardcoded IP, hardcoded `1.0` shares/minute, and other shortcuts.
The division-by-zero fix it contained will be done properly in Phase 3.

### Phase 2: Upgrade vendored stratum-apps to v0.3.0

**Files changed:**
- `common/stratum-apps/` — wholesale replace with `~/work/sv2-apps/stratum-apps/` at v0.3.0

**Steps:**
1. Copy `~/work/sv2-apps/stratum-apps/` → `common/stratum-apps/` (replace entire directory)
2. Update the version comment in `roles/translator/Cargo.toml` to note `v0.3.0`

**Cargo patch section review:**
v0.3.0 stratum-apps uses `stratum-core = "0.2.1"` from crates.io (was git branch).
To avoid duplicate crate instances, add stratum-core to `[patch.crates-io]` in
`roles/Cargo.toml`, pointing to `../../stratum/stratum-core`, consistent with the
existing git patch. This ensures all crates in the workspace compile against the same
modified stratum-core binary.

### Phase 3: Sync translator source with v0.3.0 delta

Apply these targeted changes to `roles/translator/src/` to match v0.3.0:

**File: `roles/translator/src/lib/config.rs`**
- Change `monitoring_cache_refresh_secs: u64` → `Option<u64>` (serde default = None)
- Update `monitoring_cache_refresh_secs()` accessor return type → `Option<u64>`
- Add `monitoring_address: Option<SocketAddr>` and `monitoring_cache_refresh_secs: Option<u64>`
  parameters to `TranslatorConfig::new()` constructor
- Update all call sites in `mod.rs` and tests to pass `None, None`
- Update `monitoring_cache_refresh_secs()` usage in `mod.rs`:
  `.unwrap_or(15)` at the call site

**File: `roles/translator/src/lib/sv2/channel_manager/mining_message_handler.rs`**
- In `handle_share_error()`: call `ch.on_share_rejection()` after logging the rejection

**File: `roles/translator/src/lib/monitoring.rs`**
- Rename field: `shares_accepted` → `shares_acknowledged`
- Change `shares_submitted` source: use `share_accounting.get_validated_shares()` (remove
  manual `share_sequence_counters` tracking if it was carried forward)
- Add `shares_rejected: share_accounting.get_rejected_shares()`

### Phase 4: Verify build

```bash
cargo build --workspace  # in roles/
```

Fix any compilation errors from the above changes before proceeding.

### Phase 5: Implement proper proxy monitoring

**`roles/translator/src/lib/sv1/downstream/data.rs`**
- Add `shares_submitted: u64`, `connected_at: SystemTime`, `peer_address: Option<SocketAddr>`
- Add `WindowedMetricsCollector` for rolling hashrate calculation

**`roles/translator/src/lib/sv1/sv1_server/sv1_server.rs`**
- Capture `peer_addr` at TCP accept and store in `DownstreamData`
- Track `shares_submitted` increment in `handle_submit_shares`
- Use windowed metrics for hashrate derivation (no more hardcoded 1.0)
- Remove hardcoded `monitoring_api_url`; read from config

**`common/stratum-apps/src/monitoring/sv1.rs`** (vendor modification — to be upstreamed)
- Extend `Sv1ClientInfo` struct: add `shares_submitted: u64`, `connected_at_secs: u64`,
  `peer_address: Option<String>`, `hashrate_5min: Option<f64>`

**`roles/translator/src/lib/sv1_monitoring.rs`**
- Populate the new `Sv1ClientInfo` fields from `DownstreamData`
- Compute hashrate from windowed metrics using SV2 formula (`2^256 / target * shares / window_secs`)
- Apply `redact_ip` config flag before setting `peer_address`

**`roles/translator/src/lib/faucet_api.rs`**
- Add `GET /balance` endpoint returning current ehash wallet balance as JSON

**`roles/translator/src/lib/config.rs`**
- Add `monitoring_api_url: String` under `[translator]` section
- Remove any remaining hardcoded URLs

**`roles/web-proxy/src/`**
- Fetch miners list from translator REST API (`/api/v1/sv1/clients`) instead of scraping metrics
- Fetch ehash balance from faucet API (`/balance`)
- Update health check to use stratum-apps `/health` endpoint

**`config/shared/miner.toml` (or equivalent)**
- Add `monitoring_api_url = "http://127.0.0.1:9109"` under `[translator]`
- Remove any hardcoded addresses from source code

### Phase 6: End-to-end smoke test

```bash
# Build
cargo build --workspace  # in roles/

# Integration
devenv up
# Verify in web-proxy UI:
# - Miners page: name, windowed hashrate, shares submitted, connected time, peer IP
# - Balance page: current ehash wallet balance (non-zero after mining)
# - Health check: green
```

## Critical Files

| File | Change |
|------|--------|
| `common/stratum-apps/` | Replace wholesale with v0.3.0 |
| `roles/Cargo.toml` | Add stratum-core to `[patch.crates-io]` |
| `roles/translator/Cargo.toml` | Update version comment |
| `roles/translator/src/lib/config.rs` | monitoring_cache_refresh_secs → Option<u64>, 2 new ctor params |
| `roles/translator/src/lib/monitoring.rs` | shares_acknowledged, shares_rejected, validated |
| `roles/translator/src/lib/sv2/channel_manager/mining_message_handler.rs` | on_share_rejection() |
| `roles/translator/src/lib/sv1/downstream/data.rs` | shares_submitted, connected_at, peer_address, windowed metrics |
| `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs` | capture peer addr, windowed hashrate |
| `common/stratum-apps/src/monitoring/sv1.rs` | add hashpool monitoring fields (upstream candidate) |
| `roles/translator/src/lib/sv1_monitoring.rs` | populate new Sv1ClientInfo fields |
| `roles/translator/src/lib/faucet_api.rs` | add GET /balance |
| `roles/translator/src/lib/config.rs` | add monitoring_api_url |
| `roles/web-proxy/src/` | use REST API + faucet balance |
| `config/shared/miner.toml` | add monitoring_api_url |

## Long-Term Upgrade Path (not in scope now)

- **Upstream `CustomMiningMessageHandler` trait to sv2-apps** → delete `payment/custom_handler.rs`,
  import from stratum-apps
- **Upstream `Sv1ClientInfo` extensions to sv2-apps** → vendor modifications come back as
  upstream API
- **When sv2-apps publishes to crates.io** → drop `common/stratum-apps/`, use version dep
  (`stratum-apps = "0.4.0"`)
- **When CDK 0.16.0 ships** → drop all `[patch]` sections for CDK, bump versions,
  publish `cdk-ehash`
