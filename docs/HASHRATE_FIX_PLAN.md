# Hashrate Misreport Investigation & Fix Plan

## Summary
After refactoring to Prometheus-backed stats, the pool dashboard shows inflated hashrate (e.g., ~0.25 EH/s) while the proxy dashboard shows extremely low hashrate (e.g., ~300 H/s) for a single BitAxe miner. The evidence points to two distinct issues:

1) **SV2 → SV1 difficulty conversion uses the SV2 difficulty formula** when sending `mining.set_difficulty` to SV1 miners. This makes miners mine at ~4.29e9× higher difficulty than intended, causing proxy-side hashrate to read ~4.29e9× too low and triggering vardiff disconnect loops when difficulty is raised.

2) **Pool metrics count rejected shares** at full channel difficulty. The pool records share difficulty before validating shares and before enforcing `minimum_share_difficulty_bits`, which can dramatically inflate pool-side hashrate.

This document captures the analysis, the log evidence collected, and a stepwise fix plan with predicted outcomes and verification steps.

---

## Observed Symptoms

- **Pool dashboard** shows wildly high hashrate (e.g., 0.25 EH/s) for a single BitAxe miner.
- **Proxy dashboard** shows extremely low hashrate (e.g., ~300 H/s).
- **Raising vardiff** to realistic values causes miners to stop finding shares and enter disconnect loops.
- **Minimum share difficulty** for ehash (32 leading zero bits) suspected of skewing hashrate.

---

## Trace of the stats flow

### Proxy side (translator)
1) SV1 `mining.submit` received → `roles/translator/src/lib/sv1/downstream/message_handler.rs`
2) `validate_sv1_share()` checks share against downstream target.
3) Metrics use `target_to_difficulty(self.target)` and record via `WindowedMetricsCollector`.
4) `/metrics` exports `hashpool_translator_miner_hashrate_hs` using:
   ```
   hashrate = (sum_difficulty * 2^32) / window_seconds
   ```
5) Prometheus scrapes → web-proxy queries `hashpool_translator_miner_hashrate_hs`.

### Pool side
1) Translator submits SV2 share → `roles/pool/src/lib/mining_pool/message_handler.rs`.
2) **Metrics record share difficulty immediately**, before share validation and min-difficulty checks.
3) `/metrics` exports `hashpool_pool_downstream_hashrate_hs` using the same formula.
4) Prometheus scrapes → web-pool queries `hashpool_pool_downstream_hashrate_hs`.

---

## Root Cause Analysis

### A) SV2 → SV1 difficulty conversion mismatch (primary cause of proxy under-report)

`roles/roles-utils/stratum-translation/src/sv2_to_sv1.rs` currently builds `mining.set_difficulty` using:

```
sv2_target_to_difficulty(target) = 2^256 / target
```

This is the **SV2 difficulty** convention. SV1 miners expect **Bitcoin difficulty**:

```
sv1_difficulty = max_target / target
```

Ratio between these is ~4.29e9 (2^32). Result:

- SV1 miners are told to mine at ~4.29e9× higher difficulty than intended.
- Proxy-side hashrate becomes ~4.29e9× too low.
- Vardiff updates become impossible, leading to disconnect loops.

### B) Pool metrics count invalid/rejected shares (cause of pool over-report)

In `roles/pool/src/lib/mining_pool/message_handler.rs`, the pool calls:

```
stats.record_share_with_difficulty(difficulty)
```

**before**:
- share validity is confirmed, and
- `minimum_share_difficulty_bits` is enforced.

So **rejected shares are still counted** as if they met the full channel target difficulty, inflating pool-side hashrate.

---

## Log & Metrics Evidence (2026-03-13)

### Prometheus snapshots (host local)
- Proxy (translator):
  - `hashpool_translator_miner_hashrate_hs` = **257.5 H/s**
- Pool:
  - `hashpool_pool_downstream_hashrate_hs` = **186,554,556,496 H/s (~0.186 TH/s)**

**Ratio ≈ 7.2e8×**, consistent with the SV2/SV1 difficulty mismatch (~4.29e9×) plus windowing and share timing effects.

### Pool logs (last hour)
- Frequent `SubmitSharesExtended: valid share` lines
- No `share-difficulty-too-low` errors observed in the sample (not definitive)
- Vardiff `SetTarget` and `UpdateChannel` cycling observed

### Translator logs (last hour)
- Shows `SetTarget`/`UpdateChannel` activity
- No explicit logging of `mining.set_difficulty` values (needed for direct proof)

---

## Fix Plan (stepwise)

