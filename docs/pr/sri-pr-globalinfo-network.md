# SRI PR: Add `network` field to `GlobalInfo`

**Target repo:** `stratum-mining/sv2-apps`
**Status:** Ready to file pending rebase + integration test run. All code changes complete
as of 2026-04-14. Branch `feat/globalinfo-network` in `stratum-mining/sv2-apps`
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

### Step 1.3 — Pool infers network from tp_address port ✅

**Files:** `pool-apps/pool/src/lib/config.rs`, `pool-apps/pool/src/lib/mod.rs`

Old approach (to be replaced): `PoolConfig` had `network: Option<String>` set manually
by the operator. This is error-prone and redundant — the operator already configures
`tp_address` correctly or nothing works.

New approach:
- Add a helper (e.g. `fn network_from_tp_port(port: u16) -> Option<&'static str>`) that
  maps standard sv2-tp ports to network name strings:
  ```rust
  match port {
      8442  => Some("mainnet"),
      18442 => Some("testnet3"),
      48442 => Some("testnet4"),
      38442 => Some("signet"),
      18447 => Some("regtest"),
      _     => None,
  }
  ```
- `PoolConfig` retains `network: Option<String>` with `#[serde(default)]` as an
  **explicit override** for non-standard port setups
- Add `fn effective_network(&self) -> Option<String>` (or equivalent) that:
  1. Returns `self.network.clone()` if it is `Some` (config override wins)
  2. Otherwise parses the port from `self.tp_address` and maps it via the helper
  3. Returns `None` if the port is non-standard and no override is set
- Pool calls `.with_network(config.effective_network())` on its `MonitoringServer`

No changes to `stratum-apps` library code are needed; only `config.rs` and the
call-site in `mod.rs` change.

### Step 1.4 — Translator fetches network from pool on connect ✅

**Files:** `miner-apps/translator/src/lib/config.rs`, `miner-apps/translator/src/lib/mod.rs`

Old approach (to be replaced): a background task polling `/api/v1/global` every 60
seconds indefinitely. This is overkill — the network is stable for the lifetime of the
process.

New approach: fetch once per upstream connection, stop as soon as a result is obtained.

- `TranslatorConfig` gains `upstream_monitoring_url: Option<String>` with `#[serde(default)]`
  (config shape unchanged)
- After establishing the upstream SV2 connection to the pool, perform a single
  `GET <upstream_monitoring_url>/api/v1/global` and write the result into the network Arc
- No background timer or periodic re-poll; the fetch is a one-shot per connection
- On disconnect/reconnect (existing fallback-restart path), fetch again — covers the
  case of reconnecting to a different pool instance, and naturally handles pool-not-ready
  at startup since the SV2 connection itself would not yet be established
- If `upstream_monitoring_url` is not set, network remains `None`

### Step 1.5 — Integration test ✅

**File:** `integration-tests/tests/monitoring_integration.rs`
**Also:** `integration-tests/lib/mod.rs`

The existing test `global_info_exposes_network` uses `network = "regtest"` as an
explicit config override (config-override path). This tests that the override works
but does not exercise port-based inference.

Updated approach — two test cases:

1. **`global_info_network_from_config_override`** — pool config has `network = "regtest"`
   with tp_address on a non-standard port; verifies the config override takes precedence.
   (This is essentially the old test, renamed.)

2. **`global_info_network_inferred_from_tp_port`** — pool config has no `network` field;
   sv2-tp runs on port 18447 (regtest default); verifies the pool reports `"regtest"` via
   port inference. Translator still polls pool monitoring to acquire the value.

Helper `start_pool_with_network()` renamed to `start_pool_with_network_override()` to
signal it uses the config path. A new `start_pool_on_regtest_port()` helper (or similar)
starts the pool with sv2-tp bound to port 18447 and no explicit `network` config.

Existing `start_pool`/`start_sv2_translator` remain unchanged.

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

`common/stratum-apps/src/monitoring/http_server.rs` updated to match upstream Steps
1.1/1.2: `Arc<RwLock<Option<String>>>`, `with_network()` writes into existing Arc,
`network_handle()` added, `handle_global` reads via lock.

This step must be re-synced after Step 1.3 revision lands upstream (though Step 1.3
changes are in pool application code, not the library — check whether any library API
surface changes).

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

### Step 2.4 — Translator fetches network from pool on connect (parallel reimplementation) ✅

`pool_monitoring_url: Option<String>` added to `TranslatorConfig` (config shape
unchanged). Current implementation uses a background polling loop — this must be revised
to a one-shot fetch per upstream connection, consistent with revised SRI Step 1.4.
Hashpool's translator is a separate application from the SRI translator; this change
must be made independently in hashpool's codebase.
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
- [x] **Update integration tests** — helper renamed to `start_pool_with_network_override()`
- [x] Rebase onto upstream main (2026-04-14; Cargo.toml conflict resolved)
- [x] Squash commits — branch is 2 clean commits on top of main
- [x] Vendored copy already in sync (Step 2.2 was done; rebase introduced no new API changes)
- [ ] Run integration tests in the sv2-apps repo — blocked by missing `capnp` binary in
  devenv shell; tests pass structurally (`cargo check` clean on all three workspaces)

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
