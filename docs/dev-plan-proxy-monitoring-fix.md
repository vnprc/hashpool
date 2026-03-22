# Dev Plan: Restore Proxy Monitoring Feature Parity After sv2-apps Migration

## Background

The sv2-apps translator migration (commit 2b6f60d7) replaced the old hashpool translator
core with the upstream sv2-apps implementation. The old translator had a bespoke monitoring
layer (`miner_stats.rs`, `monitoring.rs`) that:

1. Tracked per-miner state: shares submitted, windowed hashrate, connected timestamp, IP
2. Exposed custom Prometheus metrics: `hashpool_translator_miner_info`,
   `hashpool_translator_miner_hashrate_hs`, `hashpool_translator_miner_shares_total`,
   `hashpool_translator_miner_connected_at_seconds`, `hashpool_translator_wallet_balance_ehash`
3. The web-proxy queried these Prometheus metrics to populate the /miners and /balance pages

The new sv2-apps monitoring (stratum-apps crate) provides a REST API and aggregate Prometheus
metrics (`sv1_clients_total`, `sv1_hashrate_total`) but no per-miner labeled Prometheus metrics.
A quick-fix commit (092a7666) was applied but it introduced several problems:

- **Hardcoded default IP** `"http://127.0.0.1:9109"` in `config.rs` — violates config-driven
  architecture; production deployments have different addresses
- **Hardcoded `1.0` shares/minute** in `sv1_server.rs` — wrong approach; hashrate should be
  derived from actual share submissions using a windowed metrics collector, same as the old code
- **Shares not reported** — `Sv1ClientInfo` has no `shares_submitted` field; `DownstreamData`
  doesn't track per-miner share counts
- **ehash balance not reported** — `hashpool_translator_wallet_balance_ehash` Prometheus metric
  no longer exists; no substitute in stratum-apps monitoring
- **Missing miner metadata** — IP address and connected-at timestamp not stored in new
  `DownstreamData`

## Scope and Goals

Restore full feature parity with the pre-migration web-proxy UI:
- Miners page: miner name, hashrate (windowed), shares submitted, connected time
- Balance page: current ehash wallet balance
- All values derived from config files; no hardcoded addresses or magic numbers

## Root Cause Analysis

### 1. Hardcoded `monitoring_api_url` default

The web-proxy needs to know the translator's monitoring API address. The translator already has
`monitoring_address = "127.0.0.1:9109"` in `config/tproxy.config.toml`. The web-proxy reads
`config/shared/miner.toml` (via `--config` and `--shared-config` CLI flags). These config files
are the right place to share this address.

**Fix**: Add `[translator] monitoring_api_url` to `config/shared/miner.toml`. The web-proxy reads
it from there. No hardcoded fallback — fail with a clear error if missing (same pattern as other
required config values). The `[monitoring_api]` section added to `web-proxy.config.toml` in the
quick fix is redundant and should be removed.

### 2. Hardcoded `1.0` shares/minute in hashrate derivation

**Wrong approach entirely.** Deriving hashrate from the pool's SetTarget message using a
fixed shares/minute reference was the wrong design. The correct approach (used by the old
translator) is:

- Per-miner **windowed metrics collector** (`WindowedMetricsCollector` from `roles-utils/stats-sv2`)
- Each share submission records the share's **Bitcoin difficulty** and a timestamp
- Hashrate = `derive_hashrate(sum_difficulty_in_window, window_seconds)` (from stats-sv2)
- Window size comes from `config.metrics_window_secs` (already in tproxy.config.toml)

The `shares_per_minute` value from `[downstream_difficulty_config]` is intentionally set to
`12_000_000` to force a very easy initial target before the upstream assigns one. It has no
relevance to hashrate monitoring.

The share difficulty (Bitcoin convention) when a share is submitted is:
`difficulty = genesis_target / current_target` where `genesis_target ≈ 0x00000000FFFF0000...`
This is what `roles_logic_sv2::utils::target_to_difficulty(bitcoin::Target)` computes.

The revert of the `1.0` hardcode requires reverting the SetTarget path change in
`sv1_server.rs` — with windowed share tracking, the SetTarget path no longer needs to
estimate hashrate at all.

### 3. Shares not tracked per-miner

`DownstreamData` has no `shares_submitted` counter. `Sv1ClientInfo` (in stratum-apps) has no
shares field. This data never reaches the web-proxy.

