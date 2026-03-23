# Dev Plan: stratum-apps Cleanup

**Branch:** `feat/sv2-apps-clean`
**Created:** 2026-03-22

## Goal

Eliminate the vendored `common/stratum-apps/` directory, remove dead Prometheus code paths
from `web-proxy`, and identify what should be upstreamed to SRI. Minimize hashpool-specific
code while maintaining a stable base.

---

## Findings

### 1. stratum-apps 0.3.0 is already on crates.io

Published 2026-03-19. The vendored `common/stratum-apps/` differs from the published crate
in **exactly two files**:

| File | Change |
|------|--------|
| `src/monitoring/sv1.rs` | 4 extra fields on `Sv1ClientInfo`: `shares_submitted`, `connected_at_secs`, `peer_address`, `hashrate_5min` (to be renamed `nominal_hashrate`) |
| `src/key_utils/mod.rs` | Re-export of local `key-utils` crate to unify `Secp256k1PublicKey` types across the workspace |

Everything else is unmodified upstream code.

### 2. Prometheus paths in web-proxy are dead code

The translator no longer emits Prometheus metrics directly. The stratum-apps monitoring
server at port 9109 exposes `/metrics` for Prometheus scraping AND a REST API for
direct query. With `monitoring_api_url = "http://127.0.0.1:9109"` always set in
`config/shared/miner.toml`, the Prometheus query paths in web-proxy are never reached:

| Handler | Primary path | Fallback (dead) |
|---------|-------------|-----------------|
| `api_miners_handler` | `get_miner_stats_from_api` (monitoring API) | `get_miner_stats` (Prometheus) |
| `balance_handler` | `get_wallet_balance_from_faucet` (faucet) | `get_wallet_balance` (Prometheus) |
| `health_handler` | stratum-apps `/health` | Prometheus `hashpool_translator_info` |
| `api_pool_handler` | **none** | `get_pool_info` (Prometheus) — only path |

`api_pool_handler` is the exception: it correctly uses Prometheus as its only path.
`blockchain_network` is pool-derived data (the pool knows what chain bitcoin-node is on);
it flows pool → translator via SV2 → Prometheus → web-proxy. There is no config-local
substitute because the web-proxy has no direct connection to the pool server.

### 3. Dead dependency: stats-sv2 in translator

`roles/translator/Cargo.toml` lists `stats-sv2` but no translator source file imports it.
It is used only by `roles/pool/src/lib/monitoring.rs` (one call to `derive_hashrate`).

---

## Phase 1: Submit upstream PRs to SRI

Open PRs to `stratum-mining/stratum` for the two changes that belong upstream before
removing the vendor directory. This establishes the contribution record and unblocks the
crates.io path.

**PR A — Sv1ClientInfo session metrics:** see [`docs/pr/sri-pr-sv1-monitoring-fields.md`](pr/sri-pr-sv1-monitoring-fields.md)

**PR B — Vendor message extension hook:** see [`docs/pr/sri-pr-vendor-message-extension.md`](pr/sri-pr-vendor-message-extension.md)

**PR C — `network` field in `GlobalInfo`:** see [`docs/pr/sri-pr-globalinfo-network.md`](pr/sri-pr-globalinfo-network.md)

**Note on key_utils:** the `src/key_utils/mod.rs` re-export in the vendored crate is NOT
a valid upstream contribution. It is a local workspace type-unification workaround. The
correct fix is a `[patch.crates-io]` entry (see Phase 2 below). Do not open a PR for this.

---

## Phase 2: Replace vendor with git dep

While Phase 1 PRs are open (before merge), switch from the vendored path to a git dep
pointing to a `vnprc/stratum` fork branch containing the Sv1ClientInfo changes. The vendor
message extension (PR B) does not affect un-vendoring — it is a new addition to upstream,
not a modification of existing code. Follows the CDK pattern.

**Steps:**

1. Apply the `hashrate_5min` → `nominal_hashrate` rename (and `f64` → `f32`) locally
   (see PR A doc) so the fork branch tracks the PR exactly.

2. Create branch `hashpool/sv1-monitoring-extensions` on `vnprc/stratum` (fork of SRI
   monorepo) containing:
   - `src/monitoring/sv1.rs` with the four new fields
   - `src/key_utils/mod.rs` with the re-export (needed only until the `[patch]` fix below
     resolves the type mismatch — can be dropped from the fork after that)

3. In `roles/Cargo.toml`, add to `[patch.crates-io]`:
   ```toml
   stratum-apps = { git = "https://github.com/vnprc/stratum", rev = "<sha>" }
   key-utils    = { path = "../utils/key-utils" }
   ```
   The `key-utils` patch forces all crate instances — including stratum-apps — to resolve
   to the same local copy, unifying `Secp256k1PublicKey` types without any fork change.
   Once this patch is in place, the `key_utils/mod.rs` re-export can be removed from
   the fork branch entirely.

4. In `roles/translator/Cargo.toml`, change:
   ```toml
   # Before
   stratum-apps = { path = "../../common/stratum-apps", features = ["translator"] }
   # After
   stratum-apps = { version = "0.3.0", features = ["translator"] }
   ```
   The `[patch.crates-io]` in the workspace Cargo.toml redirects this to the git dep.

