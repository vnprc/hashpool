# Warm Start Time-Series Initialization

## Goal
Eliminate the "restart crater" on pool and translator hashrate graphs without inventing synthetic data or distorting upside estimates. After any process restart, charts should continue from the last known steady-state as soon as the stats service receives the next snapshot.

## Key Idea
Reuse the real `hashrate_samples` that are already persisted to SQLite every five seconds. When a process boots, hydrate each in-memory `WindowedMetricsCollector` with the most recent samples that fall inside its configured window. Because the data is historical and already reflects exact per-share difficulty sums, the collector immediately reports the same `sum_difficulty_in_window` it had before the restart. No new schema, no fake shares, no UI changes.

```
      ┌────────────┐    snapshots    ┌──────────────────┐
      │ pool/tproxy│ ─────────────▶ │ stats storage    │
      └────────────┘                 │ (hashrate_samples│
             ▲                       └──────────────────┘
             │   warm-start query            │
             └─────────────── replay recent ─┘
```

## Requirements
1. **Accuracy** – Warm-start must use only real snapshots that were already persisted; never invent synthetic difficulty or cap upside luck.
2. **Isolation** – Translator and pool can warm-start independently; if one was down longer than its window, it simply begins empty again.
3. **Safety** – Ignore stale samples (older than window) and handle missing data gracefully.
4. **Minimal change** – Touch only stats plumbing (collector init + storage helper). The rest of the stack keeps using target-based difficulty.

## Implementation Plan

### 1. Storage Helper
- Add `fn recent_samples(&self, downstream_id: u32, window_seconds: u64) -> Vec<HashrateSample>` to the SQLite implementation of `StatsStorage` (roles/roles-utils/stats-sv2/src/storage.rs).
- Query `hashrate_samples` ordered by timestamp DESC, filtered to `timestamp >= now - window_seconds`, limited to e.g. 512 rows per downstream.
- Return tuples `(timestamp, sum_difficulty, window_seconds)`; no schema change required.

### 2. Collector Hydration Utility
- Introduce a helper (e.g., `warm_start_collector`) that accepts a mutable `WindowedMetricsCollector` plus the rows returned from step 1 and replays them oldest→newest.
- When replaying, push the stored `sum_difficulty` values directly by calling a new method such as `collector.record_share_at(timestamp, difficulty)` to preserve original timestamps. If modifying the collector API is undesirable, expose a small internal method guarded under cfg(test) to avoid copying logic.
- Skip any sample whose timestamp is older than `now - window_seconds` to prevent reintroducing stale data.

### 3. Translator Integration
- During translator startup (e.g., when building `MinerInfo` inside `add_miner` or when loading existing miners), call the storage helper and hydrate each miner’s `metrics_collector` before the first snapshot is emitted.
- Log `info` when warm-start succeeds (`mined 43s of history restored for miner #123`). Fall back silently if the query returns nothing (fresh miner or DB cleared).

### 4. Pool Integration
- Extend `pool-stats` registry initialization to warm-start each downstream collector the same way. The pool stats service uses the identical `WindowedMetricsCollector`, so the helper can be shared.

### 5. Testing
- **Unit tests**: cover the new collector hydration function by feeding deterministic samples and asserting that `sum_difficulty_in_window()` matches expectations. Also test that out-of-window samples are ignored.
- **Manual smoke**: run `devenv up`, let shares accumulate, restart pool/translator, and confirm `/api/hashrate` does not crater.

## Operational Notes
- If services are down longer than their window (e.g., 10 minutes for a 1-minute window), warm-start naturally returns no samples, so charts ramp from zero just as today.
- Because we replay only the last `window_seconds`, we never double-count older work and we preserve upside spikes exactly as they occurred.
- This approach keeps the persistent format unchanged, so no migration is needed for testnet or production deployments.

## Next Steps
1. Implement storage helper + collector hydration utility.
2. Wire pool + translator init paths to call the helper for each downstream/miner.
3. Add unit tests for hydration logic.
4. Verify via `devenv` restart that graphs remain stable.

With this warm-start in place, the stats service always has enough real shares in memory to report accurate hashrate immediately after a restart—no synthetic data, no exaggerated upside, and no crater.
