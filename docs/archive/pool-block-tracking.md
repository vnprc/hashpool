# Pool Block Tracking & Dashboard Display

## Objective

Add **block height** and **last block found** tracking to the pool dashboard, displaying:
- Current block height being mined
- Timestamp of the last block solution found by the pool

## Architecture Principle

**Keep SRI code pristine** - All block tracking logic lives in the `pool-stats` crate using atomic counters and minimal pool code changes.

## Current State Analysis

**What we have:**
- `PoolSnapshot` struct in `stats/stats_adapter.rs` (lines 36-42) - pool snapshot sent to dashboard
- `SetNewPrevHash` messages indicate new blocks (contains `template_id`, `prev_hash`, `header_timestamp`)
- `ShareMeetBitcoinTarget` events in message handler indicate block solutions found
- Dashboard HTML at `roles/web-pool/templates/dashboard.html` ready to display new fields

**What we need:**
- Track current block height from incoming templates
- Track timestamp when pool finds a valid block
- Add fields to `PoolSnapshot` struct
- Display in dashboard UI

## Problem: Block Height Extraction

The Stratum V2 protocol **does not include block height** in `NewTemplate` or `SetNewPrevHash` messages. Block height must be extracted from the **coinbase transaction** prefix (BIP34 requirement).

**BIP34** mandates that block height is encoded as a `scriptSig` in the coinbase transaction input. The height is in the `coinbase_prefix` field of `NewTemplate`.

## Solution Architecture

### Phase 1: Add Block Height Parser to pool-stats

**Location:** `roles/roles-utils/pool-stats/src/lib.rs`

**Tasks:**

1. **Add BIP34 height parser function**
   ```rust
   /// Extract block height from BIP34 coinbase prefix
   ///
   /// BIP34 requires height to be first item in scriptSig as a push operation.
   /// Format: [length_byte][height_bytes_little_endian]
   fn parse_block_height(coinbase_prefix: &[u8]) -> Option<u64> {
       if coinbase_prefix.is_empty() {
           return None;
       }

       // First byte is the push opcode length
       let length = coinbase_prefix[0] as usize;

       // Validate we have enough bytes
       if length == 0 || length > 8 || coinbase_prefix.len() < length + 1 {
           return None;
       }

       // Extract height bytes (little-endian)
       let height_bytes = &coinbase_prefix[1..=length];
       let mut height: u64 = 0;
       for (i, &byte) in height_bytes.iter().enumerate() {
           height |= (byte as u64) << (i * 8);
       }

       Some(height)
   }
   ```

2. **Add block tracking to PoolStatsRegistry**
   ```rust
   pub struct PoolStatsRegistry {
       stats: RwLock<HashMap<u32, Arc<DownstreamStats>>>,
       current_block_height: AtomicU64,           // ADD THIS
       last_block_found_at: AtomicU64,            // ADD THIS
   }

   impl PoolStatsRegistry {
       pub fn new() -> Arc<Self> {
           Arc::new(Self {
               stats: RwLock::new(HashMap::new()),
               current_block_height: AtomicU64::new(0),      // ADD
               last_block_found_at: AtomicU64::new(0),       // ADD
           })
       }

       /// Update current block height from template
       pub fn update_block_height(&self, coinbase_prefix: &[u8]) {
           if let Some(height) = parse_block_height(coinbase_prefix) {
               self.current_block_height.store(height, Ordering::Relaxed);
           }
       }

       /// Record that a block was found
       pub fn record_block_found(&self) {
           let now = unix_timestamp();
           self.last_block_found_at.store(now, Ordering::Relaxed);
       }

       /// Get current block height
       pub fn get_block_height(&self) -> u64 {
           self.current_block_height.load(Ordering::Relaxed)
       }

       /// Get last block found timestamp (0 if none)
       pub fn get_last_block_found_at(&self) -> Option<u64> {
           let timestamp = self.last_block_found_at.load(Ordering::Relaxed);
           if timestamp > 0 {
               Some(timestamp)
           } else {
               None
           }
       }
   }
   ```

3. **Update snapshot method to include block data**
   ```rust
   pub fn snapshot(&self) -> (HashMap<u32, (u64, u64, u64, Option<u64>)>, u64, Option<u64>) {
       let downstream_stats = self.stats
           .read()
           .iter()
           .map(|(id, stats)| {
               // ... existing code ...
           })
           .collect();

       let block_height = self.get_block_height();
       let last_block = self.get_last_block_found_at();

       (downstream_stats, block_height, last_block)
   }
   ```

---

### Phase 2: Hook Template Updates in Pool

**Location:** `roles/pool/src/lib/mining_pool/mod.rs`

**Task:** Call `update_block_height()` when new templates arrive

1. **Update `on_new_template` handler** (around line 915)
   ```rust
   async fn on_new_template(
       self_: Arc<Mutex<Self>>,
       rx: Receiver<NewTemplate<'static>>,
       sender_message_received_signal: Sender<()>,
   ) -> PoolResult<()> {
       let status_tx = self_.safe_lock(|s| s.status_tx.clone())?;
       while let Ok(new_template) = rx.recv().await {
           debug!("New template received: {:?}", new_template.template_id);

           // Extract and update block height from coinbase prefix
           if let Ok(stats_registry) = self_.safe_lock(|s| s.stats_registry.clone()) {
               stats_registry.update_block_height(new_template.coinbase_prefix.inner_as_ref());
           }

           // ... existing template handling code ...
       }
   }
   ```

---

### Phase 3: Hook Block Solutions in Message Handler

**Location:** `roles/pool/src/lib/mining_pool/message_handler.rs`

**Task:** Call `record_block_found()` when a share meets Bitcoin target

