# Hashpool Architecture Flow Analysis

## Complete Flow Map: Share Validation and Difficulty Assignment

This document maps out the complete flows for:
1. Share validation at translator and pool
2. Difficulty/target assignment to downstream miners
3. Variable difficulty (vardiff) algorithm implementation
4. Where minimum_difficulty is applied vs where it should be applied

---

## 1. SHARE VALIDATION FLOW

### 1.1 TRANSLATOR (SV1 Downstream Miner) - Share Validation

**Location:** `/home/evan/work/hashpool/roles/translator/src/lib/utils.rs` (lines 21-141)
**Function:** `validate_sv1_share()`

**INPUTS:**
- `share`: SV1 `Submit<'static>` message from miner (contains nonce, timestamp, job_id)
- `target`: Target difficulty assigned to this downstream
- `extranonce1`: Server-provided extranonce (from channel setup)
- `version_rolling_mask`: Optional version rolling parameters
- `sv1_server_data`: Reference to shared SV1 job storage
- `channel_id`: Channel ID for job lookup

**VALIDATION STEPS:**
1. **Find Job**: Look up job by job_id in aggregated_valid_jobs or non_aggregated_valid_jobs
2. **Construct Extranonce**: Combine extranonce1 + extranonce2 from share
3. **Calculate Merkle Root**: From coinbase_tx_prefix + extranonce + coinbase_tx_suffix + merkle_path
4. **Build Block Header**: version, prev_blockhash, merkle_root, timestamp, bits, nonce
5. **Hash the Header**: Calculate block hash using SHA-256(SHA-256(header))
6. **Compare Against Target**: If `hash < downstream_target` → Valid share, else → Invalid

**CONSTRAINTS APPLIED:**
- Share must match a known job (in valid_jobs storage)
- Share hash must be less than the DOWNSTREAM TARGET (difficulty set by translator)

**OUTPUT:**
- `Ok(true)` - Share is valid
- `Ok(false)` - Share is valid structurally but doesn't meet target
- `Err(TproxyError)` - Job not found or invalid data

**CODE SNIPPET:**
```rust
// From translator/src/lib/utils.rs:42-141
pub fn validate_sv1_share(
    share: &client_to_server::Submit<'static>,
    target: Target,  // <-- Downstream target to validate against
    extranonce1: Vec<u8>,
    version_rolling_mask: Option<HexU32Be>,
    sv1_server_data: std::sync::Arc<Mutex<Sv1ServerData>>,
    channel_id: u32,
) -> Result<bool, TproxyError> {
    // 1. Find job
    let job = sv1_server_data.super_safe_lock(|server_data| { /* ... */ })?;
    
    // 2. Construct full extranonce
    let mut full_extranonce = vec![];
    full_extranonce.extend_from_slice(extranonce1.as_slice());
    full_extranonce.extend_from_slice(share.extra_nonce2.0.as_ref());
    
    // 3-5. Calculate merkle root, build header, hash it
    let hash = header.block_hash();
    let hash_as_target: Target = raw_hash.into();
    
    // 6. Check against downstream target
    if hash_as_target < target {
        return Ok(true);  // Valid share
    }
    Ok(false)  // Doesn't meet target
}
```

---

### 1.2 POOL (SV2 Downstream Miner) - Share Validation

**Location:** `/home/evan/work/hashpool/roles/pool/src/lib/mining_pool/message_handler.rs` (lines 858-1026 for standard, 1037-1221 for extended)

**Functions:** 
- `handle_submit_shares_standard()` (line 858)
- `handle_submit_shares_extended()` (line 1037)

**INPUTS:**
- `m`: `SubmitSharesStandard` or `SubmitSharesExtended` message
  - Contains: channel_id, sequence_number, nonce, version, ntime, coinbase_tx, etc.

