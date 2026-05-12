# Time Series Hashrate Metrics Collection Plan

## Overview

Implement clean, share-based hashrate data collection across translator and pool services, enabling high-quality time-series dashboards without impacting core mining service performance.

## Architecture

```
Translator
  ├─ tracks per-downstream (miner) metrics
  └─ sends snapshots to stats-proxy instance

Pool
  ├─ tracks per-downstream (translator connection) metrics
  └─ sends snapshots to stats-pool instance

stats-sv2 (library - NEW)
  ├─ shared snapshot types
  ├─ metrics calculations
  ├─ storage abstraction
  └─ time-series queries

stats-proxy (existing service)
  ├─ uses stats-sv2 library
  ├─ receives translator snapshots
  └─ stores to SQLite + serves queries

stats-pool (existing service)
  ├─ uses stats-sv2 library
  ├─ receives pool snapshots
  └─ stores to SQLite + serves queries

web-proxy
  └─ queries stats-proxy for dashboards

web-pool
  └─ queries stats-pool for dashboards
```

## Data Collection Strategy

### Share Difficulty

Bitcoin's proof-of-work uses a **target** (32-byte value) to determine difficulty:

```
difficulty = max_target / target
```

Where `max_target` is the genesis block target. This is already implemented in:
`/hashpool/protocols/v2/channels-sv2/src/target.rs:target_to_difficulty()`

### Hashrate Calculation

```
hashrate (H/s) = sum_of_share_difficulties / time_window_seconds
```

For MVP with 10-second windows:
```
hashrate = sum_difficulty / 10
```

### Data Collection Approach: Aggregated

**Why aggregated instead of raw shares:**
- Minimal overhead: just 2 atomic operations per share (increment counter, add difficulty)
- High-quality data: aggregate share counts + sum of difficulties (no precision loss)
- Flexible: can recompute hashrate with different time windows
- Clean schema: easy to serialize and store

## Shared Library: stats-sv2

New crate in `roles/roles-utils/stats-sv2/`