**Fix**: Add `shares_submitted: u64` to `DownstreamData`. Increment it atomically in
`sv1_server.handle_submit_shares`. Expose it in `Sv1ClientInfo` and the monitoring REST API.

### 4. ehash wallet balance not reported

The old translator's `/metrics` endpoint computed `wallet.total_balance().await` and wrote the
`hashpool_translator_wallet_balance_ehash` gauge. The new stratum-apps monitoring server has no
wallet balance hook.

Two possible approaches considered:
- **Option A**: Add a custom Prometheus gauge to the stratum-apps registry, updated by
  quote_sweeper after each mint. Requires hooking into stratum-apps' internal registry.
- **Option B**: Add a `/balance` endpoint to the faucet API (port 8083) that returns
  the wallet balance directly. The web-proxy fetches from this endpoint instead of Prometheus.

**Option B is preferred**: the faucet API already holds a reference to the wallet, the endpoint
is trivial to add, and it avoids coupling the Prometheus exposition path to CDK wallet state.
The web-proxy already has a config-driven `faucet_url` and a reqwest client (added in 092a7666).

The web-proxy `balance_handler` changes from querying Prometheus to calling
`GET {faucet_url}/balance`.

The `hashpool_translator_info` metric (queried by the health check) is also missing from the new
translator. It should be added in the same pass, OR the health check should be updated to not
depend on it.

### 5. Missing miner IP address and connected-at timestamp

The TCP peer address is available in `sv1_server` at `listener.accept()` but discarded.
The connection timestamp is not captured. Both are needed for the miners table.

**Fix**: Add `peer_address: SocketAddr` and `connected_at: std::time::SystemTime` to
`DownstreamData`. Populate them at downstream creation time. Pass through to `Sv1ClientInfo`.
Apply `redact_ip` config flag when building `Sv1ClientInfo` (already in translator config).

### 6. `Sv1ClientInfo` missing fields

The stratum-apps `Sv1ClientInfo` struct is the data contract between translator monitoring and
web-proxy. It needs to carry the full per-miner state. Since stratum-apps is vendored in
`common/stratum-apps/`, we can extend it.

## Implementation Plan

### Step 1: Revert the quick-fix commit

```
git revert 092a7666 --no-edit
```

This removes the hardcoded IP, the `1.0` shares/minute hack, and the incomplete web-proxy
Prometheus→REST migration. We start clean from the original migration commit (2b6f60d7).

### Step 2: Extend `DownstreamData` with per-miner tracking fields

**File**: `roles/translator/src/lib/sv1/downstream/data.rs`

Add the following fields to `DownstreamData`:

```rust
use stats_sv2::WindowedMetricsCollector;
use std::net::SocketAddr;
use std::time::SystemTime;

pub struct DownstreamData {
    // ... existing fields ...
    pub shares_submitted: u64,
    pub connected_at: SystemTime,
    pub peer_address: Option<SocketAddr>,
    pub metrics_collector: WindowedMetricsCollector,  // window from config
}
```

Update `DownstreamData::new()` to accept `connected_at: SystemTime`, `peer_address: Option<SocketAddr>`,
and `metrics_window_secs: u64` (for the windowed collector).

**Dependency**: Add `stats-sv2` to `roles/translator/Cargo.toml` as a path dependency
(`path = "../roles-utils/stats-sv2"`).

### Step 3: Capture peer address and timestamp at connection accept

**File**: `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs`

At the `listener.accept()` call where a new downstream is created, pass `addr` (the
`SocketAddr` from the TCP accept) and `SystemTime::now()` into `Downstream::new()` / the
initial `DownstreamData` construction. Also pass `metrics_window_secs` from
`self.config.metrics_window_secs` (already in tproxy.config.toml).

### Step 4: Track shares and windowed hashrate in `handle_submit_shares`

**File**: `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs`

In `handle_submit_shares`, after building `submit_share_extended`:

```rust
// Increment share counter and record difficulty for windowed hashrate
self.downstreams
    .get(&message.downstream_id)
    .map(|downstream| {
        downstream.downstream_data.safe_lock(|data| {
            data.shares_submitted += 1;
            // Compute Bitcoin difficulty from current target
            let target = bitcoin::Target::from_be_bytes(
                data.target.to_be_bytes()
            );
            let difficulty = roles_logic_sv2::utils::target_to_difficulty(target) as f64;
            data.metrics_collector.record_share(difficulty);
        });
    });
```

This records shares regardless of whether vardiff is enabled, matching the old architecture.