**VALIDATION PROCESS:**
1. **Check Channel Exists**: Verify channel_id maps to a valid standard/extended channel
2. **Get Channel Lock**: Acquire write lock on standard_channel or extended_channel
3. **Get Vardiff Lock**: Acquire write lock on vardiff state (for share counting)
4. **Call validate_share()**: Channel-specific validation method (built-in to StandardChannel/ExtendedChannel)
   - This method checks:
     - Job validity
     - Share meets minimum target threshold
     - Duplicate detection
     - Block hash calculation
5. **Increment Vardiff Counter**: `vardiff.increment_shares_since_last_update()`
6. **Record Metrics**: Store share difficulty for time-series metrics

**VALIDATION RESULTS HANDLED:**
- `ShareValidationResult::Valid(accepted_share)` → Share accepted (no response sent)
- `ShareValidationResult::ValidWithAcknowledgement(...)` → Send SubmitSharesSuccess
- `ShareValidationResult::BlockFound(...)` → Block found! Send solution to Template Provider
- `ShareValidationError::Invalid` → "invalid-share"
- `ShareValidationError::DoesNotMeetTarget` → "difficulty-too-low"
- `ShareValidationError::Stale` → "stale-share"
- `ShareValidationError::InvalidJobId` → "invalid-job-id"
- `ShareValidationError::DuplicateShare` → "duplicate-share"

**CODE SNIPPET:**
```rust
// From pool/src/lib/mining_pool/message_handler.rs:858-905
fn handle_submit_shares_standard(&mut self, m: SubmitSharesStandard) -> Result<SendTo<()>, Error> {
    let channel_id = m.channel_id;
    
    // 1. Check channel exists
    if !self.standard_channels.contains_key(&channel_id) {
        return Err(...);
    }
    
    // 2-3. Get locks
    let mut standard_channel = self.standard_channels.get(&channel_id).unwrap().write()?;
    let mut vardiff = self.vardiff.get(&channel_id).unwrap().write()?;
    
    // 4. Validate share
    let res = standard_channel.validate_share(m.clone());
    
    // 5. Increment vardiff counter
    vardiff.increment_shares_since_last_update();
    
    // 6. Record metrics
    if let Some(stats) = self.stats_registry.get_stats(self.id) {
        let target = standard_channel.get_target().clone();
        let difficulty = target_to_difficulty(target);
        stats.record_share_with_difficulty(difficulty);
    }
    
    // Handle results...
    match res {
        Ok(ShareValidationResult::Valid(accepted_share)) => { /* ... */ }
        // etc.
    }
}
```

---

## 2. DIFFICULTY/TARGET ASSIGNMENT FLOW

### 2.1 Channel Creation - Initial Target Assignment

Both translator and pool use `hash_rate_to_target()` to calculate initial targets from hashrate.

**Location:** `/home/evan/work/hashpool/protocols/v2/channels-sv2/src/target.rs` (lines 109-152)

**Function:** `hash_rate_to_target(hashrate: f64, shares_per_minute: f64) -> Result<U256>`

**FORMULA:**
```
t = (2^256 - sh) / (sh + 1)
where:
  h = hashrate (H/s)
  s = seconds per share (60 / shares_per_minute)
  sh = h * s (hashes per share interval)
  t = target threshold
```

**INPUTS:**
- `hashrate`: Miner's hash rate in hashes per second (f64)
- `shares_per_minute`: Pool's target share frequency (f64)

**OUTPUTS:**
- `Ok(U256)`: Target as 256-bit value (big-endian, little-endian representation)
- `Err(HashRateToTargetError)`: Division by zero or negative inputs

**CONSTRAINTS:**
- Requires `shares_per_minute > 0`
- Requires `hashrate >= 0`

**EXAMPLE:**
```
If hashrate = 1 TH/s = 1e12 H/s and shares_per_minute = 5:
  s = 60 / 5 = 12 seconds per share
  sh = 1e12 * 12 = 1.2e13 hashes per share
  target = (2^256 - 1.2e13) / (1.2e13 + 1)
  
This gives a target where expected share rate = 5 shares/minute
```

