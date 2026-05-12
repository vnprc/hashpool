# Minimum Difficulty Implementation - Architectural Fix

## Problem Statement

Currently, `[ehash] minimum_difficulty = 32` is used to prevent CPU miners from flooding the pool with low-value shares. However, the implementation constrains **hashrate** instead of **share difficulty**, which breaks vardiff for high-speed miners.

**Current flow:**
```
minimum_difficulty (32)
  → min_individual_miner_hashrate (0.43 GH/s)
  → Clamps ALL targets to <= 2^32 difficulty
  → High-speed miners (1.2 TH/s) get wrong targets
  → Hashrate shows as 0.6 H/s instead of 1.2 TH/s
```

## Root Cause

Two different concepts are conflated:
1. **Minimum share difficulty** - "Don't accept shares with less than X leading zeros"
2. **Minimum channel hashrate** - "Don't open channels for miners with less than X H/s"

The current code uses minimum_difficulty (a share-level constraint) to derive minimum_hashrate (a channel-level constraint), which is mathematically wrong.

## Recommended Solution

Separate concerns into two independent subsystems:

### 1. Share Validation Layer (NEW)

**Apply minimum_difficulty as a hard constraint on accepted shares.**

**Where:** During share validation, after target check passes

**How:**
```
For each share submitted:
  1. Check: hash < channel_target  [existing check]
  2. NEW: Check: hash has >= minimum_difficulty leading zeros
  3. If both pass → Accept share
  4. If #2 fails → Reject with "share-difficulty-too-low"
```

**Implementation:**

Add to `ehash` protocols:
```rust
// protocols/ehash/src/work.rs
pub fn get_leading_zero_bits(hash: [u8; 32]) -> u32 {
    // Count leading zeros in big-endian representation
    let mut count = 0u32;
    for byte in hash {
        if byte == 0 {
            count += 8;
        } else {
            count += byte.leading_zeros();
            break;
        }
    }
    count
}
```

Add to translat validator:
```rust
// translator/src/lib/utils.rs validate_sv1_share()
pub fn validate_sv1_share(
    share: &client_to_server::Submit,
    target: Target,
    minimum_difficulty: Option<u32>,  // NEW: From config
    // ... other params
) -> Result<bool, TproxyError> {
    // ... existing validation ...

    if hash_as_target < target {  // Existing check
        // NEW: Additional minimum difficulty check
        if let Some(min_bits) = minimum_difficulty {
            let leading_zeros = get_leading_zero_bits(hash);
            if leading_zeros < min_bits {
                return Err(TproxyError::ShareDifficultyTooLow {
                    required: min_bits,
                    actual: leading_zeros,
                });
            }
        }
        return Ok(true);
    }
    Ok(false)
}
```

Similarly for pool:
```rust
// pool/src/lib/mining_pool/message_handler.rs handle_submit_shares_standard()
let res = standard_channel.validate_share(m.clone());

// Add minimum difficulty check
if let Some(min_bits) = self.config.minimum_difficulty() {
    let leading_zeros = get_leading_zero_bits(block_header_hash);
    if leading_zeros < min_bits {
        return error response "share-difficulty-too-low";
    }
}

match res {
    Ok(ShareValidationResult::Valid(...)) => { ... }
}
```

### 2. Channel Hashrate Layer (INDEPENDENT)

**Minimum hashrate becomes a pool-policy knob, separate from ehash.**

**Where:** Channel creation and updates (already implemented correctly!)

**How:** Keep existing clamping logic BUT make it use independent config

**Configuration:**

```toml
# config/shared/pool.toml

[ehash]
# Minimum difficulty (leading zero bits) required to earn 1 ehash unit
# ONLY used for: share validation threshold
minimum_difficulty = 32

[pool]
# NEW: Independent from ehash!
# Minimum hashrate for channel creation (prevents resource starvation)
# Set to 0 to disable (allow any hashrate)
min_downstream_hashrate = 100000000  # 100 MH/s
```

Update code:
```rust
// roles/pool/src/lib/config.rs
pub struct PoolConfig {
    pub min_downstream_hashrate: Option<f32>,  // NEW: replaces deriving from ehash
    // ... rest of config
}

// roles/pool/src/lib/mining_pool/mod.rs
fn handle_open_standard_mining_channel(...) {
    let mut nominal_hash_rate = m.nominal_hash_rate;

    // Clamp only to independent pool minimum (not ehash minimum!)
    if let Some(min) = self.min_downstream_hashrate {
        if nominal_hash_rate < min {
            nominal_hash_rate = min;
        }
    }

    StandardChannel::new_for_pool(
        channel_id,
        user_identity,
        extranonce_prefix,
        requested_max_target.into(),
        nominal_hash_rate,  // No longer constrained by ehash minimum_difficulty
        // ...
    )?;
}
```

