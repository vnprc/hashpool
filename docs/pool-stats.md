# Pool Stats Collection & Reporting

## Architecture Principle

**Keep SRI code pristine** - Push all stats logic into hashpool extension modules (`roles-utils/`) using the existing **callback pattern** already established by `quote-dispatcher`.

## Current State Analysis

**Good News:**
- `quote-dispatcher` crate already has a `QuoteEventCallback` trait (line 18-21) for extensibility
- Pool creates `QuoteDispatcher` instances per downstream (mod.rs:267)
- `quote_dispatcher.submit_quote()` is already called on every accepted share (message_handler.rs:347, 368)
- Stats service architecture is working (snapshot-based, TCP→HTTP flow)

**Problem:**
Stats integration code (`stats_integration.rs:60-63`) returns hardcoded zeros because it has no visibility into per-downstream share/quote counters.

## Refactored Solution: Stats Callback Extension

Instead of modifying `Downstream` struct (which lives in SRI-adjacent pool code), we:

1. **Create a stats callback module** in hashpool extension space
2. **Use the existing callback mechanism** in `QuoteDispatcher`
3. **Store stats externally** in a hashpool-owned structure
4. **Query stats during snapshot collection** from the external store

---

## Phase 1: Create Stats Extension Module

**Location:** `roles/roles-utils/pool-stats/` (new crate)

**Tasks:**

1. **Create new crate `pool-stats`**
   ```bash
   mkdir -p roles/roles-utils/pool-stats/src
   touch roles/roles-utils/pool-stats/Cargo.toml
   ```

2. **Define stats storage structure** (`roles-utils/pool-stats/src/lib.rs`)
   ```rust
   use std::sync::Arc;
   use std::sync::atomic::{AtomicU64, Ordering};
   use std::collections::HashMap;
   use parking_lot::RwLock;

   /// Per-downstream stats tracked externally from SRI code
   pub struct DownstreamStats {
       pub shares_submitted: AtomicU64,
       pub quotes_created: AtomicU64,
       pub ehash_mined: AtomicU64,
       pub last_share_at: AtomicU64,
   }

   impl DownstreamStats {
       pub fn new() -> Self {
           Self {
               shares_submitted: AtomicU64::new(0),
               quotes_created: AtomicU64::new(0),
               ehash_mined: AtomicU64::new(0),
               last_share_at: AtomicU64::new(0),
           }
       }
   }

   /// Global stats registry for all downstreams
   pub struct PoolStatsRegistry {
       stats: RwLock<HashMap<u32, Arc<DownstreamStats>>>,
   }

   impl PoolStatsRegistry {
       pub fn new() -> Arc<Self> {
           Arc::new(Self {
               stats: RwLock::new(HashMap::new()),
           })
       }

       pub fn register_downstream(&self, downstream_id: u32) -> Arc<DownstreamStats> {
           let stats = Arc::new(DownstreamStats::new());
           self.stats.write().insert(downstream_id, stats.clone());
           stats
       }

       pub fn unregister_downstream(&self, downstream_id: u32) {
           self.stats.write().remove(&downstream_id);
       }

       pub fn get_stats(&self, downstream_id: u32) -> Option<Arc<DownstreamStats>> {
           self.stats.read().get(&downstream_id).cloned()
       }

       pub fn snapshot(&self) -> HashMap<u32, (u64, u64, u64, Option<u64>)> {
           self.stats.read().iter().map(|(id, stats)| {
               let shares = stats.shares_submitted.load(Ordering::Relaxed);
               let quotes = stats.quotes_created.load(Ordering::Relaxed);
               let ehash = stats.ehash_mined.load(Ordering::Relaxed);
               let last_share = stats.last_share_at.load(Ordering::Relaxed);
               let last_share_opt = if last_share > 0 { Some(last_share) } else { None };
               (*id, (shares, quotes, ehash, last_share_opt))
           }).collect()
       }
   }
   ```