---

### 2.2 Translator (SV1) - Target Assignment to Downstream

#### A. INITIAL CONNECTION - No Vardiff

**Location:** `/home/evan/work/hashpool/roles/translator/src/lib/sv1/downstream/downstream.rs`

When a new SV1 miner connects, they initially receive a SetDifficulty message with a starting target.
If no hashrate provided, a default initial_target is used.

#### B. WITH VARDIFF ENABLED

**Location:** `/home/evan/work/hashpool/roles/translator/src/lib/sv1/sv1_server/difficulty_manager.rs`

**Function:** `spawn_vardiff_loop()` (line 47) - Runs every 60 seconds

**INPUTS TO VARDIFF LOOP:**
- `sv1_server_data`: Contains all downstreams, their current targets, hashrates, upstream targets
- `shares_per_minute`: Pool's target (from config)
- `is_aggregated`: Whether running in aggregated mode

**VARDIFF ALGORITHM (lines 165-247):**

1. **For Each Downstream:**
   ```rust
   let new_hashrate_opt = vardiff.try_vardiff(
       hashrate,           // Current hashrate
       &target,           // Current target
       shares_per_minute  // Target share frequency
   );
   ```

2. **If Hashrate Changed:**
   ```rust
   let new_target = hash_rate_to_target(
       new_hashrate as f64,
       shares_per_minute as f64
   )?;
   ```

3. **Store Pending State:**
   - Set pending_target and pending_hashrate
   - Update miner_tracker with new hashrate

4. **Target Comparison Logic:**
   - **IF** `new_target >= upstream_target`: Send SetDifficulty immediately
   - **ELSE** `new_target < upstream_target`: Store as pending update (wait for SetTarget response)

5. **Send UpdateChannel Messages:**
   - Aggregated mode: Single UpdateChannel with min_target of ALL downstreams + sum of hashrates
   - Non-aggregated mode: Individual UpdateChannel for each downstream

**VARDIFF STATE (from Vardiff trait):**
- Tracks share count since last update
- Implements exponential smoothing or moving average for hashrate estimation
- Applies min/max bounds on difficulty changes

**CODE SNIPPET:**
```rust
// From difficulty_manager.rs:125-278
async fn handle_vardiff_updates(&self, sv1_server_data, channel_manager_sender, 
                                 sv1_server_to_downstream_sender) {
    for (downstream_id, vardiff_state) in vardiff_map.iter() {
        let mut vardiff = vardiff_state.write().unwrap();
        
        // 1. Get current state
        let (channel_id, hashrate, target, upstream_target) = sv1_server_data.super_safe_lock(|data| {
            data.downstreams.get(downstream_id).and_then(|ds| {
                ds.downstream_data.super_safe_lock(|d| {
                    Some((d.channel_id, d.hashrate?, d.target.clone(), d.upstream_target.clone()))
                })
            })
        })?;
        
        // 2. Try vardiff
        let new_hashrate_opt = vardiff.try_vardiff(hashrate, &target, shares_per_minute);
        
        if let Ok(Some(new_hashrate)) = new_hashrate_opt {
            // 3. Calculate new target
            let new_target = hash_rate_to_target(new_hashrate as f64, shares_per_minute as f64)?;
            
            // 4. Store pending
            sv1_server_data.safe_lock(|dmap| {
                if let Some(d) = dmap.downstreams.get(downstream_id) {
                    d.downstream_data.safe_lock(|dd| {
                        dd.set_pending_target(new_target.clone());
                        dd.set_pending_hashrate(Some(new_hashrate));
                    });
                }
            });
            
            // 5. Determine when to send
            match upstream_target {
                Some(upstream_target) if new_target >= upstream_target => {
                    // Send immediately
                    immediate_updates.push((channel_id, Some(downstream_id), new_target));
                }
                Some(upstream_target) if new_target < upstream_target => {
                    // Store as pending
                    sv1_server_data.super_safe_lock(|data| {
                        data.pending_target_updates.push(PendingTargetUpdate {
                            downstream_id,
                            new_target,
                            new_hashrate,
                        });
                    });
                }
                None => {
                    // No upstream target yet, send immediately as fallback
                    immediate_updates.push((channel_id, Some(downstream_id), new_target));
                }
            }
        }
    }
    
    // 6. Send UpdateChannel messages
    if !all_updates.is_empty() {
        self.send_update_channel_messages(all_updates, sv1_server_data, channel_manager_sender).await;
    }
    
    // 7. Send immediate SetDifficulty
    for (channel_id, downstream_id, target) in immediate_updates {
        let set_difficulty_msg = build_sv1_set_difficulty_from_sv2_target(target)?;
        sv1_server_to_downstream_sender.send((channel_id, downstream_id, set_difficulty_msg))?;
    }
}
```

