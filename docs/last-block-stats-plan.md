# Plan: Add "Last Block Found" Stats to Pool Page

## Goal
Display "Last Block Found" timestamp on the miner's pool.html page to show when the upstream pool last found a block.

## Architecture Overview

### Current Stats Flow
```
Translator → stats-proxy (TCP snapshots) → web-proxy (HTTP API) → pool.html
```

### Data Source
- Translator receives `SetNewPrevHash` messages from Pool via SV2 protocol
- These messages arrive when a new block is found on the Bitcoin network
- By timestamping when these arrive, we know when the pool's chain advanced

## Key Principle: Zero SRI Code Modifications

Following the adapter pattern used for `miner_stats::MinerTracker`, we'll create a separate `PoolStatsTracker` to capture pool-related events without modifying SRI translator code.

This maintains our rebase-friendly architecture where only ~80 lines of SRI code touch stats via trait implementations.

---

## Implementation Plan

### 1. Create New PoolStatsTracker Module (~40 lines)

**New File:** `roles/translator/src/lib/pool_stats.rs`

```rust
use std::sync::{Arc, Mutex};

/// Tracks statistics about the upstream pool
/// This is hashpool-specific code that listens to SV2 events
#[derive(Clone)]
pub struct PoolStatsTracker {
    last_block_found_at: Arc<Mutex<Option<u64>>>,
}

impl PoolStatsTracker {
    pub fn new() -> Self {
        Self {
            last_block_found_at: Arc::new(Mutex::new(None)),
        }
    }

    /// Called when SetNewPrevHash is received (new block found)
    pub fn on_new_block(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        *self.last_block_found_at.lock().unwrap() = Some(now);
    }

    pub fn get_last_block_found_at(&self) -> Option<u64> {
        *self.last_block_found_at.lock().unwrap()
    }
}
```

### 2. Add PoolStatsTracker to TranslatorSv2 (~3 lines)

**File:** `roles/translator/src/lib/mod.rs`

Add to struct:
```rust
pub struct TranslatorSv2 {
    config: ProxyConfig,
    reconnect_wait_time: u64,
    wallet: Option<Arc<Wallet>>,
    mint_client: HttpClient,
    miner_tracker: Arc<miner_stats::MinerTracker>,
    pool_stats_tracker: Arc<pool_stats::PoolStatsTracker>,  // NEW
    global_config: shared_config::MinerGlobalConfig,
}
```

Initialize in constructor:
```rust
pool_stats_tracker: Arc::new(pool_stats::PoolStatsTracker::new()),
```

### 3. Hook Into SetNewPrevHash Handler (~2 lines)

**File:** `roles/translator/src/lib/upstream_sv2/upstream.rs`

In existing `SetNewPrevHash` handler:
```rust
Mining::SetNewPrevHash(m) => {
    self.pool_stats_tracker.on_new_block();  // NEW - just 1 line!

    // ... existing SRI code unchanged ...
    let message = Mining::SetNewPrevHash(m.into_static());
    // ...
}
```

**This is the ONLY line we add to SRI code!** The rest is pure hashpool.

### 4. Update Stats Integration (~3 lines)

**File:** `roles/translator/src/lib/stats_integration.rs`

In `get_snapshot()`:
```rust
ProxySnapshot {
    ehash_balance,
    upstream_pool,
    downstream_miners,
    blockchain_network,
    last_block_found_at: self.pool_stats_tracker.get_last_block_found_at(),  // NEW
    timestamp: unix_timestamp(),
}
```

### 5. Update ProxySnapshot (~2 lines)

**File:** `roles/roles-utils/stats/src/stats_adapter.rs` (hashpool-specific)

```rust
pub struct ProxySnapshot {
    pub ehash_balance: u64,
    pub upstream_pool: Option<PoolConnection>,
    pub downstream_miners: Vec<MinerInfo>,
    pub blockchain_network: String,
    pub last_block_found_at: Option<u64>,  // NEW
    pub timestamp: u64,
}
```

Update test in same file:
```rust
#[test]
fn test_snapshot_serialization() {
    let snapshot = ProxySnapshot {
        ehash_balance: 1000,
        upstream_pool: Some(PoolConnection {
            address: "pool.example.com:3333".to_string(),
        }),
        downstream_miners: vec![...],
        blockchain_network: "testnet4".to_string(),
        last_block_found_at: Some(1234567890),  // NEW
        timestamp: 1234567890,
    };
    // ...
}
```

### 6. Update web-proxy /api/pool endpoint (~2 lines)

