# SRI PR: Add operational session metrics to `Sv1ClientInfo`

**Target repo:** `stratum-mining/stratum`
**File:** `protocols/stratum-apps/src/monitoring/sv1.rs`
**Status:** Pending — can be filed now; does not require un-vendoring `common/stratum-apps/`

---

## Summary

Add four operational session metrics to `Sv1ClientInfo` in the stratum-apps monitoring
framework. These fields expose per-miner data that any SV1-accepting proxy operator needs
to run a useful mining dashboard.

---

## Motivation

The existing `Sv1ClientInfo` struct captures protocol-level state (channel ID, extranonce,
version rolling, target). What it lacks is session-level operational data: how long has this
miner been connected, how many shares have they submitted, and what is their current hashrate?

Without these fields, operators building dashboards on top of the stratum-apps monitoring API
must maintain their own parallel data collection — duplicating work that the proxy already
tracks internally.

Every SV1 proxy (not just hashpool) needs this data. It is not application-specific.

---

## Proposed changes

```rust
/// Information about a single SV1 client connection
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Sv1ClientInfo {
    // ... existing fields unchanged ...

    // Session metrics
    /// Total shares submitted by this miner in the current session
    pub shares_submitted: u64,
    /// Unix timestamp (seconds) when this miner connected
    pub connected_at_secs: u64,
    /// IP address and port of the connected miner, if available.
    /// Implementations may omit this for privacy (set to `None` or redact before serving).
    pub peer_address: Option<String>,
    /// Estimated hashrate in H/s, computed over a recent observation window.
    /// `None` when vardiff is disabled or insufficient share data is available.
    pub nominal_hashrate: Option<f32>,
}
```

Note: our current local name is `hashrate_5min`. The PR should use `nominal_hashrate`
to match the existing convention in `ServerExtendedChannelInfo` and `ServerStandardChannelInfo`.
Update `sv1_monitoring.rs` locally when filing.

---

## Alignment with SRI goals

SRI's monitoring module is designed to be a complete operational toolkit for SV2 apps.
The fields being added are:

- **`shares_submitted`**: fundamental session metric; no dashboard is useful without it
- **`connected_at_secs`**: enables computing session duration client-side; universally needed
- **`peer_address`**: useful for debugging, rate limiting, and abuse detection; optional for privacy
- **`nominal_hashrate`**: the most directly useful metric for pool operators; derived from
  share timestamps which the proxy already maintains for vardiff. Matches the field name
  and type used on `ServerExtendedChannelInfo` and `ServerStandardChannelInfo`.

None of these are hashpool-specific. Any SV1 proxy — including SRI's own translator — would
surface these fields in a monitoring UI.

---

## Privacy consideration for `peer_address`

The field is `Option<String>`. Implementations that do not wish to expose IP addresses can:
- Set it to `None`
- Redact before passing to the monitoring server

The PR should document this clearly in the field's doc comment. Hashpool's `redact_ip`
config flag is one example pattern implementors can follow; it need not be part of the
upstream API.

---

## Architectural note: pool/proxy separation

In hashpool's deployment model, pool and translator run on separate servers separated by
the internet. Pool-derived data (blockchain network, upstream connection status) flows
pool → translator via SV2, then translator → monitoring REST API → web-proxy. This is why
per-miner session data (which is proxy-side) flows through the monitoring REST API rather
than Prometheus — the REST API is the correct path for current per-miner state.

The session metrics added by this PR (`shares_submitted`, `connected_at_secs`, etc.) are
translator-side data and are the natural fit for the monitoring REST API.

## Unblocking condition

This PR can be filed immediately against `stratum-mining/sv2-apps`. It does not depend on
any other work. Once merged, the vendored copy at `common/stratum-apps/src/monitoring/sv1.rs`
can be synced to include the upstream fields and the hashpool-specific comment annotations
can be removed. If the PR uses `nominal_hashrate` (see "Local changes needed" below),
update the vendored struct and all call sites at that time.

---

## Local changes needed when filing

1. Rename `hashrate_5min` → `nominal_hashrate` and change type `f64` → `f32` in:
   - `common/stratum-apps/src/monitoring/sv1.rs` (the struct field)
   - `roles/translator/src/lib/sv1_monitoring.rs` (the field assignment and cast)
   - `roles/web-proxy/src/web.rs` (JSON key `hashrate_5min` → `nominal_hashrate` in
     `get_miner_stats_from_api`)

2. Update the fork branch with these renames before the PR is opened so hashpool tracks
   exactly the upstream PR's field names.