---

### 2.3 Pool (SV2) - Target Assignment to Downstream

#### A. INITIAL CHANNEL OPEN

**Location:** `/home/evan/work/hashpool/roles/pool/src/lib/mining_pool/message_handler.rs`

**Functions:**
- `handle_open_standard_mining_channel()` (line 262)
- `handle_open_extended_mining_channel()` (line 493)

**STEPS:**
1. **Clamp Nominal Hashrate:**
   ```rust
   let mut nominal_hash_rate = incoming.nominal_hash_rate;
   if nominal_hash_rate < self.min_individual_miner_hashrate {
       nominal_hash_rate = self.min_individual_miner_hashrate;
   }
   ```
   **WHERE MINIMUM IS APPLIED:** Here on line 321/505 - Pool enforces minimum hashrate

2. **Create Channel with Target:**
   ```rust
   let mut standard_channel = StandardChannel::new_for_pool(
       channel_id,
       user_identity,
       extranonce_prefix,
       requested_max_target.into(),  // From miner's request
       nominal_hash_rate,            // Clamped to minimum
       self.share_batch_size,
       self.shares_per_minute,
       job_store,
       self.pool_tag_string.clone(),
   )?;
   ```

3. **Create Vardiff State with Minimum:**
   ```rust
   let vardiff = VardiffState::new_with_min(self.min_individual_miner_hashrate)?;
   ```
   **WHERE VARDIFF MIN IS SET:** Line 465/691 - VardiffState created with minimum

4. **Send OpenMiningChannelSuccess** with calculated target

---

#### B. WITH UPDATECHANNEL (Vardiff Adjustment)

**Location:** `/home/evan/work/hashpool/roles/pool/src/lib/mining_pool/message_handler.rs` (line 710)

**Function:** `handle_update_channel(m: UpdateChannel)`

**STEPS:**
1. **Clamp New Hashrate:**
   ```rust
   let mut new_nominal_hash_rate = m.nominal_hash_rate;
   if new_nominal_hash_rate < self.min_individual_miner_hashrate {
       new_nominal_hash_rate = self.min_individual_miner_hashrate;
   }
   ```
   **WHERE MINIMUM IS APPLIED:** Line 715-722

2. **Update Channel:**
   ```rust
   let res = standard_channel.update_channel(
       new_nominal_hash_rate,
       Some(requested_maximum_target.into())
   );
   ```

3. **Send SetTarget** with new target

---

#### C. ON SHARE SUBMISSION

**Location:** `/home/evan/work/hashpool/roles/pool/src/lib/mining_pool/message_handler.rs` (lines 887-895)

**Steps:**
1. Get standard_channel lock
2. Get vardiff lock
3. Call `standard_channel.validate_share(m.clone())`
4. Call `vardiff.increment_shares_since_last_update()`

Note: **Vardiff is NOT called automatically in pool** - it only increments share counter.
Pool relies on miner sending UpdateChannel messages (translator would send these).

---

## 3. MINIMUM DIFFICULTY APPLICATION