**File:** `roles/web-proxy/src/web.rs`

In `get_pool_info()`:
```rust
async fn get_pool_info(storage: Arc<SnapshotStorage>) -> serde_json::Value {
    match storage.get() {
        Some(snapshot) => {
            json!({
                "blockchain_network": snapshot.blockchain_network,
                "upstream_pool": snapshot.upstream_pool,
                "connected": snapshot.upstream_pool.is_some(),
                "last_block_found_at": snapshot.last_block_found_at,  // NEW
            })
        }
        None => {
            json!({
                "blockchain_network": "unknown",
                "upstream_pool": null,
                "connected": false,
                "last_block_found_at": null,  // NEW
            })
        }
    }
}
```

### 7. Update pool.html to Display Data (~15 lines)

**File:** `roles/web-proxy/templates/pool.html`

In JavaScript `updatePoolStatus()` function:
```javascript
fetch('/api/pool')
    .then(response => response.json())
    .then(data => {
        // ... existing connection status code ...

        // Update blockchain network
        if (blockchainEl && data.blockchain_network) {
            blockchainEl.textContent = data.blockchain_network;
        }

        // Update last block found (NEW)
        if (lastBlockEl && data.last_block_found_at) {
            const elapsed = Math.floor(Date.now() / 1000) - data.last_block_found_at;
            if (elapsed < 60) {
                lastBlockEl.textContent = `${elapsed}s ago`;
            } else if (elapsed < 3600) {
                lastBlockEl.textContent = `${Math.floor(elapsed / 60)}m ago`;
            } else if (elapsed < 86400) {
                lastBlockEl.textContent = `${Math.floor(elapsed / 3600)}h ago`;
            } else {
                lastBlockEl.textContent = `${Math.floor(elapsed / 86400)}d ago`;
            }
        } else {
            lastBlockEl.textContent = '-';
        }

        // Block height placeholder
        if (blockHeightEl) blockHeightEl.textContent = '-';
    })
    .catch(e => {
        // ... error handling ...
        if (lastBlockEl) lastBlockEl.textContent = '-';
    });
```

---

## Total Scope

### Code Changes
- **New file:** `pool_stats.rs` (~40 lines)
- **Modified files:** 6 files
- **Total new/modified code:** ~70 lines
- **SRI code modified:** 3 lines (2 in mod.rs, 1 in upstream.rs)

### Testing
- Unit test for `PoolStatsTracker`
- Update `ProxySnapshot` serialization test
- Manual testing: observe "last block found" updates when new blocks arrive

---

## SRI Code Impact Summary

### Files Modified in SRI Code:
1. **`roles/translator/src/lib/upstream_sv2/upstream.rs`** - **1 line added** to `SetNewPrevHash` handler
2. **`roles/translator/src/lib/mod.rs`** - **2 lines added** (field + initialization)

**Total SRI modifications: 3 lines**

### Files That Are Pure Hashpool (Not SRI):
- `roles/translator/src/lib/pool_stats.rs` (new file, ~40 lines)
- `roles/translator/src/lib/stats_integration.rs` (already hashpool-specific)
- `roles/roles-utils/stats/src/stats_adapter.rs` (already hashpool-specific)
- `roles/web-proxy/src/web.rs` (already hashpool-specific)
- `roles/web-proxy/templates/pool.html` (already hashpool-specific)

---

## Rebase Strategy

When rebasing to latest SRI:

1. **Check if `SetNewPrevHash` handler changed** in upstream SRI
   - Location: `roles/translator/src/lib/upstream_sv2/upstream.rs`
   - If handler code changed: Re-apply our 1-line `pool_stats_tracker.on_new_block()` call
   - If handler unchanged: No action needed

2. **Check if `TranslatorSv2` struct changed** in upstream SRI
   - Location: `roles/translator/src/lib/mod.rs`
   - If struct changed: Re-add `pool_stats_tracker` field and initialization
   - If struct unchanged: No action needed

3. **All other code survives rebase** because it's in hashpool-specific files

4. **Test pattern:**
   ```bash
   # After rebase, verify the hook is still there:
   rg "pool_stats_tracker.on_new_block" roles/translator/src/lib/upstream_sv2/

   # Verify field exists:
   rg "pool_stats_tracker: Arc" roles/translator/src/lib/mod.rs
   ```

5. **Validation:**
   - Run `cargo build` to ensure no compilation errors
   - Check stats-integration test still passes
   - Manual test: connect to pool and verify last block updates when new block arrives

---