### Snapshot Types

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServiceType {
    Translator,
    Pool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownstreamSnapshot {
    pub downstream_id: u32,
    pub name: String,
    pub address: String,

    // Lifetime stats
    pub shares_lifetime: u64,

    // Current window (10 seconds)
    pub shares_in_window: u64,
    pub sum_difficulty_in_window: f64,

    // Metadata
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSnapshot {
    pub service_type: ServiceType,
    pub downstreams: Vec<DownstreamSnapshot>,
    pub timestamp: u64,
}
```

### Metrics Module

```rust
pub mod metrics {
    pub fn derive_hashrate(
        sum_difficulty: f64,
        window_seconds: u64,
    ) -> f64 {
        if window_seconds == 0 {
            0.0
        } else {
            sum_difficulty / window_seconds as f64
        }
    }
}
```

### Storage Module

Provides SQLite backend for time-series storage:

```rust
pub trait StatsStorage {
    async fn store_downstream(&self, downstream: &DownstreamSnapshot) -> Result<()>;

    async fn query_hashrate(
        &self,
        downstream_id: u32,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<HashratePoint>>;

    async fn query_aggregate_hashrate(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<HashratePoint>>;
}

pub struct SqliteStorage { /* implementation */ }
```

SQLite schema:
```sql
CREATE TABLE downstreams (
    id INTEGER PRIMARY KEY,
    downstream_id INTEGER,
    name TEXT,
    address TEXT
);

CREATE TABLE hashrate_samples (
    timestamp INTEGER,
    downstream_id INTEGER,
    shares_in_window INTEGER,
    sum_difficulty REAL,
    shares_lifetime INTEGER,

    PRIMARY KEY (timestamp, downstream_id)
);

CREATE INDEX idx_timestamp_downstream
ON hashrate_samples(timestamp, downstream_id);
```

## Translator Changes

### Add metrics tracking to MinerTracker

Update `translator/src/lib/miner_stats.rs`:

```rust
#[derive(Debug, Clone)]
pub struct MinerInfo {
    pub id: u32,
    pub name: String,
    pub address: SocketAddr,
    pub connected_time: Instant,
    pub shares_submitted: u64,           // Lifetime total
    pub last_share_time: Option<Instant>,

    // NEW: Current 10-second window metrics
    pub shares_in_window: u64,
    pub sum_difficulty_in_window: f64,
    pub window_start: Instant,
}
```

### Record shares with difficulty

In `translator/src/lib/sv1/downstream/message_handler.rs` where `increment_shares()` is called:

**Before:**
```rust
miner_tracker.increment_shares(miner_id, current_hashrate).await;
```

**After:**
```rust
// Extract target from channel state
let target = get_current_target_for_downstream(downstream_id);
let difficulty = target_to_difficulty(target);
miner_tracker.record_share(miner_id, difficulty).await;
```

Add to MinerTracker:
```rust
pub async fn record_share(&self, id: u32, difficulty: f64) {
    let mut miners = self.miners.write().await;
    if let Some(miner) = miners.get_mut(&id) {
        miner.shares_submitted += 1;
        miner.last_share_time = Some(Instant::now());
        miner.sum_difficulty_in_window += difficulty;
        miner.shares_in_window += 1;
    }
}
```

### Update snapshot

In `translator/src/lib/stats_integration.rs`:

```rust
impl StatsSnapshotProvider for TranslatorSv2 {
    type Snapshot = ServiceSnapshot;

    fn get_snapshot(&self) -> ServiceSnapshot {
        let downstreams = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                self.miner_tracker.get_all_miners().await
                    .into_iter()
                    .map(|miner| DownstreamSnapshot {
                        downstream_id: miner.id,
                        name: miner.name,
                        address: if self.config.redact_ip {
                            "REDACTED".to_string()
                        } else {
                            miner.address.to_string()
                        },
                        shares_lifetime: miner.shares_submitted,
                        shares_in_window: miner.shares_in_window,
                        sum_difficulty_in_window: miner.sum_difficulty_in_window,
                        timestamp: unix_timestamp(),
                    })
                    .collect()
            })
        });

        ServiceSnapshot {
            service_type: ServiceType::Translator,
            downstreams,
            timestamp: unix_timestamp(),
        }
    }
}
```

## Pool Changes

### Extend DownstreamStats

Update `roles/roles-utils/pool-stats/src/lib.rs`:

```rust
pub struct DownstreamStats {
    pub shares_submitted: AtomicU64,
    pub sum_difficulty: AtomicU64,       // NEW
    pub window_start: AtomicU64,         // NEW
    pub quotes_created: AtomicU64,
    pub ehash_mined: AtomicU64,
    pub last_share_at: AtomicU64,
}

impl DownstreamStats {
    pub fn record_share(&self, difficulty: f64) {
        let now = unix_timestamp();
        self.shares_submitted.fetch_add(1, Ordering::Relaxed);

        // Convert f64 difficulty to fixed-point u64 (multiply by 2^32 to avoid floats)
        let difficulty_fixed = (difficulty * (2u64.pow(32) as f64)) as u64;
        self.sum_difficulty.fetch_add(difficulty_fixed, Ordering::Relaxed);

        self.last_share_at.store(now, Ordering::Relaxed);
    }

    pub fn get_sum_difficulty(&self) -> f64 {
        let fixed = self.sum_difficulty.load(Ordering::Relaxed);
        fixed as f64 / (2u64.pow(32) as f64)
    }
}
```

### Record shares with difficulty

In `pool/src/lib/mining_pool/message_handler.rs` after `validate_share()`:

**Before:**
```rust
vardiff.increment_shares_since_last_update();
```

**After:**
```rust
// Extract target from share message/channel
let difficulty = target_to_difficulty(share.target());
stats.record_share(difficulty);
```

### Update snapshot

In `pool/src/lib/stats_integration.rs`:

```rust
impl StatsSnapshotProvider for Pool {
    type Snapshot = ServiceSnapshot;

