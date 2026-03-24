# SRI PR: Add `network` field to `GlobalInfo`

**Target repo:** `stratum-mining/sv2-apps`
**Status:** Ready to file — branch `feat/globalinfo-network` at `/home/vnprc/work/sv2-apps`

All implementation steps (Parts 1 and 2) are complete as of 2026-03-23.
The upstream branch is ready; see "Filing checklist" below before opening the PR.

---

## Summary

Add a `network: Option<String>` field to `GlobalInfo` that exposes the Bitcoin network
the application is operating on. This gives monitoring consumers a machine-readable
network identity without consulting Prometheus or reading config files.

---

## Motivation

`GlobalInfo` is the dashboard summary endpoint (`GET /api/v1/global`) for any SV2
application. It provides a snapshot of operational state. Network identity is the most
fundamental piece of operational metadata about a running SV2 node — yet it is currently
absent. Every SV2 application — pool, translator, JDC — has a concept of which Bitcoin
network it is operating on, and any monitoring dashboard built on this API would want to
display it.

---

## Correctness note

The `network` field reflects the network the application reports operating on, not a
verified chain state. Pool operators could misconfigure this value; validating block
templates against actual chain data is out of scope for this PR.

The config assertion is intentional. Alternative detection approaches were considered
and rejected:

- **nbits heuristic** — `n_bits` encodes proof-of-work difficulty, not chain identity.
  Only the regtest minimum (`0x207fffff`) is uniquely identifying; all other values are
  ambiguous across networks.
- **Coinbase output parsing** — `NewTemplate.coinbase_tx_outputs` contains raw
  `scriptPubKey` bytes. No script type encodes network identity in its script bytes;
  the network discriminator exists only in the human-readable address representation.
  This approach also degrades as coinbase outputs become more creative (commitments,
  miniscript, vault scripts).

The long-term fix is to propagate network identity authoritatively through the SV2
stack. Two upstream issues track this work:

- [sv2-tp#88](https://github.com/stratum-mining/sv2-tp/issues/88) — expose network via
  HTTP status endpoint (near-term, no protocol change required)
- [sv2-spec#190](https://github.com/stratum-mining/sv2-spec/issues/190) — add network
  to `SetupConnection.Success` in the Template Distribution Protocol (long-term)

---

## Part 1 — SRI PR changes ✅ COMPLETE

All steps implemented in branch `feat/globalinfo-network` at `/home/vnprc/work/sv2-apps`.
Two commits on top of main:

- `3d08825b` — Steps 1.1: GlobalInfo.network + with_network() + tests
- `2d92c95c` — Steps 1.2–1.5: network_handle(), pool config, translator polling,
  integration test

### Step 1.1 — stratum-apps library: `GlobalInfo.network` + `with_network()` ✅

**Files:** `stratum-apps/src/monitoring/mod.rs`, `stratum-apps/src/monitoring/http_server.rs`

- `GlobalInfo` gets `pub network: Option<String>`
- `MonitoringServer` gets `pub fn with_network(self, network: Option<String>) -> Self`
  (writes into existing `Arc<RwLock<...>>` — does not replace the Arc)
- `handle_global` serves `state.network.read().unwrap().clone()`
- Unit tests: `global_endpoint_with_no_sources` asserts `network` is null;
  `global_endpoint_network_field` asserts both null and populated cases

### Step 1.2 — Extend `MonitoringServer` for runtime network updates ✅

**File:** `stratum-apps/src/monitoring/http_server.rs`

The translator learns its network by polling the pool — it cannot know the network from
static config because it is a proxy. This requires updating network after the monitoring
server starts.

- `ServerState.network` changed from `Option<String>` to `Arc<RwLock<Option<String>>>`
- `with_network(self, Option<String>)` writes into the Arc (no public API change)
- `network_handle(&self) -> Arc<RwLock<Option<String>>>` — returns a clone of the Arc
  so a background task can write to it after `run()` has consumed `self`
- `handle_global` reads: `state.network.read().unwrap().clone()`

### Step 1.3 — Pool wires `with_network()` from config ✅

**Files:** `pool-apps/pool/src/lib/config.rs`, `pool-apps/pool/src/lib/mod.rs`

- `PoolConfig` gains `network: Option<String>` with `#[serde(default)]`
- Pool calls `.with_network(config.network())` on its `MonitoringServer`

### Step 1.4 — Translator polls pool for network ✅

**Files:** `miner-apps/translator/src/lib/config.rs`, `miner-apps/translator/src/lib/mod.rs`

- `TranslatorConfig` gains `upstream_monitoring_url: Option<String>` with `#[serde(default)]`
  and `with_upstream_monitoring_url()` builder
- After building `MonitoringServer`, calls `.network_handle()` to get the Arc
- Spawns `poll_network_from_pool()` background task:
  - Immediately fetches `GET <upstream_monitoring_url>/api/v1/global`
  - Writes `global.network` into the Arc
  - Sleeps 60 seconds between polls; trims trailing slash before URL construction
  - Shuts down cleanly on cancellation token
- Applied to both startup and fallback-restart code paths

### Step 1.5 — Integration test ✅

**File:** `integration-tests/tests/monitoring_integration.rs`
**Also:** `integration-tests/lib/mod.rs`

Test `global_info_exposes_network`:
1. Starts pool with `network = "regtest"` via `start_pool_with_network()`
2. Starts translator with `upstream_monitoring_url` pointing at pool monitoring
   via `start_sv2_translator_with_upstream_monitoring()`
3. Asserts `pool /api/v1/global` returns `network = "regtest"` immediately
4. Polls `translator /api/v1/global` for up to 10 seconds until `network = "regtest"`

New helpers `start_pool_with_network` and `start_sv2_translator_with_upstream_monitoring`
added; existing `start_pool`/`start_sv2_translator` unchanged (no behaviour change for
other tests).

---

## Part 2 — Hashpool-local changes ✅ COMPLETE

Committed 2026-03-23 in hashpool `923afb3f`.

### Step 2.1 — Revert wrong-direction changes ✅

Removed static `network` field from translator config and `.with_network()` calls.
Removed `network =` from tproxy config files.

### Step 2.2 — Sync vendored `MonitoringServer` with SRI Step 1.2 ✅

`common/stratum-apps/src/monitoring/http_server.rs` updated to match upstream:
`Arc<RwLock<Option<String>>>`, `with_network()` writes into existing Arc,
`network_handle()` added, `handle_global` reads via lock.

### Step 2.3 — Pool exposes `network` via monitoring server ✅

Hashpool's pool uses a custom axum router (not `stratum_apps::MonitoringServer`).
`GET /api/v1/global` added by hand using the shared `GlobalInfo` struct:

```rust
// Lightweight global endpoint used by the translator to poll for the Bitcoin network
// name. server/sv2_clients/sv1_clients are intentionally None — the pool exposes those
// stats only via Prometheus (/metrics), not through this endpoint.
async fn global_handler(State(state): State<Arc<MonitoringState>>) -> Json<GlobalInfo> {
    Json(GlobalInfo {
        server: None, sv2_clients: None, sv1_clients: None,
        uptime_secs: state.uptime_secs(),
        network: state.network.clone(),
    })
}
```

`pool.config.toml`: `network = "regtest"` / `prod/pool.config.toml`: `network = "testnet4"`

### Step 2.4 — Translator polls pool for network ✅

`pool_monitoring_url: Option<String>` added to `TranslatorConfig`.
`poll_pool_network()` background task spawned on both startup and fallback-restart paths.
`config/tproxy.config.toml`: `pool_monitoring_url = "http://127.0.0.1:9108"`

### Step 2.5 — Web-proxy ✅ (no changes needed)

`api_pool_handler` already queries `GET <monitoring_api_url>/api/v1/global` and reads
`network` and `server.total_channels`.

---

## Monitoring data flow (hashpool)

```
pool config (network = "regtest")
    ↓
pool monitoring server :9108
    GET /api/v1/global → { network: "regtest", uptime_secs: N, ... }
    ↓  translator polls every 60s
translator MonitoringServer :9109
    GET /api/v1/global → { network: "regtest", uptime_secs: N, ... }
    ↓  web-proxy reads on every /api/pool request
web-proxy
    GET /api/pool → { blockchain_network: "regtest", connected: true/false }
```

---

## Filing checklist

Before opening the PR against `stratum-mining/sv2-apps`:

- [ ] Rebase `feat/globalinfo-network` onto current upstream main (check for conflicts
  in `stratum-apps/src/monitoring/` and translator config)
- [ ] Run integration tests in the sv2-apps repo: `cargo test -p integration-tests`
- [ ] Squash or clean up commits as needed (currently 2 commits on top of main — fine
  to keep or squash into one)
- [ ] Sync any API differences back to hashpool vendored copy if the rebase introduces
  changes (particularly field names: upstream uses `upstream_monitoring_url`,
  hashpool uses `pool_monitoring_url`)

---

## Future work: migrate pool off custom axum router

Hashpool's pool monitoring (`roles/pool/src/lib/monitoring.rs`) is a bespoke axum
router. The sv2-apps reference pool uses `stratum_apps::MonitoringServer` with
`Sv2ClientsMonitoring` implemented on `ChannelManager` — giving it the full REST API,
Prometheus metrics, Swagger UI, and snapshot cache for free.

When hashpool migrates to the same pattern:
- The custom `monitoring.rs` axum code is deleted
- `Pool` or its downstream tracking struct implements `Sv2ClientsMonitoring`
- Hashpool-specific stats (ehash, quotes, per-downstream hashrate) move into
  Prometheus metrics via the standard `PrometheusMetrics` extension points
- Steps 2.2–2.3 (hand-rolled `/api/v1/global`) are replaced by `.with_network(config.network())`
- The translator polling in Step 2.4 simplifies because the pool's GlobalInfo
  is now a first-class stratum-apps endpoint

Blocked on mapping hashpool's ehash/quote stats onto the stratum-apps monitoring trait
surface. Separate task; does not block the network PR.