## Block Height: Open Question

### The Challenge
The SV2 `SetNewPrevHash` message contains the previous block hash but **not** the block height. This is by design in the SV2 protocol.

### Current State
Block height is displayed as `-` (placeholder) in the UI. The field exists in the data structures but is not populated.

### Options for Surfacing Block Height

#### Option A: Query Bitcoind (If Available)
**Approach:** Add optional bitcoind RPC client to `PoolStatsTracker`

**Implementation:**
```rust
pub struct PoolStatsTracker {
    last_block_found_at: Arc<Mutex<Option<u64>>>,
    block_height: Arc<Mutex<Option<u64>>>,  // NEW
    bitcoind_client: Option<Arc<bitcoincore_rpc::Client>>,  // NEW
}

pub fn on_new_block(&self) {
    // ... timestamp logic ...

    // Query block height if bitcoind available
    if let Some(client) = &self.bitcoind_client {
        if let Ok(count) = client.get_block_count() {
            *self.block_height.lock().unwrap() = Some(count);
        }
    }
}
```

**Config** (`config/shared/miner.toml`):
```toml
[bitcoind]
# Optional: enables block height display on pool page
# If not provided, block height shows as "-"
rpc_url = "http://127.0.0.1:18443"
rpc_user = "user"
rpc_password = "pass"
```

**Pros:**
- Simple RPC call: `getblockcount`
- ~30 additional lines of code
- Accurate block height from chain

**Cons:**
- Requires bitcoind to be running and accessible
- Shows miner's chain tip, not necessarily pool's
- Could be on wrong chain/fork during reorgs
- Adds external dependency and configuration

**Trade-off:** Enables block height for full deployments while gracefully degrading for minimal deployments

#### Option B: Infer from SetNewPrevHash Count
**Approach:** Count `SetNewPrevHash` messages and estimate height

**Pros:**
- No bitcoind dependency
- Works in minimal deployments

**Cons:**
- Not accurate (restarts reset count)
- Can't get initial height
- Only works if translator runs continuously from genesis
- Misleading data is worse than no data

**Verdict:** Not recommended

#### Option C: Pool Sends Block Height via SV2 Extension
**Approach:** Extend SV2 protocol with custom message containing block height

**Pros:**
- Shows pool's actual block height (most accurate)
- No miner-side bitcoind needed
- Survives translator restarts

**Cons:**
- Requires custom SV2 protocol extension (breaks standard)
- Major implementation effort on both pool and translator
- Harder to rebase SRI changes
- Not portable to other pools

**Verdict:** Not recommended for now

#### Option D: Leave as `-` for Now
**Approach:** Display placeholder, revisit later

**Pros:**
- Zero effort
- Can add later without breaking changes
- Field is already `Option<u64>` so `None` is valid
- "Last block found" is more useful anyway

**Cons:**
- Incomplete feature
- May confuse users expecting block height

**Verdict:** Acceptable interim state

### Recommendation

**Start with Option D, optionally implement Option A if there's user demand.**

Rationale:
1. "Last block found" is the more valuable metric for miners
2. Block height is less critical (mostly informational)
3. Adding bitcoind dependency increases complexity
4. Can add later based on user feedback
5. Keeps initial implementation focused and minimal

### Future Considerations

**If we decide to support block height:**

1. **Make bitcoind optional** - Support both deployment modes:
   - Minimal: translator only (no block height)
   - Full: translator + bitcoind (with block height)

2. **Configuration design:**
   ```toml
   [bitcoind]
   # Optional section - omit to disable block height
   rpc_url = "http://127.0.0.1:18443"
   rpc_user = "user"
   rpc_password = "pass"
   ```

3. **Graceful degradation:**
   - If config absent → `block_height: None` → UI shows `-`
   - If RPC call fails → log warning, continue showing `-`
   - No crashes or errors

4. **Documentation requirements:**
   - Explain both deployment modes
   - Document when block height is available
   - Clarify it shows miner's chain, not pool's

### Decision Point

**Should hashpool require bitcoin nodes for all deployments?**

This is a broader architectural question that affects:
- User experience (barrier to entry)
- System requirements and resource usage
- Configuration complexity
- Testing matrix
- Documentation scope
- Future features that may need blockchain access

**This decision should be made considering:**
- Target user profile (hobbyists vs professional miners)
- Typical deployment scenarios
- Trade-offs between features and simplicity
- Maintenance burden

**Recommended approach:** Support both modes to maximize flexibility while keeping requirements minimal for basic functionality.
