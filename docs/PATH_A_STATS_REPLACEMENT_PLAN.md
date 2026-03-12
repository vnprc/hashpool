# Path A Plan: Replace Custom Stats With Monitoring + Metrics Store (Per-Deployment)

## Goal
Keep the current UI (web-pool + web-proxy + wallet flows) while deleting the custom stats stack. Use upstream monitoring metrics for pool/translator and a per-deployment metrics store (Prometheus/VictoriaMetrics) as the time-series backend.

**Key constraint:** pool and proxy are separate install bases. They cannot query the same stats service. Therefore each deployment runs its own metrics store.

## Outcome
- **Remove:** `stats-pool`, `stats-proxy`, snapshot pollers, TCP JSON ingestion, in-memory snapshot stores
- **Add:** one metrics store per deployment (Prometheus or VictoriaMetrics)
- **Keep:** existing UI and wallet flows (unchanged API surface if possible)

## Runtime Topology (Per Deployment)
**Pool side:**
- pool (with monitoring enabled)
- metrics store (scrapes pool `/metrics`)
- web-pool (adapter + UI)

**Miner side:**
- translator (with monitoring enabled)
- metrics store (scrapes translator `/metrics`)
- web-proxy (adapter + UI + wallet)

## Phase 0 — Baseline Inventory (0.5–1 day)
- Lock in current UI API shapes:
  - `roles/web-pool/src/web.rs`
  - `roles/web-proxy/src/web.rs`
- Identify current fields used in templates and JS.

**Output:** mapping table of UI endpoints → data fields.

## Phase 1 — Monitoring Source of Truth (1–3 days)
Choose one:
1) **Backport sv2-apps monitoring into hashpool pool/translator**
   - Bring in `stratum-apps::monitoring` module.
   - Enable `monitoring_address` and cache refresh in configs.
2) **Minimal Prometheus metrics in hashpool roles** if backport is too heavy.

**Likely files:**
- `roles/pool/src/lib/mod.rs`
- `roles/translator/src/lib/mod.rs`
- `roles/pool/src/lib/config.rs`
- `roles/translator/src/lib/config.rs`

**Output:** `/metrics` exposed on pool + translator.

## Phase 2 — Metrics Store (Per Deployment) (0.5–1 day)
- Pool deployment runs Prometheus/VictoriaMetrics scraping pool `/metrics`.
- Miner deployment runs Prometheus/VictoriaMetrics scraping translator `/metrics`.

**Output:** time-series data available locally per deployment.

## Phase 3 — Adapter Layer in Web Services (2–4 days)
- Add Prometheus client/query layer inside:
  - `roles/web-pool`
  - `roles/web-proxy`
- Replace old stats service fetches with Prometheus queries.
- Preserve existing JSON shapes where possible to avoid UI changes.

**Endpoints to support**
- web-pool: `/api/stats`, `/api/services`, `/api/connections`, `/api/hashrate`, `/api/downstream/{id}/hashrate`
- web-proxy: `/api/miners`, `/api/pool`, `/balance`, `/mint/tokens`

**Output:** UI works without stats services.

## Phase 4 — Remove Old Stats Services (1 day)
- Delete crates:
  - `roles/stats-pool`
  - `roles/stats-proxy`
- Remove snapshot pollers and stats adapters if no longer used.
- Update configs, devenv, docs.

**Output:** no custom stats stack remaining.

## Phase 5 — Smoothing + Correctness (1–2 days)
- Use longer rate windows in PromQL (e.g., 5–15m) to smooth hashrate graphs.
- Validate hashrate and shares against expected values.

## Notes
- Service count stays the same per deployment, but the custom code surface shrinks substantially.
- No cross-deployment coupling: each install has its own metrics store.