    fn get_snapshot(&self) -> ServiceSnapshot {
        let stats_snapshot = self.stats_registry.snapshot();

        let downstreams = self.downstreams
            .iter()
            .filter_map(|(id, downstream)| {
                let (address, _) = downstream.safe_lock(|d| {
                    (d.address.to_string(), d.requires_custom_work)
                }).ok()?;

                let stats = self.stats_registry.get_stats(*id)?;
                let (shares, _, _, _) = stats_snapshot.get(id).copied().unwrap_or((0, 0, 0, None));

                Some(DownstreamSnapshot {
                    downstream_id: *id,
                    name: format!("translator_{}", id),
                    address,
                    shares_lifetime: shares,
                    shares_in_window: stats.shares_in_current_window(),
                    sum_difficulty_in_window: stats.get_sum_difficulty_in_window(),
                    timestamp: unix_timestamp(),
                })
            })
            .collect();

        ServiceSnapshot {
            service_type: ServiceType::Pool,
            downstreams,
            timestamp: unix_timestamp(),
        }
    }
}
```

## stats-proxy and stats-pool Updates

Both services already exist and use the stats library. Changes needed:

1. **Add dependency on stats-sv2 library**
   ```toml
   stats-sv2 = { path = "../roles-utils/stats-sv2" }
   ```

2. **Update storage layer** to use `stats-sv2::storage::SqliteStorage`

3. **Update snapshot handling** to accept `ServiceSnapshot` from stats-sv2

4. **Keep backwards compatibility** with existing `stats_client` and `stats_poller`

## Web Service Queries

Both `web-proxy` and `web-pool` query their respective stats services:

```rust
// Query per-downstream hashrate
GET /api/downstream/:id/hashrate?from=<timestamp>&to=<timestamp>

// Query aggregate hashrate
GET /api/hashrate?from=<timestamp>&to=<timestamp>
```

Response format (same for both):
```json
{
  "data": [
    {
      "timestamp": 1234567890,
      "hashrate_hs": 1500000000000.0
    },
    ...
  ]
}
```

## Implementation Checklist

### Phase 1: Shared Library
- [ ] Create `roles/roles-utils/stats-sv2/` crate
- [ ] Define `DownstreamSnapshot` and `ServiceSnapshot` types
- [ ] Implement `metrics::derive_hashrate()` function
- [ ] Implement `storage::SqliteStorage` with schema
- [ ] Add tests for metrics calculations

### Phase 2: Translator
- [ ] Add `shares_in_window` and `sum_difficulty_in_window` to `MinerInfo`
- [ ] Extract target from share messages in downstream handler
- [ ] Implement `record_share(difficulty)` in `MinerTracker`
- [ ] Update snapshot to include window metrics
- [ ] Test that shares are being recorded with correct difficulties

### Phase 3: Pool
- [ ] Add `sum_difficulty` field to `DownstreamStats`
- [ ] Extract target from share messages in message handler
- [ ] Implement `record_share(difficulty)` in `DownstreamStats`
- [ ] Update snapshot to include window metrics
- [ ] Test that shares are being recorded with correct difficulties

### Phase 4: Stats Services
- [ ] Update `stats-proxy` to depend on `stats-sv2`
- [ ] Update `stats-pool` to depend on `stats-sv2`
- [ ] Implement storage layer using `stats-sv2::SqliteStorage`
- [ ] Test snapshot ingestion and storage

### Phase 5: Web Services
- [ ] Add query endpoints to `web-proxy` for hashrate data
- [ ] Add query endpoints to `web-pool` for hashrate data
- [ ] Implement hashrate graphs in UI
- [ ] Test end-to-end data flow

## Performance Considerations

**Overhead per share (zero impact on hot path):**
- Translator: 2 atomic operations (shares counter, difficulty sum)
- Pool: 2 atomic operations (shares counter, difficulty sum)

**Snapshot generation (every 10 seconds):**
- Translator: Iterate miner map, serialize to JSON (~1-2ms for 100 miners)
- Pool: Iterate downstream map, serialize to JSON (~1ms for 10 downstreams)

**Storage (every 10 seconds per service):**
- 1 INSERT per downstream per window
- Indexed queries on timestamp + downstream_id

Total overhead: negligible, no impact on mining performance.

## MVP Scope

Focus on:
- Basic share counting + difficulty aggregation
- 10-second snapshot intervals
- SQLite storage (single node)
- Per-downstream hashrate graphs
- Aggregate pool/translator hashrate

Out of scope for MVP:
- Advanced analytics (variance, stale share rates)
- High-availability replication
- Detailed error/failure analysis
- Difficulty distribution histograms

Can be added later without changing core architecture.