### 3. Remove Bad Derivation

**Delete or deprecate:** `derive_min_hashrate_from_difficulty()`

```rust
// DELETE from roles/pool/src/lib/mining_pool/mod.rs line 96
// DELETE from roles/translator/src/lib/config.rs set_min_hashrate_from_difficulty()
```

## Benefits

### ✅ Solves the Hashrate Display Bug
- High-speed miners (1.2 TH/s) no longer get targets constrained by ehash minimum
- Vardiff can set appropriate targets for any hashrate
- Hashrate metrics display correctly

### ✅ Maintains CPU Miner Protection
- Minimum share difficulty is enforced at validation layer
- CPU miners submitting < 32 bit difficulty shares get rejected
- No resource waste from low-difficulty spam

### ✅ Independent Configuration
- `[ehash] minimum_difficulty` - Protocol requirement (32 bits for this pool)
- `[pool] min_downstream_hashrate` - Pool policy (can be 0, 100M, 1G, etc.)
- Can adjust independently without affecting each other

### ✅ Clear Semantics
- Share validation: "Must have >= 32 leading zero bits"
- Channel creation: "Must claim >= 100 MH/s hashrate"
- Both are meaningful and independent

## Implementation Steps

### Phase 1: Add Share Validation Check
1. Implement `get_leading_zero_bits()` in ehash protocols
2. Add minimum_difficulty check to translator's `validate_sv1_share()`
3. Add minimum_difficulty check to pool's `handle_submit_shares_standard/extended()`
4. Test: CPU miner with low-difficulty shares gets rejected

### Phase 2: Add Independent Pool Config
1. Add `min_downstream_hashrate` to pool config struct
2. Update pool TOML config with new setting
3. Change clamping logic to use new setting
4. Keep translator's config as-is (doesn't need this, uses vardiff)

### Phase 3: Remove Bad Derivation
1. Remove `derive_min_hashrate_from_difficulty()` function
2. Remove `set_min_hashrate_from_difficulty()` from translator config
3. Remove minimum_difficulty usage from pool's min_individual_miner_hashrate derivation

### Phase 4: Verify
1. CPU miner with 100 MH/s claiming 1 TH/s → gets channel opened, but low-diff shares rejected
2. BitAxe Gamma (1.2 TH/s) → gets correct targets, hashrate displays as 1.2 TH/s
3. Vardiff still functions correctly for any hashrate

## Configuration Examples

### Example 1: Strict Minimum (Current Intent)
```toml
[ehash]
minimum_difficulty = 32  # 32 leading zeros

[pool]
min_downstream_hashrate = 100000000  # 100 MH/s minimum
```

**Effect:**
- Shares must have >= 32 leading zeros (enforced at validation)
- Channels need >= 100 MH/s claimed hashrate (enforced at channel creation)
- Miners claiming >= 100 MH/s but submitting low-difficulty shares get disconnected

### Example 2: Permissive (Development)
```toml
[ehash]
minimum_difficulty = 16  # Lower threshold for testing

[pool]
min_downstream_hashrate = 1000000  # 1 MH/s (almost no constraint)
```

**Effect:**
- Share validation is permissive
- Channel creation is permissive
- Useful for testing with simulation/CPU miners

### Example 3: No Minimum (Ultra Permissive)
```toml
[ehash]
minimum_difficulty = 0  # No share validation

[pool]
min_downstream_hashrate = null  # No channel constraint
```

**Effect:**
- All shares accepted
- Any hashrate accepted
- Useful for early development

## Code Locations to Modify

### Add Share Validation:
- `protocols/ehash/src/work.rs` - Add `get_leading_zero_bits()`
- `translator/src/lib/utils.rs` - Add check in `validate_sv1_share()`
- `pool/src/lib/mining_pool/message_handler.rs` - Add check after `validate_share()`

### Add Config:
- `roles/pool/src/lib/config.rs` - Add `min_downstream_hashrate` field
- `config/shared/pool.toml` - Add `[pool] min_downstream_hashrate` setting
- `roles/pool/src/args.rs` - Parse new config value

### Remove/Disable:
- `roles/pool/src/lib/mining_pool/mod.rs:96-105` - Delete `derive_min_hashrate_from_difficulty()`
- `roles/pool/src/lib/mining_pool/mod.rs:1187-1194` - Remove minimum_difficulty usage
- `roles/translator/src/lib/config.rs` - Remove `set_min_hashrate_from_difficulty()`
- `roles/translator/src/args.rs` - Stop loading minimum_difficulty from config

