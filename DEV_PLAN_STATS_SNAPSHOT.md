# Dev Plan: Stats Snapshot Architecture

## Overview

Replace the current event-driven push notification stats system with a snapshot-based polling architecture. This eliminates race conditions, zombie entries, and state synchronization issues.

**Applies to both:**
- **stats-pool** (pool dashboard showing connected services: mint, JD, translators)
- **stats-proxy** (proxy dashboard showing connected miners)

## Current Problems

### Both Pool and Proxy Stats:
1. **Race conditions** - Disconnect events can arrive before final share events
2. **Zombie entries** - Missed disconnect events leave stale entries on dashboard
3. **State drift** - Stats DB tries to mirror source of truth but gets out of sync
4. **Restart issues** - Stats service loses current state on restart
5. **Complex cleanup** - Manual 15-second stale detection hack (removes everything!)

### Pool-Specific Issues:
6. **Hard-coded "Pool" entry** - Dashboard shows hardcoded pool connection instead of actual downstream connections
7. **Services disappear** - Mint, JD, translators all get cleaned up after 15 seconds of no activity
8. **No connection type info** - Can't distinguish between mint, JD, translator connections properly

## Proposed Architecture

### Message Flow

**Proxy Stats (stats-proxy):**
```
Translator (every 10 seconds):
├─ Query MinerTracker (source of truth for connected miners)
├─ Build MinerSnapshot message
└─ Send via existing TCP connection → stats-proxy

stats-proxy:
├─ Receive MinerSnapshot
├─ Replace in-memory HashMap with new snapshot
├─ Append hashrate samples to DB (time-series)
└─ Dashboard queries in-memory state + DB history
```

**Pool Stats (stats-pool):**
```
Pool (every 10 seconds):
├─ Query internal downstream tracker (source of truth for connected services)
├─ Build ConnectionSnapshot message with proper service types
│  (Translator, Mint, Job Declarator)
└─ Send via existing TCP connection → stats-pool

stats-pool:
├─ Receive ConnectionSnapshot
├─ Replace in-memory HashMap with new snapshot
├─ Append hashrate samples to DB (time-series)
└─ Dashboard queries in-memory state + DB history
```

### Data Storage

**In-Memory (current state):**
- `HashMap<u32, MinerInfo>` - replaced every 10 seconds from snapshot
- No persistence needed - source of truth is MinerTracker

**Database (time-series only):**
- `hashrate_samples` - historical hashrate data for graphs
- `quote_history` - historical payment data
- `balance` - global wallet balance (single row)
- **REMOVE** `current_stats` table entirely

### New Message Types

**For stats-proxy (translator → stats-proxy):**
```rust
pub enum StatsMessage {
    MinerSnapshot {
        miners: Vec<MinerInfo>,
        timestamp: u64,
    },
}

pub struct MinerInfo {
    pub downstream_id: u32,
    pub name: String,
    pub address: Option<String>,  // None if redact_ip=true
    pub hashrate: f64,
    pub shares_submitted: u64,
    pub connected_at: i64,
}
```

**For stats-pool (pool → stats-pool):**
```rust
pub enum StatsMessage {
    ConnectionSnapshot {
        connections: Vec<ConnectionInfo>,
        timestamp: u64,
    },
}

pub struct ConnectionInfo {
    pub downstream_id: u32,
    pub connection_type: String,  // "Translator", "Mint", "Job Declarator"
    pub hashrate: f64,
    pub shares_submitted: u64,
    pub channels: Vec<u32>,
    pub connected_at: i64,
    // Mint-specific
    pub quotes_created: Option<u64>,
    pub ehash_mined: Option<u64>,
}
```

## Implementation Steps

### Phase 1: Add Snapshot Support (Keep Old System Working)

**1a. Proxy Stats (translator + stats-proxy):**
   - [ ] Add `MinerInfo` struct to translator/src/lib/stats_client.rs
   - [ ] Add `MinerSnapshot` variant to `StatsMessage` enum
   - [ ] Create background task in translator that runs every 10 seconds
   - [ ] Query MinerTracker to get all connected miners
   - [ ] Build MinerSnapshot from MinerTracker data
   - [ ] Send via existing StatsHandle

**1b. Proxy Stats Receiver (stats-proxy):**
   - [ ] Add in-memory HashMap to store current miners in stats-proxy
   - [ ] Handle MinerSnapshot message in stats_handler.rs
   - [ ] Replace entire HashMap with snapshot data
   - [ ] Insert hashrate samples into time-series DB