1. **Update both standard and extended share handlers** (lines ~270, ~347)
   ```rust
   roles_logic_sv2::channel_logic::channel_factory::OnNewShare::ShareMeetBitcoinTarget((share,t_id,coinbase,_)) => {
       // Record that we found a block!
       if let Ok(stats_registry) = self.pool.safe_lock(|p| p.stats_registry.clone()) {
           stats_registry.record_block_found();
           if let Some(stats) = stats_registry.get_stats(self.id) {
               stats.record_share();  // existing code
           }
       }

       // ... existing solution submission code ...
   }
   ```

---

### Phase 4: Update Stats Adapter Types

**Location:** `roles/roles-utils/stats/src/stats_adapter.rs`

**Task:** Add new fields to `PoolSnapshot`

1. **Update PoolSnapshot struct** (line 36)
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct PoolSnapshot {
       pub services: Vec<ServiceConnection>,
       pub downstream_proxies: Vec<ProxyConnection>,
       pub listen_address: String,
       pub timestamp: u64,
       pub block_height: u64,              // ADD THIS
       pub last_block_found_at: Option<u64>, // ADD THIS
   }
   ```

---

### Phase 5: Update Stats Integration

**Location:** `roles/pool/src/lib/stats_integration.rs`

**Task:** Populate new fields in snapshot

1. **Update `get_snapshot` implementation** (line 93)
   ```rust
   // Get stats snapshot from registry (existing line 36)
   let (stats_snapshot, block_height, last_block_found_at) = self.stats_registry.snapshot();

   // ... existing downstream collection code ...

   PoolSnapshot {
       services,
       downstream_proxies,
       listen_address: self.listen_address.clone(),
       timestamp: unix_timestamp(),
       block_height,              // ADD THIS
       last_block_found_at,       // ADD THIS
   }
   ```

---

### Phase 6: Update Dashboard UI

**Location:** `roles/web-pool/templates/dashboard.html`

**Tasks:**

1. **Add new stat boxes** (after line 167, before "Connected Proxies" heading)
   ```html
   <div class="stat-box">
       <div>Block Height</div>
       <div class="stat-value" id="block-height">-</div>
   </div>
   <div class="stat-box">
       <div>Last Block Found</div>
       <div class="stat-value" id="last-block-found">Never</div>
   </div>
   ```

2. **Update JavaScript to populate fields** (in `updateStats()` function around line 233)
   ```javascript
   const servicesData = await servicesResponse.json();
   const connectionsData = await connectionsResponse.json();

   const services = servicesData.services || [];
   const proxies = connectionsData.proxies || [];

   // Extract block tracking data
   const blockHeight = servicesData.block_height || 0;
   const lastBlockFoundAt = servicesData.last_block_found_at;

   // Update block height
   document.getElementById('block-height').textContent =
       blockHeight > 0 ? blockHeight.toLocaleString() : '-';

   // Update last block found
   if (lastBlockFoundAt) {
       const date = new Date(lastBlockFoundAt * 1000);
       const timeAgo = getTimeAgo(date);
       document.getElementById('last-block-found').textContent = timeAgo;
   } else {
       document.getElementById('last-block-found').textContent = 'Never';
   }

   // Helper function to format relative time
   function getTimeAgo(date) {
       const seconds = Math.floor((new Date() - date) / 1000);

       if (seconds < 60) return `${seconds}s ago`;
       if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
       if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
       return `${Math.floor(seconds / 86400)}d ago`;
   }
   ```

---

## Summary of Changes

**New code (pool-stats extension):**
- `parse_block_height()` - BIP34 parser (~20 lines)
- `PoolStatsRegistry` - Add 2 atomic fields + 4 methods (~40 lines)
- Updated `snapshot()` method to return block data

**Modified pool code (minimal SRI footprint):**
- `mod.rs` - Hook `on_new_template` to extract height (~5 lines)
- `message_handler.rs` - Hook `ShareMeetBitcoinTarget` to record block found (~5 lines)
- `stats_integration.rs` - Populate new snapshot fields (~5 lines)

**Modified stats adapter:**
- `stats_adapter.rs` - Add 2 fields to `PoolSnapshot` struct (~2 lines)

**Modified dashboard:**
- `dashboard.html` - Add UI elements and update JavaScript (~30 lines)

**Total impact:** ~110 lines, only ~15 lines touch pool code

---

## Testing

1. **Build and run locally**
   - `cargo build -p pool-stats -p pool_sv2`
   - Start devenv stack

2. **Verify block height tracking**
   - Check logs for "New template received" messages
   - Open dashboard at http://localhost:8081
   - Confirm block height displays and increments

3. **Verify block found tracking**
   - Submit shares until one meets Bitcoin target (may require lowering difficulty for testing)
   - Confirm "Last Block Found" updates with timestamp
   - Check that relative time formatting works ("5s ago", "2m ago", etc.)

4. **Edge cases**
   - Verify "Never" displays when no block has been found yet
   - Verify block height shows "-" before first template received
   - Test dashboard refresh updates correctly

---

## Benefits

1. **Minimal SRI footprint** - Only 3 files touched in pool code (~15 lines total)
2. **Clean separation** - All parsing and tracking logic in `pool-stats` crate
3. **Thread-safe** - Atomic operations for lock-free reads during snapshots
4. **Rebase-friendly** - Changes isolated to extension points
5. **Accurate BIP34 parsing** - Handles variable-length height encoding correctly
6. **User-friendly display** - Relative time formatting ("5m ago") more useful than timestamps

---

## Future Enhancements

- Add "blocks found" counter (total lifetime blocks)
- Track block solution per-downstream (which proxy found the block)
- Add pool uptime tracking
- Calculate effective pool hashrate from block solutions