### 3.1 Current Application Locations

**TRANSLATOR (SV1):**
- `config.rs` line 188: `set_min_hashrate_from_difficulty()` - Sets min_individual_miner_hashrate from mint's minimum
- `sv1_server.rs` line 90: Gets shares_per_minute from config
- `difficulty_manager.rs` line 47: vardiff loop uses shares_per_minute
- **NO EXPLICIT MINIMUM ENFORCEMENT** in vardiff updates - relies on VardiffState bounds

**POOL (SV2):**
- `message_handler.rs` line 321/505: Clamps nominal_hash_rate to min_individual_miner_hashrate
- `message_handler.rs` line 465/691: Creates VardiffState with min constraint
- `message_handler.rs` line 715-722: Clamps UpdateChannel nominal_hash_rate to minimum

### 3.2 Where Minimum_Difficulty SHOULD Be Applied

#### For Translator:
1. **On initial downstream connection** - ensure starting target respects minimum difficulty
2. **In vardiff loop** - ensure adjusted targets never go below minimum
3. **When clamping hashrate in vardiff** - prevent target from getting too easy

#### For Pool:
1. ✓ **On OpenMiningChannel** - Already clamped
2. ✓ **On UpdateChannel** - Already clamped  
3. ✓ **In VardiffState creation** - Already applied with new_with_min()
4. ✓ **On share validation** - Target already enforced

---

## 4. VARDIFF ALGORITHM DETAILS

### 4.1 Core Vardiff Trait (from stratum-common)

**Used Types:**
- `Vardiff` trait: Define interface for variable difficulty
- `VardiffState`: Implementation of variable difficulty algorithm

**Key Methods:**
- `try_vardiff(current_hashrate, current_target, shares_per_minute) -> Result<Option<f32>>`
  - Returns new hashrate if adjustment needed, None otherwise
- `increment_shares_since_last_update()`
  - Count shares for the adjustment window

**Algorithm Properties:**
- Runs periodically (60-second window in translator)
- Uses exponential smoothing or moving averages
- Adjusts difficulty up/down based on share submission rate
- Applies min/max multiplier bounds to prevent wild swings

### 4.2 Share Rate Target

**Configuration:**
```rust
// translator/src/lib/config.rs
pub shares_per_minute: f32,  // Target share frequency (e.g., 5.0)
```

**Usage:**
- `hash_rate_to_target()` converts hashrate + shares_per_minute → target
- If hashrate increases → target decreases (harder, fewer shares)
- If hashrate decreases → target increases (easier, more shares)

**Example:**
```
If shares_per_minute = 5 and current_hashrate = 100 GH/s:
  target = hash_rate_to_target(100e9, 5.0)
  Expected shares: 5 per minute = 1 share per 12 seconds

If shares_per_minute = 5 and hashrate increases to 200 GH/s:
  target = hash_rate_to_target(200e9, 5.0)
  New target is harder (smaller) so still 1 share per 12 seconds
```

---

## 5. DATA STRUCTURES AND STATE

### 5.1 Translator Downstream State

**Location:** `/home/evan/work/hashpool/roles/translator/src/lib/sv1/downstream/data.rs`

```rust
pub struct DownstreamData {
    pub channel_id: Option<u32>,           // SV2 channel ID
    pub downstream_id: u32,                 // Miner identifier
    pub target: Target,                     // Current target (32 bytes, big-endian)
    pub hashrate: Option<f32>,              // Current estimated hashrate
    pub upstream_target: Option<Target>,    // Target from upstream SV2 channel
    pub pending_target: Option<Target>,     // Target waiting for SetTarget approval
    pub pending_hashrate: Option<f32>,      // Hashrate waiting for SetTarget approval
}
```

### 5.2 Pool Downstream State

**Location:** `/home/evan/work/hashpool/roles/pool/src/lib/mining_pool/mod.rs`