**2a. Pool Stats (pool + stats-pool):**
   - [ ] Add `ConnectionInfo` struct to pool/src/lib/stats_client.rs
   - [ ] Add `ConnectionSnapshot` variant to `StatsMessage` enum
   - [ ] Create background task in pool that runs every 10 seconds
   - [ ] Query downstream connections to get all connected services
   - [ ] Identify connection types (Translator, Mint, Job Declarator)
   - [ ] Build ConnectionSnapshot from downstream data
   - [ ] Send via existing StatsHandle

**2b. Pool Stats Receiver (stats-pool):**
   - [ ] Add in-memory HashMap to store current connections in stats-pool
   - [ ] Handle ConnectionSnapshot message in stats_handler.rs
   - [ ] Replace entire HashMap with snapshot data
   - [ ] Insert hashrate samples into time-series DB
   - [ ] Remove hardcoded "Pool" connection entry

### Phase 2: Update Dashboard to Use In-Memory State

4. **Modify stats service**
   - [ ] Change `/api/miners` to return from HashMap instead of DB
   - [ ] Keep time-series queries using DB
   - [ ] Test that dashboard shows current miners correctly

### Phase 3: Remove Old Event System

5. **Remove event messages** (translator/pool)
   - [ ] Remove all `send_stats()` calls for individual events:
     - DownstreamConnected
     - DownstreamDisconnected
     - ShareSubmitted
     - HashrateUpdate
   - [ ] Remove event handling code from downstream_sv1/downstream.rs
   - [ ] Keep BalanceUpdate and QuoteCreated (wallet-specific)

6. **Remove event handling** (stats service)
   - [ ] Remove handlers for old event types from stats_handler.rs
   - [ ] Remove stale miner cleanup task (no longer needed)
   - [ ] Remove methods: record_share, record_downstream_connected, record_downstream_disconnected, record_hashrate

7. **Database cleanup**
   - [ ] Drop `current_stats` table (or mark deprecated)
   - [ ] Remove all ALTER TABLE migrations for current_stats
   - [ ] Keep only time-series tables

### Phase 4: Testing & Validation

8. **Functional testing**
   - [ ] Start translator with miners, verify dashboard shows them
   - [ ] Disconnect miner, verify it disappears within 10 seconds
   - [ ] Restart stats service, verify it syncs within 10 seconds
   - [ ] Check hashrate_samples being populated

9. **Edge cases**
   - [ ] Stats service starts before translator (should show empty, then populate)
   - [ ] Translator crashes (stats shows empty after 10s)
   - [ ] Network interruption between services
   - [ ] Multiple rapid miner connects/disconnects

## Benefits

✅ **Single source of truth** - MinerTracker is the only place miner state lives
✅ **Self-healing** - Stats automatically syncs with reality every 10 seconds
✅ **No race conditions** - Snapshot is atomic view of current state
✅ **Automatic cleanup** - Disconnected miners vanish when they're not in snapshot
✅ **Restart safe** - Stats service recovers current state within 10 seconds
✅ **Simpler code** - Remove all event tracking, just periodic sync
✅ **Time-series preserved** - Historical hashrate data for graphs

## Risks & Mitigations

**Risk:** 10-second delay before changes appear on dashboard
**Mitigation:** Acceptable for monitoring use case, can reduce to 5s if needed

**Risk:** Increased network traffic (full snapshot every 10s vs events)
**Mitigation:** Minimal - even 1000 miners * 100 bytes = 100KB/10s = 10KB/s

**Risk:** Loss of share-by-share granularity
**Mitigation:** Time-series DB still has samples, just not every individual share

## Timeline Estimate

- Phase 1: 4-6 hours (add snapshot alongside existing)
- Phase 2: 2 hours (update dashboard)
- Phase 3: 3-4 hours (remove old system)
- Phase 4: 2-3 hours (testing)

**Total:** ~12-15 hours of focused development

## Dependencies

- None - fully backwards compatible during Phase 1
- Can deploy incrementally
- Old and new systems can coexist during transition

## Success Criteria

- [ ] Dashboard shows current miners from in-memory state
- [ ] Miners appear/disappear within 10 seconds of connecting/disconnecting
- [ ] No zombie entries on dashboard
- [ ] Stats service restart recovers state within 10 seconds
- [ ] Hashrate graphs work from time-series DB
- [ ] Simpler codebase (fewer LoC in stats handling)