5. Verify `cargo build --workspace` in `roles/` passes.

6. Delete `common/stratum-apps/` directory.

**When SRI merges PR A and releases 0.3.1+:**
- Remove the `stratum-apps` `[patch.crates-io]` override
- Bump the version dep to the new release
- Keep the `key-utils` patch (it is correct and harmless indefinitely)
- Delete the fork branch

---

## Phase 3: Remove dead Prometheus query paths from web-proxy

### Architectural constraint: pool/proxy separation

The pool and translator are separated by the internet in production. All information that
the web-proxy displays about pool-side state (blockchain network, upstream connection status)
must flow through the SV2 protocol from pool → translator → monitoring server → web-proxy.
The web-proxy cannot read pool configs or directly contact pool services.

This means:

- **`api_pool_handler` must keep its Prometheus path.** `blockchain_network` and upstream
  connection status are pool-derived data that the translator learns via SV2 and surfaces
  via its Prometheus metrics. There is no config-local substitute.
- **`PrometheusClient` must stay in `AppState`.** The planned hashrate metrics page (future
  work) will use Prometheus time-series queries for historical hashrate graphs — data that
  the monitoring REST API does not provide (it gives only current state).

### What is actually dead code

Three handlers have monitoring-API primary paths with Prometheus fallback branches that are
never reached because `monitoring_api_url` is always configured:

| Function | Status |
|----------|--------|
| `get_miner_stats()` + `fetch_miners()` | Dead — replaced by `get_miner_stats_from_api()` |
| `get_wallet_balance()` | Dead — replaced by `get_wallet_balance_from_faucet()` |
| Prometheus `else` branch in `health_handler` | Dead — replaced by stratum-apps `/health` |
| `get_pool_info()` via Prometheus | **Keep** — only path for pool-derived data |
| `PrometheusClient` in `AppState` | **Keep** — needed for pool info + future hashrate page |

### Step 3.1: Delete dead Prometheus query functions

Remove from `roles/web-proxy/src/web.rs`:
- `get_miner_stats()` (~60 lines)
- `fetch_miners()` (~80 lines)
- `get_wallet_balance()` (~15 lines, the Prometheus variant — keep `get_wallet_balance_from_faucet`)

### Step 3.2: Remove unreachable `else` branches

In `api_miners_handler`: remove the `else { get_miner_stats(...) }` branch — keep only
the `if let Some(monitoring_url)` path.

In `balance_handler`: remove the `else { get_wallet_balance(...) }` branch — keep only
the `if let Some(faucet_url)` path.

In `health_handler`: remove the `else { prometheus.query_instant(...) }` branch — keep
only the `if let Some(monitoring_url)` path.

### Step 3.3: Remove the `MinerMetrics` Prometheus parsing struct

The `MinerMetrics` struct and any helpers that exist solely to parse Prometheus query
results into miner data can be deleted now that `get_miner_stats_from_api` owns that path.

### Step 3.4: Fix `api_pool_handler` — replace broken Prometheus query with monitoring REST API

**This is a bug fix.** `hashpool_translator_info` was removed during the sv2-apps migration
but the web-proxy was not updated. The metric is never emitted; `GET /api/pool` currently
always returns `{ "blockchain_network": "unknown", "connected": false }`.

Requires `GlobalInfo.network` from PR C (can be applied to the fork branch in Phase 2
before SRI merges):

1. Add `network: Option<String>` to `GlobalInfo` in the fork branch (local change pending PR C)
2. Populate it from `TranslatorConfig.network` in the translator at monitoring server startup
3. Rewrite `api_pool_handler` to:
   - Query `GET /api/v1/global` via `state.monitoring_api_url` for `network` and
     `server.total_channels`
   - Use `state.upstream_address` (already in `AppState` from config) for the pool address
   - Return `{ "blockchain_network": network, "upstream_pool": {"address": upstream},
     "connected": total_channels > 0 }`
4. Delete `get_pool_info()` (~30 lines)

---

## Phase 4: Remove dead stats-sv2 dep from translator

In `roles/translator/Cargo.toml`, remove:
```toml
# Delete these two lines:
# Hashpool-specific metrics
stats-sv2 = { path = "../roles-utils/stats-sv2" }
```

Verify `cargo build -C roles/translator -Z unstable-options` still passes.

---

## Summary: what gets deleted

| Item | Lines removed |
|------|--------------|
| `common/stratum-apps/` (entire directory) | ~5,500 |
| Dead Prometheus query functions in `web-proxy/src/web.rs` | ~155 |
| `get_pool_info()` in `web-proxy/src/web.rs` (broken, replaced by Step 3.4) | ~30 |
| `MinerMetrics` struct + helpers (Prometheus miner parsing) | ~100 |
| `stats-sv2` dep line in translator Cargo.toml | 2 |
| **Total** | **~5,787** |

Lines added: ~20 (Step 3.4 rewrite of `api_pool_handler` using monitoring REST API + config)

---

## Execution order

1. Phase 1 (SRI PRs) — open these in parallel, no code changes needed
2. Phase 4 (dead dep) — trivial, do first
3. Phase 3 (web-proxy Prometheus) — self-contained, do next
4. Phase 2 (un-vendor) — do last; depends on fork setup
