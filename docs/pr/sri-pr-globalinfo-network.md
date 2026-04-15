# SRI PR: Add `network` field to `GlobalInfo`

**Target repo:** `stratum-mining/sv2-apps`
**PR:** https://github.com/stratum-mining/sv2-apps/pull/367
**Status:** Open. Branch `feat/globalinfo-network` in `stratum-mining/sv2-apps`
(local checkout: `/home/vnprc/work/sv2-apps`).

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

The near-term approach is to infer network from the sv2-tp port the pool connects on.
sv2-tp already uses well-known per-network default ports (from `man sv2-tp`):

| Port  | Network  |
|-------|----------|
| 8442  | mainnet  |
| 18442 | testnet3 |
| 48442 | testnet4 |
| 38442 | signet   |
| 18447 | regtest  |

The pool reads its `tp_address` config (already required for operation), extracts the
port, and maps it to a network name. An optional `network` config field overrides this
for non-standard port setups. No changes to sv2-tp are needed.

[sv2-tp#88](https://github.com/stratum-mining/sv2-tp/issues/88) originally proposed an
HTTP status endpoint on sv2-tp to expose the network. Sjors (Bitcoin Core / sv2-tp
reviewer) requested the simpler port-convention approach instead — sv2-tp already speaks
clearly through its default ports, and adding an HTTP server to sv2-tp is undesirable.

The long-term protocol fix is tracked separately:

- [sv2-spec#190](https://github.com/stratum-mining/sv2-spec/issues/190) — add network
  to `SetupConnection.Success` in the Template Distribution Protocol (long-term,
  requires spec change; Sjors indicated willingness to support this)

---

## Part 1 — SRI PR changes ✅ COMPLETE

Branch `feat/globalinfo-network` in `stratum-mining/sv2-apps`
(local checkout: `/home/vnprc/work/sv2-apps`). Revised 2026-04-14.

### Step 1.1 — stratum-apps library: `GlobalInfo.network` + `with_network()` ✅

**Files:** `stratum-apps/src/monitoring/mod.rs`, `stratum-apps/src/monitoring/http_server.rs`

- `GlobalInfo` gets `pub network: Option<String>`
- `MonitoringServer` gets `pub fn with_network(self, network: Option<String>) -> Self`
  (writes into `Arc<RwLock<...>>`)
- `handle_global` serves `state.network.read().expect("...").clone()`
- Unit tests: `global_endpoint_with_no_sources` asserts `network` is null;
  `global_endpoint_network_field` asserts both null and populated cases

### Step 1.2 — `MonitoringServer` owns the upstream fetch ✅

**File:** `stratum-apps/src/monitoring/http_server.rs`

The translator learns its network from the pool. Rather than leaking internal state via
`network_handle()` and making the translator responsible for the HTTP fetch, the fetch
logic lives entirely inside `MonitoringServer`.

- `MonitoringServer` gains `upstream_monitoring_url: Option<String>` field
- `pub fn with_upstream_monitoring_url(self, url: Option<String>) -> Self` builder:
  validates the URL starts with `http://` (logs a warning and ignores if not); stores the URL
- `network_handle()` **removed** from the public API
- `run()` spawns a one-shot `tokio::spawn` that calls `fetch_network_from_upstream(url, network)`
  concurrently with the HTTP server startup
- Private `fetch_network_from_upstream` calls private `fetch_global_info` which uses
  `hyper` (already compiled via `axum`) + `http-body-util` — **no reqwest, no new Cargo.lock entries**
- `stratum-apps/Cargo.toml`: `hyper`, `hyper-util`, `http-body-util` added to the `monitoring`
  feature (they are already transitively present via axum; making them explicit adds
  zero new crates to Cargo.lock)
- All `.unwrap()` on `RwLock` guards changed to `.expect("network lock poisoned")`

### Step 1.3 — Pool infers network from tp_address port ✅

**Files:** `pool-apps/pool/src/lib/config.rs`, `pool-apps/pool/src/lib/mod.rs`

- `fn network_from_tp_port(port: u16) -> Option<&'static str>` maps standard sv2-tp ports
  using bitcoin-cli (`getblockchaininfo`) convention:
  ```rust
  8442  => Some("main"),      // mainnet
  18442 => Some("test"),      // testnet3
  48442 => Some("testnet4"),
  38442 => Some("signet"),
  18447 => Some("regtest"),
  ```
- `const VALID_NETWORKS: &[&str]` enumerates the known values; `effective_network()` validates
  the explicit override against it and logs a warning + returns `None` for unrecognised values
- `PoolConfig` retains `network: Option<String>` with `#[serde(default)]` as an override for
  non-standard port setups; `effective_network()` returns the override if valid, otherwise
  falls back to port inference
- Pool calls `.with_network(config.effective_network())` on its `MonitoringServer`
- Unit tests: `network_from_tp_port_known_ports` uses correct bitcoin-cli names;
  new `valid_networks_covers_known_port_outputs` asserts every port-inference result is
  in `VALID_NETWORKS`

### Step 1.4 — Translator delegates upstream fetch to MonitoringServer ✅

**Files:** `miner-apps/translator/Cargo.toml`, `miner-apps/translator/src/lib/mod.rs`

- `reqwest` **removed entirely** from `translator/Cargo.toml` — no longer needed
- `TranslatorConfig` retains `upstream_monitoring_url: Option<String>` with `#[serde(default)]`
  (TOML config shape unchanged; operators configure this field as before)
- Translator chains `.with_upstream_monitoring_url(self.config.upstream_monitoring_url())`
  onto the `MonitoringServer` builder — a one-liner replacing the old `network_handle` +
  spawned task pattern
- `fetch_network_from_pool` private function **removed** from `translator/src/lib/mod.rs`
- Both the initial-connect and reconnect paths updated identically

### Step 1.5 — Integration tests ✅

**File:** `integration-tests/tests/monitoring_integration.rs`

- `global_info_exposes_network` renamed to **`global_info_network_from_config_override`**
  (name now reflects what the test actually exercises)
- Polling deadline bumped **10 s → 30 s** (headroom for slow CI machines)
- New test **`global_info_network_unreachable_upstream`**: translator starts with
  `upstream_monitoring_url` pointing at a port where nothing is listening; asserts that
  the translator starts cleanly and serves `network: null` rather than panicking
- Port-based inference is validated by the unit tests in `pool/src/lib/config.rs`
  (`network_from_tp_port_known_ports`, `valid_networks_covers_known_port_outputs`)

Existing `start_pool` / `start_sv2_translator` helpers unchanged.

---

## Part 2 — Hashpool-local changes

Hashpool does not use the SRI reference pool or translator — it has its own application
code. Part 2 therefore has two distinct concerns:

1. **Library sync** — pull upstream `stratum-apps` library changes into the vendored
   copy at `common/stratum-apps/`. Mechanical; no hashpool-specific logic.
2. **Parallel reimplementation** — hashpool's custom pool and translator must implement
   the same feature independently, because they are separate applications that do not
   share code with the SRI reference roles.

The vendored copy exists so hashpool can track upstream library changes before they are
published as a crate. It should not be removed; removing it would require depending on
an unpublished crate version.

Committed 2026-03-23 in hashpool `923afb3f`. Steps 2.3 and 2.4 need revision to match
the updated SRI approach (port inference, one-shot fetch).

### Step 2.1 — Revert wrong-direction changes ✅

Removed static `network` field from translator config and `.with_network()` calls.
Removed `network =` from tproxy config files.

### Step 2.2 — Sync vendored `stratum-apps` library (library sync) ✅

Vendored `common/stratum-apps/` synced to upstream commit `5e3bb6fe` (2026-04-15).

**`monitoring/http_server.rs`**: `network_handle()` removed; `with_upstream_monitoring_url()`
builder added; `run()` spawns a one-shot hyper fetch at startup when the URL is set;
`fetch_network_from_upstream()` / `fetch_global_info()` private functions added.
`with_network()` `.unwrap()` → `.expect("network lock poisoned")`.
`monitoring` feature in `Cargo.toml` now lists `hyper`/`hyper-util`/`http-body-util` explicitly.

**`tp_type.rs`**: `VALID_NETWORKS` constant, `network_from_tp_port()` function,
`BitcoinNetwork::as_network_str()` method, and `TemplateProviderType::infer_network()`
method added as public API, matching upstream Step 1.3. New unit tests added.

**`monitoring/sv1.rs`**: hashpool-specific extensions preserved
(`shares_submitted`, `connected_at_secs`, `peer_address`, `hashrate_5min`).

**`key_utils/mod.rs`**: intentionally kept as `pub use key_utils_impl::*;` re-export
(type identity fix; upstream's standalone implementation would cause `E0308` mismatches).

### Step 2.3 — Pool exposes `network` via monitoring server (parallel reimplementation) ✅

Hashpool's pool uses a bespoke axum router rather than `stratum_apps::MonitoringServer`
(pre-existing divergence; see "Future work" below). This is not created by this PR.
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

These are config **overrides** (consistent with revised SRI Step 1.3). Whether to drop
them in favour of port inference depends on confirming which ports hashpool's sv2-tp
instances use. The port inference logic from Step 1.3 must also be added to hashpool's
pool config (parallel reimplementation — the SRI pool config code is not shared).

**Status (2026-04-15):** `network_from_tp_port()`, `VALID_NETWORKS`, and
`TemplateProviderType::infer_network()` are now public API in the vendored
`common/stratum-apps/src/tp_type.rs` (synced from upstream). The pool `config.rs` still
uses only the direct `network: Option<String>` override; `effective_network()` (port
inference + override validation) is not yet implemented in hashpool's pool config.
Regtest port 18447 is confirmed as hashpool's sv2-tp port. Adding `effective_network()`
to hashpool's `PoolConfig` would complete the parallel reimplementation.

### Step 2.4 — Translator fetches network from pool on connect (parallel reimplementation) ✅

`pool_monitoring_url: Option<String>` in `TranslatorConfig` (config shape unchanged).
Revised 2026-04-15 to use the one-shot `with_upstream_monitoring_url()` API from Step 1.4:
the background `reqwest`-based `fetch_pool_network` loop is removed; both the initial
connection and reconnect paths now chain
`.with_upstream_monitoring_url(self.config.pool_monitoring_url().map(|s| s.to_string()))`
onto the `MonitoringServer` builder. `reqwest` dep removed from `translator/Cargo.toml`.
`config/tproxy.config.toml`: `pool_monitoring_url = "http://127.0.0.1:9108"`

### Step 2.5 — Web-proxy ✅ (no changes needed)

`api_pool_handler` already queries `GET <monitoring_api_url>/api/v1/global` and reads
`network` and `server.total_channels`.

---

## Monitoring data flow (hashpool)

```
tp_address port 18447 → inferred "regtest"  [or network = "..." config override]
    ↓
pool monitoring server :9108
    GET /api/v1/global → { network: "regtest", uptime_secs: N, ... }
    ↓  translator fetches once on connect
translator MonitoringServer :9109
    GET /api/v1/global → { network: "regtest", uptime_secs: N, ... }
    ↓  web-proxy reads on every /api/pool request
web-proxy
    GET /api/pool → { blockchain_network: "regtest", connected: true/false }
```

Note: hashpool's pool currently uses the config-override path (`network = "regtest"` in
`pool.config.toml`). Once hashpool's tp_address is confirmed to use the standard regtest
port 18447, the explicit override can be dropped. Until then, keep the override.

---

## Filing checklist

Before opening the PR against `stratum-mining/sv2-apps`:

- [x] **Revise Step 1.3** — port-based inference + optional override implemented
- [x] **Fix network naming** — `network_from_tp_port` now returns bitcoin-cli names
  (`"main"`, `"test"`) matching `getblockchaininfo`; `VALID_NETWORKS` constant added
- [x] **Remove reqwest** — translator no longer carries an HTTP client dep; fetch lives
  in `stratum-apps/monitoring` using hyper (already compiled via axum)
- [x] **`network_handle()` removed** — `MonitoringServer` owns the upstream fetch via
  `with_upstream_monitoring_url()`; no internal Arc leaks into application code
- [x] **URL validation** — `with_upstream_monitoring_url` rejects non-`http://` URLs with
  a clear warning at startup
- [x] **Lock poisoning** — all `.unwrap()` on RwLock guards changed to `.expect("...")`
- [x] **Update integration tests** — test renamed to `global_info_network_from_config_override`;
  timeout bumped to 30 s; new `global_info_network_unreachable_upstream` test added
- [x] **JDC network inference** — moved `network_from_tp_port` / `VALID_NETWORKS` /
  `infer_network()` / `BitcoinNetwork::as_network_str()` into `stratum-apps/src/tp_type.rs`
  as public API; added `effective_network()` + `with_network()` builder + `network`
  override field to `JobDeclaratorClientConfig`; wired into JDC `MonitoringServer` at
  both startup and reconnect paths; new `global_info_network_jdc_from_config_override`
  integration test; unit tests in `tp_type.rs`, pool config, and JDC config
- [x] Rebase onto upstream main (2026-04-14; Cargo.toml conflict resolved)
- [x] Vendored copy synced (2026-04-15) to `5e3bb6fe` — see Step 2.2 above
- [x] **PR open**: https://github.com/stratum-mining/sv2-apps/pull/367
- [ ] Squash/amend commits — review feedback addressed in fixup commit; squash before merging
- [ ] Run integration tests in the sv2-apps repo — blocked by missing `capnp` binary in
  devenv shell; unit tests pass (`cargo test` clean on stratum-apps, pool-apps, miner-apps);
  integration-tests workspace has a pre-existing `icu_provider` dep constraint (requires
  rustc 1.86, project pins 1.85) unrelated to these changes

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
surface. Tracked in [vnprc/hashpool#86](https://github.com/vnprc/hashpool/issues/86).
Separate task; does not block the network PR or Part 2 revision — Part 2 can and should
be revised against the custom router in the meantime. When the migration eventually
happens, Step 2.3 collapses to a one-liner (`.with_network(config.effective_network())`).
