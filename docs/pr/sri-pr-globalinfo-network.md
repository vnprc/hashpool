# SRI PR: Add `network` field to `GlobalInfo`

**Target repo:** `stratum-mining/stratum`
**File:** `protocols/stratum-apps/src/monitoring/mod.rs`
**Status:** Pending — can be filed independently of other work

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

## Proposed changes

```rust
/// Global statistics from `/api/v1/global` endpoint
///
/// Fields are `Option` to distinguish "not monitored" (`None`) from "monitored but empty"
/// (`Some` with zeros).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ToSchema)]
pub struct GlobalInfo {
    /// Server (upstream) summary - `None` if server monitoring is not enabled
    pub server: Option<ServerSummary>,
    /// Sv2 clients (downstream) summary - `None` if Sv2 client monitoring is not enabled
    pub sv2_clients: Option<Sv2ClientsSummary>,
    /// Sv1 clients summary - `None` if Sv1 monitoring is not enabled
    pub sv1_clients: Option<Sv1ClientsSummary>,
    /// Uptime in seconds since the application started
    pub uptime_secs: u64,
    /// Bitcoin network this application is operating on.
    /// `None` if the application has not been configured with a network.
    /// Values follow bitcoin-cli convention: "main", "test", "testnet4", "regtest", "signet"
    pub network: Option<String>,
}
```

The field is `Option<String>` because:
1. Not all applications are configured with an explicit network (e.g. during initial setup)
2. The monitoring server is generic — the value must be supplied by the application

---

## How applications populate it

`MonitoringServer` (or the underlying `SnapshotCache`) should accept `network: Option<String>`
at construction time, set by the application from its config.

Example (translator):
```rust
MonitoringServer::new(
    monitoring_addr,
    Some(server_monitoring),
    None,                           // no sv2 clients (tProxy)
    Some(sv1_monitoring),
    Some("regtest".to_string()),    // from TranslatorConfig
)
```

---

## Alignment with SRI goals

SRI's monitoring module is designed to be a complete operational toolkit for SV2 apps.
Network identity is fundamental operational metadata that:
- Every dashboard consumer needs to display
- Is currently impossible to obtain reliably from the REST API alone
- Fits naturally in `GlobalInfo` alongside uptime, channel counts, and connection status

---

## Correctness note

The `network` field reflects the **configured** network, not a confirmed upstream connection
state. An application configured for `regtest` that cannot reach its pool still reports
`network = "regtest"`. This is expected and acceptable — consumers should use
`server.total_channels > 0` to determine live connection status separately.

---

## Local changes needed when filing

1. Add `network: Option<String>` to `GlobalInfo` in `protocols/stratum-apps/src/monitoring/mod.rs`
2. Update `MonitoringServer::new` (and `SnapshotCache` if needed) to accept and store `network`
3. Serve `network` from the `/api/v1/global` endpoint
4. Update hashpool translator to pass network from `TranslatorConfig` at monitoring server startup
5. Update `api_pool_handler` in `web-proxy/src/web.rs` to query `GET /api/v1/global` for
   `network` and `server.total_channels` instead of the `hashpool_translator_info` Prometheus
   metric (which is no longer emitted — see Phase 3, Step 3.4 of the cleanup plan)

---

## Unblocking condition

This PR can be filed independently of all other work. It does not depend on PR A or PR B.

The local fix (Step 3.4) can be applied to the fork branch used in Phase 2 before SRI
merges this PR, since the git dep override is already in place.

---

## Urgency note

This is not only a cleanup — `api_pool_handler` in web-proxy is **currently broken**.
The `hashpool_translator_info` Prometheus metric it queries was removed during the sv2-apps
migration but the web-proxy was not updated. As a result, `GET /api/pool` always returns
`{ "blockchain_network": "unknown", "connected": false }` regardless of translator state.