## Testing Strategy

### Test 1: CPU Miner with Low Shares
```
Setup: CPU miner claiming 1 MH/s, sending 8-bit difficulty shares
Config: minimum_difficulty = 32
Expected: Shares rejected with "share-difficulty-too-low"
```

### Test 2: Fast Miner with Correct Shares
```
Setup: BitAxe claiming 1.2 TH/s, sending 32+ bit shares
Config: minimum_difficulty = 32, min_downstream_hashrate = 100M
Expected: Shares accepted, hashrate metrics correct
```

### Test 3: Under-Hashrate Claim
```
Setup: Miner claiming 50 MH/s (below 100M minimum)
Config: min_downstream_hashrate = 100M
Expected: Channel clamped to 100M, shares still validated against minimum_difficulty
```

### Test 4: Mixed Scenario
```
Setup: 3 miners: CPU, BitAxe, Antminer
Config: minimum_difficulty = 32, min_downstream_hashrate = 1M
Expected: All get channels, but CPU's low-diff shares rejected
```

## Migration Path

For existing deployments, this is **backward compatible**:
- Default `min_downstream_hashrate = None` means "no constraint" (more permissive than current)
- Existing `[ehash] minimum_difficulty` still read but only used for validation
- No breaking changes to protocol or share handling
- Can roll out incrementally

## Summary

**Old architecture:** Difficulty concept leaks from share validation into channel constraints

**New architecture:** Two clean, independent layers:
1. **Share layer** - Validates shares meet minimum_difficulty threshold (ehash concern)
2. **Channel layer** - Controls channel creation with separate min_downstream_hashrate (pool concern)

Result: CPU miner protection remains, but high-speed miners work correctly and hashrate metrics are accurate.

---

## Implementation Analysis: Issues Encountered (Nov 2025)

**STATUS: Implementation in progress - codebase currently not working**

During implementation of this plan, Phase 3 (removing the bad derivation) exposed fundamental problems with the translator's SV1 protocol implementation that we do not yet understand or have solved.

### Phase 1 & 2 - Implemented
- Share validation checks added to translator and pool
- Pool config for min_downstream_hashrate added
- No issues preventing implementation

### Phase 3 - Removal of Bad Derivation - **Codebase broke, root cause unclear**

Changed translator args.rs to call `set_minimum_difficulty()` instead of `set_min_hashrate_from_difficulty()`:
```rust
config.downstream_difficulty_config.set_minimum_difficulty(minimum_difficulty);
```

This change compiled but caused mining device failures. Multiple cascading errors encountered:

**Error Sequence:**
1. **Initial error**: Mining device received invalid difficulty `0.00000000023282709...` and panicked
   - Traced to using wrong target variable in set_difficulty building
   - Fixed to use `initial_target` from message instead of function parameter

2. **After fix**: Mining device still fails on startup
   - Initial error was extranonce1 panic at client.rs:368
   - Various attempts to fix message timing, caching, and state management
   - Multiple revisions to OpenExtendedMiningChannelSuccess handler with no success

**Observed Problems (unsolved):**
- Translator sends extranonce1 but mining device doesn't receive it properly before notify
- Message ordering between set_difficulty and notify is unclear
- sv1_handshake_complete flag needs to be set at some point but unclear when
- Queued message processing during channel open interacts with message handling in complex ways
- Mining device intermittently connects then immediately disconnects

**Current hypothesis (unconfirmed):**
The translator bridges Stratum V1 (synchronous, ordered protocol with subscribe-response containing extranonce1) with SV2 (asynchronous, unordered messages). The SV2 channel open (OpenExtendedMiningChannelSuccess) arrives asynchronously, potentially after subscribe/authorize messages have already been queued. Reconciling when extranonce1 becomes available with when the SV1 client needs it for the notify message appears to involve intricate state management that we have not successfully implemented.

**What we tried:**
- Manually sending set_difficulty messages from channel open handler
- Caching mining.notify until handshake complete
- Setting sv1_handshake_complete flag at various points
- Processing queued messages synchronously
- Ensuring trait method set_extranonce1() is called instead of direct field assignment
- Multiple variable name corrections and target tracking fixes

**Result:** Codebase compiles but miners still cannot connect/receive proper mining work.

**Open questions:**
- What is the correct sequence of operations when OpenExtendedMiningChannelSuccess arrives?
- When exactly should extranonce1 be available to mining device?
- How should queued subscribe/authorize responses interact with asynchronously-received set_difficulty and notify?
- Is manual message coordination needed, or should the natural message flow handle it?
- Does the SV1 library's state machine require specific method calls in specific order?