**Note**: The existing `SetTarget` path no longer needs to derive/estimate hashrate; remove
the `derived_hashrate` calculation from `handle_set_target_without_vardiff` that was
introduced in 092a7666. The DivisionByZero situation is avoided by not calling
`hash_rate_from_target` there at all.

### Step 5: Extend `Sv1ClientInfo` in stratum-apps

**File**: `common/stratum-apps/src/monitoring/sv1.rs`

```rust
pub struct Sv1ClientInfo {
    pub client_id: usize,
    pub channel_id: Option<u32>,
    pub authorized_worker_name: String,
    pub user_identity: String,
    pub target_hex: String,
    pub hashrate: Option<f32>,
    // NEW:
    pub shares_submitted: u64,
    pub connected_at_secs: Option<u64>,   // Unix timestamp
    pub peer_address: Option<String>,      // "ip:port" or "REDACTED"
    // existing fields...
    pub extranonce1_hex: String,
    pub extranonce2_len: usize,
    pub version_rolling_mask: Option<String>,
    pub version_rolling_min_bit: Option<String>,
}
```

### Step 6: Populate new fields in `sv1_monitoring.rs`

**File**: `roles/translator/src/lib/sv1_monitoring.rs`

The `downstream_to_sv1_client_info` function reads from `DownstreamData` to build
`Sv1ClientInfo`. Update it to:

1. Compute `hashrate` from windowed metrics:
   ```rust
   let sum_diff = data.metrics_collector.sum_difficulty_in_window();
   let window_secs = data.metrics_collector.window_seconds();
   let hashrate = stats_sv2::derive_hashrate(sum_diff, window_secs);
   // hashrate is now in H/s as f64; store as Option<f32>
   let hashrate = if hashrate > 0.0 { Some(hashrate as f32) } else { None };
   ```

2. Populate `shares_submitted` from `data.shares_submitted`

3. Compute `connected_at_secs` from `data.connected_at`:
   ```rust
   let connected_at_secs = data.connected_at
       .duration_since(SystemTime::UNIX_EPOCH)
       .ok()
       .map(|d| d.as_secs());
   ```

4. Apply `redact_ip` config when setting `peer_address`:
   ```rust
   // Access redact_ip from global config (same pattern as tproxy_mode())
   let peer_address = if config_redact_ip() {
       Some("REDACTED".to_string())
   } else {
       data.peer_address.map(|addr| addr.to_string())
   };
   ```

**Note on `redact_ip`**: The translator config's `redact_ip` flag needs to be accessible from
`sv1_monitoring.rs`. The existing pattern for global config access (e.g., `tproxy_mode()`,
`vardiff_enabled()`, `is_aggregated()`) is the right approach. Add a `redact_ip()` global
accessor set during startup.

### Step 7: Add `/balance` endpoint to faucet API

**File**: `roles/translator/src/lib/faucet_api.rs`

Add a `GET /balance` route that returns the current wallet balance:

```rust
async fn balance_handler(State(state): State<Arc<FaucetState>>) -> impl IntoResponse {
    let balance = state.wallet.total_balance().await
        .map(u64::from)
        .unwrap_or(0);
    Json(json!({ "balance": balance, "unit": "HASH" }))
}
```

The `FaucetState` already holds `Arc<Wallet>`. No new state needed.

### Step 8: Add `hashpool_translator_info` Prometheus metric

**File**: `roles/translator/src/lib/mod.rs`

The web-proxy health check queries `hashpool_translator_info{blockchain_network, upstream_address}`.
This metric no longer exists after the migration.

**Option A** (preferred): Update the web-proxy health check to not depend on this metric. Instead
check if the monitoring API is reachable (`GET {monitoring_api_url}/health`). The stratum-apps
monitoring server already has a `/health` endpoint.

**Option B**: Register a custom Prometheus gauge with the stratum-apps registry (complex).

Prefer Option A to avoid coupling the translator's info exposition to the old Prometheus format.

### Step 9: Update web-proxy to use monitoring REST API and faucet balance

**File**: `roles/web-proxy/src/web.rs`

This replaces the Prometheus-based miner fetch with the monitoring REST API.