3. **Implement callback for quote-dispatcher** (`roles-utils/pool-stats/src/lib.rs`)
   ```rust
   use quote_dispatcher::QuoteEventCallback;

   /// Callback that updates stats when quotes are created
   pub struct StatsCallback {
       stats: Arc<DownstreamStats>,
   }

   impl StatsCallback {
       pub fn new(stats: Arc<DownstreamStats>) -> Self {
           Self { stats }
       }
   }

   impl QuoteEventCallback for StatsCallback {
       fn on_quote_created(&self, _channel_id: u32, amount: u64) {
           let now = unix_timestamp();
           self.stats.shares_submitted.fetch_add(1, Ordering::Relaxed);
           self.stats.quotes_created.fetch_add(1, Ordering::Relaxed);
           self.stats.ehash_mined.fetch_add(amount, Ordering::Relaxed);
           self.stats.last_share_at.store(now, Ordering::Relaxed);
       }
   }
   ```

---

## Phase 2: Integrate Stats Registry into Pool

**Minimal SRI code changes:**

1. **Add registry to Pool struct** (`roles/pool/src/lib/mining_pool/mod.rs:214`)
   ```rust
   pub struct Pool {
       // ... existing fields ...
       pub stats_registry: Arc<pool_stats::PoolStatsRegistry>,  // ADD THIS
   }
   ```

2. **Initialize registry in Pool::start()** (`mod.rs:998`)
   ```rust
   let stats_registry = pool_stats::PoolStatsRegistry::new();

   let pool = Arc::new(Mutex::new(Pool {
       // ... existing fields ...
       stats_registry: stats_registry.clone(),  // ADD THIS
   }));
   ```

3. **Register downstream with stats on creation** (`mod.rs:242` in `Downstream::new()`)
   ```rust
   // After creating id (line 260)
   let (stats_registry, minimum_difficulty) = pool
       .safe_lock(|p| (p.stats_registry.clone(), p.minimum_difficulty))
       .map_err(|e| PoolError::PoisonLock(e.to_string()))?;

   // Register this downstream
   let downstream_stats = stats_registry.register_downstream(id);

   // Create callback for quote dispatcher
   let stats_callback = Arc::new(pool_stats::StatsCallback::new(downstream_stats));

   let quote_dispatcher = quote_dispatcher::QuoteDispatcher::new(
       hub,
       sv2_config.clone(),
       minimum_difficulty,
   ).with_callback(stats_callback);  // ATTACH CALLBACK
   ```

4. **Unregister on disconnect** (`mod.rs:325` in receiver loop)
   ```rust
   _ => {
       let res = pool
           .safe_lock(|p| {
               p.stats_registry.unregister_downstream(id);  // ADD THIS
               p.downstreams.remove(&id)
           })
           .map_err(|e| PoolError::PoisonLock(e.to_string()));
       // ... rest of error handling
   }
   ```

---

## Phase 3: Update Stats Integration

**Replace hardcoded zeros with registry lookup:**

1. **Modify `stats_integration.rs`** (`roles/pool/src/lib/stats_integration.rs:15-96`)
   ```rust
   impl StatsSnapshotProvider for Pool {
       type Snapshot = PoolSnapshot;

       fn get_snapshot(&self) -> PoolSnapshot {
           // ... existing services collection code ...

           // Get stats snapshot from registry
           let stats_snapshot = self.stats_registry.snapshot();

           for (id, downstream) in &self.downstreams {
               if let Ok((address, is_jd, channels, work_selection)) = downstream
                   .safe_lock(|d| {
                       // ... existing channel collection ...
                       (
                           d.address.to_string(),
                           d.is_job_declarator(),
                           channels,
                           d.has_work_selection(),
                       )
                   })
               {
                   // Lookup stats from registry
                   let (shares, quotes, ehash, last_share) = stats_snapshot
                       .get(id)
                       .copied()
                       .unwrap_or((0, 0, 0, None));  // REAL VALUES NOW

                   if is_jd {
                       // ... JD handling ...
                   } else {
                       downstream_proxies.push(ProxyConnection {
                           id: *id,
                           address,
                           channels,
                           shares_submitted: shares,      // REAL
                           quotes_created: quotes,         // REAL
                           ehash_mined: ehash,            // REAL
                           last_share_at: last_share,     // REAL
                           work_selection,
                       });
                   }
               }
           }

           // ... rest of snapshot creation ...
       }
   }
   ```