```rust
pub struct Downstream {
    pub id: u32,
    pub address: SocketAddr,
    pub requires_custom_work: bool,
    pub min_individual_miner_hashrate: f32,  // Minimum difficulty floor
    pub standard_channels: HashMap<u32, Arc<RwLock<StandardChannel>>>,
    pub extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel>>>,
    pub vardiff: HashMap<u32, Arc<RwLock<VardiffState>>>,
}
```

### 5.3 SV1 Server Data (Translator)

**Location:** `/home/evan/work/hashpool/roles/translator/src/lib/sv1/sv1_server/data.rs`

```rust
pub struct Sv1ServerData {
    pub downstreams: HashMap<u32, Arc<Downstream>>,
    pub vardiff: HashMap<u32, Arc<RwLock<VardiffState>>>,
    pub pending_target_updates: Vec<PendingTargetUpdate>,
    pub initial_target: Option<Target>,
    pub aggregated_valid_jobs: Option<Vec<server_to_client::Notify>>,
    pub non_aggregated_valid_jobs: Option<HashMap<u32, Vec<server_to_client::Notify>>>,
}

pub struct PendingTargetUpdate {
    pub downstream_id: u32,
    pub new_target: Target,
    pub new_hashrate: f32,
}
```

---

## 6. MESSAGE FLOW DIAGRAMS

### 6.1 Share Submission Flow (Translator)

```
SV1 Miner
    |
    | mining.submit
    v
DownstreamData.handle_submit()
    |
    v
validate_sv1_share()  [CHECK: hash < downstream_target]
    |
    +-- Invalid? --> Send mining.error
    |
    +-- Valid? --> SubmitShareWithChannelId
        |
        v
    Sv1Server (sends to upstream)
        |
        v
    UpstreamChannelManager (SV2)
```

### 6.2 Share Submission Flow (Pool)

```
SV2 Miner
    |
    | SubmitSharesStandard/Extended
    v
Downstream.handle_submit_shares_standard()
    |
    v
StandardChannel.validate_share()  [CHECK: hash < channel_target]
    |
    +-- Invalid? --> SubmitSharesError
    |
    +-- Valid? --> SubmitSharesSuccess (or block found!)
        |
        v
    Record metrics + Quote dispatch
```

### 6.3 Difficulty Adjustment Flow (Translator - Vardiff)

```
Timer: 60 second interval
    |
    v
DifficultyManager.spawn_vardiff_loop()
    |
    v (Every 60 seconds)
    +-- For each downstream:
        |
        +-- vardiff.try_vardiff()  [analyze share rate]
            |
            +-- Hashrate changed?
                |
                +-- YES:
                    |
                    +-- new_target = hash_rate_to_target(new_hashrate, shares_per_minute)
                        |
                        v
                    |new_target >= upstream_target?|
                    |
                    +-- YES: Send SetDifficulty immediately
                    |
                    +-- NO: Store as PendingTargetUpdate
                        |
                        v (wait for SetTarget from upstream)
                        DifficultyManager.handle_set_target_message()
                            |
                            v
                            Send pending SetDifficulty
    |
    +-- Send UpdateChannel (aggregated or per-downstream)
        |
        v
    ChannelManager (upstream)
```

### 6.4 Channel Creation with Difficulty (Pool)

```
SV2 Miner: OpenStandardMiningChannel
    |
    | nominal_hash_rate = X
    v
Downstream.handle_open_standard_mining_channel()
    |
    +-- Clamp: max(X, min_individual_miner_hashrate)
        |
        v
    +-- StandardChannel::new_for_pool(clamped_hashrate)
        |
        v
    +-- VardiffState::new_with_min(min_individual_miner_hashrate)
        |
        v
    +-- Send: OpenStandardMiningChannelSuccess + SetTarget
        |
        v
SV2 Miner receives target + channel setup
```

---

## 7. KEY FINDINGS AND ISSUES

### 7.1 Minimum Difficulty Application