**`fetch_miners()`**: Call `GET {monitoring_api_url}/api/v1/sv1/clients?limit=1000`.
Deserialize `Sv1ClientsResponse` with the new fields. Map to `MinerMetrics`:
- `id`: `client.client_id as u32`
- `name`: `client.authorized_worker_name`
- `address`: `client.peer_address.unwrap_or_default()`
- `hashrate_hs`: `client.hashrate.unwrap_or(0.0) as f64`
- `shares`: `client.shares_submitted`
- `connected_at`: `client.connected_at_secs.unwrap_or(0)`

**`get_wallet_balance()`**: Change from Prometheus query to faucet API:
```rust
async fn get_wallet_balance(faucet_url: &str, client: &reqwest::Client) -> Result<u64, String> {
    let url = format!("{}/balance", faucet_url);
    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;
    Ok(resp["balance"].as_u64().unwrap_or(0))
}
```

**`health_check`**: Update to call `GET {monitoring_api_url}/health` instead of querying
`hashpool_translator_info` from Prometheus.

**`AppState`**: Add `monitoring_api_url: String` and `http_client: reqwest::Client` (a shared
client for monitoring API + faucet calls, not a separate one per call).

### Step 10: Config chain — no hardcoded addresses

**File**: `config/shared/miner.toml`

Add a new section for translator monitoring:

```toml
[translator]
# Monitoring API base URL for the sv2-apps monitoring server
monitoring_api_url = "http://127.0.0.1:9109"
```

**File**: `roles/web-proxy/src/config.rs`

- Remove `MonitoringApiConfig` and `[monitoring_api]` from `WebProxyConfig` (added in 092a7666)
- Read `monitoring_api_url` from the shared config that the web-proxy already loads:
  ```rust
  let monitoring_api_url = shared_config
      .get("translator")
      .and_then(|t| t.get("monitoring_api_url"))
      .and_then(|u| u.as_str())
      .ok_or("Missing required config: translator.monitoring_api_url")?
      .to_string();
  ```

**File**: `config/web-proxy.config.toml`

Remove the `[monitoring_api]` section added in the quick fix.

### Step 11: Remove quick-fix hack from `sv1_server.rs`

**File**: `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs`

After Step 4 (windowed share tracking), the `handle_set_target_without_vardiff` function no
longer needs to derive hashrate from the SetTarget message. Remove the `derived_hashrate`
calculation and the call to `hash_rate_from_target`. Simply forward the SetTarget to downstreams
without the hashrate derivation step. The `DivisionByZero` issue disappears because we no longer
call `hash_rate_from_target` at all in this path.

## File Change Summary

| File | Change |
|------|--------|
| `roles/translator/src/lib/sv1/downstream/data.rs` | Add `shares_submitted`, `connected_at`, `peer_address`, `metrics_collector` |
| `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs` | Pass `addr`+timestamp at connect; track shares+difficulty in `handle_submit_shares`; remove hashrate derivation from SetTarget path |
| `roles/translator/src/lib/sv1_monitoring.rs` | Compute hashrate from windowed metrics; populate new `Sv1ClientInfo` fields |
| `roles/translator/src/lib/faucet_api.rs` | Add `GET /balance` endpoint |
| `roles/translator/src/lib/mod.rs` | Pass `peer_address`+`metrics_window_secs` into downstream construction; set `redact_ip()` global |
| `roles/translator/Cargo.toml` | Add `stats-sv2` path dependency |
| `common/stratum-apps/src/monitoring/sv1.rs` | Add `shares_submitted`, `connected_at_secs`, `peer_address` to `Sv1ClientInfo` |
| `roles/web-proxy/src/web.rs` | Fetch miners from monitoring REST API; fetch balance from faucet `/balance`; update health check |
| `roles/web-proxy/src/config.rs` | Read `monitoring_api_url` from shared config; remove `MonitoringApiConfig` |
| `roles/web-proxy/src/main.rs` | Pass `monitoring_api_url` to `run_http_server` |
| `config/shared/miner.toml` | Add `[translator] monitoring_api_url` |
| `config/web-proxy.config.toml` | Remove `[monitoring_api]` section |

## Out of Scope

- **Miner ID stability across reconnects**: The old `MinerTracker` assigned stable incrementing
  IDs. The new `client_id` is `downstream_id` which resets on translator restart. Acceptable
  for now; a persistent ID strategy is a separate concern.
- **Task 7 (remove 0xC0/0xC1 from parsers_sv2)**: Still blocked on pool refactoring. Deferred.
- **`hashpool_translator_info` Prometheus metric**: Web-proxy health check updated to use
  stratum-apps `/health` endpoint instead. Old metric not re-added.