---

## Phase 4: Handle Standard Shares (Non-Extended)

**Problem:** Standard shares (non-extended mining) don't call `quote_dispatcher.submit_quote()`, so they won't increment stats.

**Solution:** Add a share-tracking callback to the stats module:

1. **Add share tracking method** (`roles-utils/pool-stats/src/lib.rs`)
   ```rust
   impl DownstreamStats {
       /// Track a standard share (no quote)
       pub fn record_share(&self) {
           let now = unix_timestamp();
           self.shares_submitted.fetch_add(1, Ordering::Relaxed);
           self.last_share_at.store(now, Ordering::Relaxed);
       }
   }
   ```

2. **Call from standard share handler** (`roles/pool/src/lib/mining_pool/message_handler.rs:290`)
   ```rust
   // In handle_submit_shares_standard, after share accepted:
   roles_logic_sv2::channel_logic::channel_factory::OnNewShare::ShareMeetDownstreamTarget => {
       // Record standard share in stats (no quote)
       if let Ok(stats_registry) = self.pool.safe_lock(|p| p.stats_registry.clone()) {
           if let Some(stats) = stats_registry.get_stats(self.id) {
               stats.record_share();
           }
       }

       let success = SubmitSharesSuccess { /* ... */ };
       Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
   }
   ```

---

## Summary of Changes

**New code (hashpool extensions):**
- `roles/roles-utils/pool-stats/` - New crate (100 lines)
  - `PoolStatsRegistry` - External stats storage
  - `DownstreamStats` - Per-downstream atomic counters
  - `StatsCallback` - Implements `QuoteEventCallback`

**Modified SRI-adjacent code (minimal):**
- `roles/pool/src/lib/mining_pool/mod.rs` - Pool struct (+1 field, +10 lines registration)
- `roles/pool/src/lib/mining_pool/message_handler.rs` - Standard share handler (+5 lines)
- `roles/pool/src/lib/stats_integration.rs` - Snapshot collection (replace hardcoded zeros with registry lookup, ~10 lines)

**No changes needed:**
- `quote-dispatcher` - Already has callback trait
- `stats-pool`, `web-pool` - Already consume the data correctly
- SRI core libraries - Zero modifications

---

## Benefits of This Approach

1. **SRI isolation** - All stats logic lives in `roles-utils/pool-stats`
2. **Rebase-friendly** - Only 3 files in `roles/pool` touched, ~25 total lines changed
3. **Clean separation** - Stats module can evolve independently
4. **Testable** - Stats registry can be tested in isolation
5. **Leverages existing patterns** - Uses callback mechanism already in quote-dispatcher
6. **Thread-safe** - Atomics for lock-free reads during snapshot collection

---

## Data Flow

```
Share arrives → Downstream validates →
  Extended share: quote_dispatcher.submit_quote() → StatsCallback.on_quote_created() →
  Standard share: stats.record_share() →
  Atomics increment →
Stats polling (5s) → Pool.get_snapshot() → stats_registry.snapshot() reads atomics →
Send to stats-pool via TCP → stats-pool stores in memory →
web-pool fetches via HTTP → Dashboard displays
```

---

## Testing

1. **Build and run locally**
   - `cargo build --workspace`
   - Start devenv stack: `devenv up`

2. **Submit test shares**
   - Use CPU miner or test harness to submit shares
   - Watch logs for share acceptance

3. **Verify stats in dashboard**
   - Open http://localhost:8081 (web-pool dashboard)
   - Confirm:
     - Total shares increments
     - Total quotes increments
     - Ehash mined increments (by minimum_difficulty per share)
     - Last share timestamp updates
     - Per-proxy table shows correct values

4. **Check stats service data flow**
   - Verify pool sends snapshots to stats-pool (logs should show "Sent snapshot")
   - Verify stats-pool receives snapshots (logs should show "Received pool snapshot")
   - Verify web-pool fetches from stats-pool API