**TRANSLATOR:**
- ✓ Minimum is set via mint's `set_min_hashrate_from_difficulty()`
- ✗ **NO EXPLICIT ENFORCEMENT** in vardiff loop - relies on VardiffState bounds
- ✗ **POTENTIAL ISSUE:** If VardiffState minimum is not properly set, targets could go below minimum

**POOL:**
- ✓ Explicitly clamped on OpenMiningChannel (line 321/505)
- ✓ Explicitly clamped on UpdateChannel (line 715/722)
- ✓ VardiffState created with minimum (line 465/691)
- ✓ **WELL PROTECTED**

### 7.2 Target Comparison Logic (Translator Vardiff)

**Current Behavior:**
- When `new_target < upstream_target`: Store as pending, wait for SetTarget response
- Problem: If SetTarget never comes or upstream target never gets low enough, pending update never sends

**Why This Matters:**
- Prevents over-requesting: Miner doesn't ask for harder target than upstream can provide
- Implements backpressure: Waits for upstream permission before sending lower targets

### 7.3 Share Validation Targets

**Translator (SV1):**
- Validates against `downstream_target` only
- This is the target set by SetDifficulty
- **CRITICAL:** Target must be ≤ upstream_target for valid shares

**Pool (SV2):**
- Validates against `channel_target` (via StandardChannel/ExtendedChannel)
- This is the target calculated from nominal_hashrate
- Always enforced via channel constraints

### 7.4 Vardiff Window and Update Frequency

**Translator:**
- Vardiff loop runs every 60 seconds (line 102)
- Accumulates shares over 60-second window
- Updates all downstreams at once

**Pool:**
- Vardiff state created per channel but NOT automatically updated
- Pool relies on miner (translator) to send UpdateChannel messages
- Shares counted via `increment_shares_since_last_update()` (line 895/1072)

---

## 8. CONFIGURATION REFERENCE

### 8.1 Translator Configuration

**File:** `roles/translator/src/lib/config.rs`

```rust
pub struct DownstreamDifficultyConfig {
    pub min_individual_miner_hashrate: f32,     // Minimum hashrate floor
    pub shares_per_minute: f32,                 // Target share frequency
    pub enable_vardiff: bool,                   // Enable automatic adjustments
    minimum_difficulty_bits: u32,               // From mint's minimum
}

// Set via: config.set_min_hashrate_from_difficulty(mint_minimum_bits)
// Uses: 2^bits / (60 / shares_per_minute) to calculate min hashrate
```

### 8.2 Pool Configuration

**File:** `roles/pool/src/lib/mining_pool/mod.rs`

```rust
pub struct Downstream {
    pub min_individual_miner_hashrate: f32,  // From pool config
    pub shares_per_minute: f32,              // Pool's target
}

// Derived from: derive_min_hashrate_from_difficulty(min_bits, shares_per_minute)
```

---

## 9. SUMMARY TABLE

| Component | Location | Input | Processing | Output | Constraints |
|-----------|----------|-------|-----------|--------|-------------|
| **Translator Share Validation** | utils.rs:42 | SV1 share, downstream target | Hash + compare | Valid/Invalid | hash < downstream_target |
| **Pool Share Validation** | message_handler.rs:858 | SV2 share, channel target | Channel.validate_share() | Valid/Invalid/Block | StandardChannel constraints |
| **Hash→Target Conversion** | target.rs:109 | Hashrate, shares/min | t = (2^256-sh)/(sh+1) | U256 target | shares/min > 0 |
| **Translator Vardiff** | difficulty_manager.rs:47 | Shares/min, downstreams | VardiffState.try_vardiff() | New target | Compares to upstream_target |
| **Pool Channel Creation** | message_handler.rs:262 | Nominal hashrate | Clamp to min, create channel | SetTarget | min_individual_miner_hashrate |
| **Pool UpdateChannel** | message_handler.rs:710 | New nominal hashrate | Clamp to min, update channel | SetTarget | min_individual_miner_hashrate |