### Step 1 — Fix SV2→SV1 difficulty conversion (proxy-side fix)
**Change:** Use Bitcoin/SV1 difficulty (`max_target / target`) for `mining.set_difficulty` sent to SV1 miners.

**Predicted behavior (YOLO testnet mode):**
- Proxy hashrate jumps by ~4.29e9×.
  - From **~257.5 H/s → ~1.1 TH/s** (current sample).
- **Share rate may spike sharply** if initial difficulty is still very low (expected “share flood”). This is an acceptable confirmation signal in a demo/testnet setting.
- Vardiff disconnect loops stop.
- Share cadence will only stabilize after initial difficulty is raised (Step 2).

**Test after deploy:**
- Query Prometheus:
  - `hashpool_translator_miner_hashrate_hs`
- Check miner logs for realistic `mining.set_difficulty` values.

**Stabilization wait (Step 1 only):**
- **1–3 minutes** is enough to confirm the predicted proxy hashrate jump and observe share flood behavior. Do not wait 10 minutes before proceeding to Step 2 if share rate is excessive.

---

### Step 2 — Raise initial SV1 difficulty (proxy config)
**Change:** Increase the proxy/translator **initial difficulty settings** (e.g., shares_per_minute, initial target) so downstream miners do not flood the pool with extremely low-difficulty shares. This is a **proxy-side configuration fix** and should be applied after Step 1 so difficulty math is correct.\n+\n+**Predicted behavior:**\n+- Share submission rate drops to the expected range (e.g., 1–5 shares/minute per miner).\n+- Pool load stabilizes; no “share storm” or DoS-style behavior.\n+- Proxy hashrate remains stable (shares are fewer but each is higher difficulty).\n+\n+**Test after deploy:**\n+- Compare share submission rate in proxy and pool logs.\n+- Prometheus hashrate should remain stable; share counts per minute should drop.\n+\n+**Stabilization wait:**\n+- **5–10 minutes** after config change.\n+\n+---\n+\n+### Step 3 — Record pool metrics only after acceptance
**Change:** Move `record_share_with_difficulty()` to **after**:
- share validity passes, and
- `minimum_share_difficulty_bits` passes.

**Predicted behavior:**
- Pool hashrate drops sharply if rejected shares were being counted.
- Pool hashrate converges toward proxy hashrate.

**Test after deploy:**
- Query Prometheus:
  - `hashpool_pool_downstream_hashrate_hs`
- Compare to proxy hashrate.

**Stabilization wait:**
- **5–10 minutes** (pool window is 60s, web chart smoothing is 300s).

---

### Step 4 — Policy choice for pool hashrate accounting

**Option A (recommended / SRI-aligned):**
- Count only **accepted shares** at the **channel target difficulty**.
- This matches SRI intent and industry-standard share accounting.

**Option B (estimator):**
- Compute actual per-share difficulty from header hash.
- Good for estimation but non-standard for accounting.

**Recommendation:**
- Use **Option A** for pool hashrate accounting.
- Option B can be added as a secondary metric if desired.

**Stabilization wait:**
- **5–10 minutes** after any change to metric definition.

---

### Step 5 (optional) — Translator-side minimum bits filter
**Change:** Enforce `minimum_share_difficulty_bits` at translator before forwarding to pool.

**Predicted behavior:**
- Less junk traffic to pool.
- Fewer reject logs.
- Pool/proxy metrics align more tightly.

**Stabilization wait:**
- **5–10 minutes**.

---

## Verification Plan (after each step)

### Metrics snapshots (Prometheus)
- Proxy:
  - `hashpool_translator_miner_hashrate_hs`
- Pool:
  - `hashpool_pool_downstream_hashrate_hs`

### Expected progression
1. **Step 1**: Proxy hashrate jumps to realistic TH/s.\n+2. **Step 2**: Share rate normalizes (no share flood), pool load drops.\n+3. **Step 3**: Pool hashrate drops and converges with proxy.\n+4. **Step 4**: Pool hashrate behavior matches SRI expectations.\n+5. **Step 5**: Cleaner logs, tighter convergence.

---

## Recommended wait time for dashboard stabilization

The UI uses smoothed Prometheus ranges (web-pool uses 300s window). To avoid false conclusions, wait:

- **At least 5 minutes** after each deploy
- **10 minutes** if hashrate is low or share interval is long

---

## Notes
- The `minimum_share_difficulty_bits` (e.g., 32) is correct for ehash policy but must not affect channel difficulty. It should only filter shares for acceptance.
- If you want direct proof, enable DEBUG logging of SV1 `mining.set_difficulty` values or capture miner logs.
