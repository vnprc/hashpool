# Minimum Difficulty Architecture: Problem Analysis and Solution

**Status:** Phase 0 Complete - Phase 1-4 In Progress
**Date:** November 2025
**Context:** Investigation of commit 9f3c27b and subsequent failures

## Current Status

**Phase 0 (Complete ✅):** Reverted broken commit 9f3c27b, restored working baseline with:
- Config: Added `[mint]` section to translator config
- Feature: Translator reads `snapshot_poll_interval_secs` from shared pool config
- Verification: eHash minting is fully operational end-to-end

**Phase 1 (Complete ✅):** Add absolute share difficulty filter at pool level only
- Pool-side validation: Implemented and tested ✅
- Config loading: Added to translator config (but not used)
- Translator-side validation: **Removed** (deferred to JDC + error forwarding design captured)
- Rationale: Pool-level validation sufficient for now; translator-side adds complexity and overhead; JDC will handle this better in future architecture

**Phase 2 (Complete ✅):** Decouple configuration at pool level
- `minimum_difficulty` used ONLY for eHash Quote Dispatcher (amount calculation)
- `minimum_share_difficulty_bits` is independent, used for share validation filtering
- NO mathematical conversions between difficulty and hashrate
- `min_individual_miner_hashrate` completely decoupled from pool logic
- Config clean: pool reads from separate sections with NO coupling

**Phase 3 (Complete ✅):** Add optional pool policy for min_downstream_hashrate
- New `[pool] min_downstream_hashrate` config parameter (optional, in H/s)
- Applied during channel creation to clamp weak devices' nominal hashrate
- Prevents resource exhaustion from low-hashrate miners
- Independent from share validation layer
- Configured in `config/shared/pool.toml`, loaded via shared config system
- Clamping logic applied in both StandardChannel and ExtendedChannel creation

**Phase 4 (In Progress):** Cleanup and documentation updates

## Executive Summary

Commit 9f3c27b attempted to unify downstream difficulty validation across the pool and translator by deriving a minimum hashrate from the shared `minimum_difficulty` config parameter. This approach conflates two fundamentally different concerns:

1. **Share-level validation**: "Do not accept shares with fewer than X leading zero bits"
2. **Channel-level resource management**: "Do not create channels for miners with less than X hashrate"

The mathematical derivation `minimum_hashrate = 2^bits / (60/shares_per_minute)` is correct for translating difficulty to hashrate, but applying this constraint at channel creation breaks the variable difficulty (vardiff) algorithm for high-speed miners.

**The core issue:** When a high-speed miner (e.g., 1.2 TH/s) claims less nominal hashrate in their channel open message (e.g., 100 GH/s), the pool clamps their hashrate upward to `min_individual_miner_hashrate`. This clamped value is then fed into the `hash_rate_to_target()` function, which calculates targets incorrectly, resulting in:
- Wrong target difficulties for the miner
- Incorrect hashrate metrics
- Broken vardiff algorithm

**The solution:** Decouple these two concerns into independent subsystems with separate configuration parameters. Share validation remains at the protocol layer (validating actual proof-of-work hashes), while channel creation constraints become a pool-specific policy knob.

---

## Protocol-Level Minimum Difficulty: Sv1 vs Sv2

Good question. The answer is **yes, but there's a critical architectural difference** between Sv1 and Sv2.

### Sv1: Client-Initiated Minimum Difficulty

**Protocol:** `mining.configure` extension with `minimum-difficulty.value` parameter
- **Direction:** Client → Server (miner requests this)
- **Purpose:** Miner communicates hardware constraints ("I can't handle work easier than X difficulty")
- **Parameter:** `"minimum-difficulty.value"` (numeric: integer or float ≥ 0)
- **Server response:** `"minimum-difficulty"` (boolean: true=accepted, false=rejected)
- **Key point:** This is **client-initiated resource protection**, not server-enforced pooling policy

### Sv2: Application-Level Enforcement

The Sv2 protocol intentionally does NOT include minimum difficulty negotiation:
- **No explicit negotiation message** - Server cannot propose minimum difficulty
- **No channel-opening constraints** - Protocol allows opening any channel with any nominal hashrate
- **Error code only:** `SubmitShares.Error` can return `"difficulty-too-low"` for rejected shares
- **Direction:** Server → Client (pool rejects, miner adapts)
- **Key point:** Sv2 expects pools to enforce minimum in **application logic**, not protocol

### Why This Matters for Hashpool

| Aspect | Sv1 | Sv2 | Hashpool's Design |
|--------|-----|-----|-------------------|
| **Who initiates?** | Client (mining.configure) | Server (share rejection) | Both: translator bridges |
| **Mechanism** | Negotiation (true/false) | One-way enforcement | Share validation layer |
| **Enforcement** | Mutual agreement | Post-submission | Pre-acceptance |
| **Error feedback** | Boolean response | `difficulty-too-low` | Both protocols |

### Hashpool's Unique Position

The translator must handle both paradigms:
- **Downstream (SV1 clients):** May request minimum-difficulty via `mining.configure`
- **Upstream (SV2 pool):** Enforces via `difficulty-too-low` error codes

The proposed architecture fits perfectly:
1. **Share validation layer** enforces minimum bits (matches SV2's philosophy)
2. **Channel policy** prevents weak devices from opening channels (practical pool management)
3. **Vardiff** respects actual hashrate without mathematical coupling

**Conclusion:** Sv2 delegates minimum difficulty to application code by design. Hashpool should do the same, using share validation + `difficulty-too-low` errors, exactly as proposed.

---

## Root Cause Analysis

### The Problem in Detail

**Current flow (commit 9f3c27b and after):**

```
[config] minimum_difficulty = 32 bits
  ↓
[translator] derives min_individual_miner_hashrate = 2^32 / (60/5) ≈ 286 GH/s
[pool] receives same derivation, sets channel constraint
  ↓
[channel open] Miner claims 100 GH/s
  ↓
[clamping] Pool clamps to min_individual_miner_hashrate = 286 GH/s
  ↓
[vardiff] hash_rate_to_target(286 GH/s, 5 spm) → very difficult target
  ↓
[miner] Gets targets suitable for 286 GH/s, not 1.2 TH/s actual hashrate
  ↓
[metrics] Hashrate shows as ~0.6 H/s instead of 1.2 TH/s
```

**Why this is wrong:**

The clamping happens at the **channel level** before vardiff adjustment. Vardiff needs the **actual nominal hashrate** that the miner is claiming, not a floor-clamped value. Consider:

- **Real scenario:** BitAxe Gamma with 1.2 TH/s claims `nominal_hash_rate = 100 GH/s`
- **Current code does:** `nominal_hash_rate = max(100 GH/s, 286 GH/s) = 286 GH/s`
- **Vardiff uses:** `hash_rate_to_target(286 GH/s, 5 spm)` → target for 286 GH/s
- **Miner actually has:** 1.2 TH/s, but submits shares against 286 GH/s target
- **Result:** Miner gets stuck shares or metrics show 0.6 H/s instead of 1.2 TH/s

### Why Commit 9f3c27b Made This Mistake

The commit attempted to enforce `minimum_difficulty` at the channel level by converting it to a hashrate floor. This was motivated by a reasonable goal: ensure all accepted shares are worth at least the minimum ehash unit (32 bits of work). However:

1. **Share validation is different from channel constraints.** A share either meets the channel target or it doesn't; that's already validated. The minimum_difficulty should be a *second* validation layer that checks the actual hash value against the minimum bits threshold.

2. **Channel creation timing is wrong.** The hashrate used at channel creation is just a nominal value for starting vardiff—it's not a lock on the miner's actual hashrate. A miner claiming 100 GH/s might have 1.2 TH/s in reality, and the pool should support both through vardiff adjustment.

3. **The derivation conflates concepts.** `minimum_difficulty` and `min_channel_hashrate` are different:
   - `minimum_difficulty` = protocol-level share validation (all shares must have ≥32 bits)
   - `min_channel_hashrate` = pool-level policy (optional resource limit per channel)

### Why This Breaks SRI Architecture

Hashpool is a fork of the Stratum V2 Reference Implementation (SRI), which uses a clean separation of concerns:

- **Pool** (`roles/pool/`) - Handles mining work distribution, channel management, vardiff
- **Mint** (`roles/mint/`) - Standalone service for ehash token generation
- **Translator** (`roles/translator/`) - SV1↔SV2 proxy with integrated wallet

The SRI's channel architecture (in `protocols/v2/channels-sv2/`) is designed so that:

1. **Nominal hashrate** is advisory—used only to initialize vardiff
2. **Vardiff adjustment** dynamically sets targets based on actual share submission rate
3. **Share validation** is separate from channel constraints

By deriving pool-level channel constraints from ehash-level share difficulty, commit 9f3c27b violated this separation.

---

## The Correct Architecture

### Principle: Two Independent Validation Layers

The solution separates minimum difficulty enforcement into two independent layers, each with its own configuration and validation logic:

#### Layer 1: Share Validation (Protocol Level)

**Purpose:** Ensure every accepted share meets minimum proof-of-work threshold for ehash issuance

**When:** During share validation, after target check passes

**Config:** `[ehash] minimum_difficulty = 32` (in shared config)

**Logic:**
```rust
fn validate_share(share_hash: [u8; 32], channel_target: Target, minimum_difficulty: Option<u32>) -> Result<()> {
    // Existing SRI check
    if hash_as_target >= channel_target {
        return Err("share-below-target");
    }

    // NEW: Minimum difficulty check (if configured)
    if let Some(min_bits) = minimum_difficulty {
        let leading_zeros = count_leading_zero_bits(share_hash);
        if leading_zeros < min_bits {
            return Err("share-difficulty-too-low");
        }
    }

    Ok(())
}
```

**Properties:**
- Applies to all submitted shares uniformly
- Independent of channel creation or vardiff
- Can be enabled/disabled per pool without affecting SRI channel logic
- Rejects low-difficulty shares early (resource protection)

#### Layer 2: Channel Creation Policy (Pool Level)

**Purpose:** Optional pool resource management—prevent excessive channels from weak devices

**When:** During `OpenStandardMiningChannel` / `OpenExtendedMiningChannel`

**Config:** `[pool] min_downstream_hashrate = 100000000` (in pool config, optional)

**Logic:**
```rust
fn handle_open_channel(nominal_hash_rate: f32, min_downstream_hashrate: Option<f32>) -> f32 {
    // If pool has a minimum, enforce it
    if let Some(min) = min_downstream_hashrate {
        if nominal_hash_rate < min {
            nominal_hash_rate = min;
        }
    }
    // Otherwise, accept any hashrate (including 0 for testing)
    nominal_hash_rate
}
```

**Properties:**
- Optional (defaults to None/"no constraint")
- Independent of ehash minimum_difficulty
- Used only for channel creation, not vardiff
- Allows operators to balance resource usage vs. accessibility

### Why This Works

**For high-speed miners:**
- ClimaxX Gamma claims 100 GH/s, actual 1.2 TH/s
- Channel opens with `nominal_hash_rate = 100 GH/s`
- Vardiff gets correct 100 GH/s value → calculates appropriate targets
- Miner submits shares, vardiff adjusts upward to match actual 1.2 TH/s
- Hashrate metrics are correct

**For CPU miners:**
- CPU miner tries to submit 8-bit difficulty share
- Share validation layer rejects it: "share-difficulty-too-low"
- No ehash token issued
- Miner disconnects or adjusts difficulty

**For weak devices:**
- Weak device claims 1 MH/s
- If `min_downstream_hashrate = 100 MH/s`, channel clamps to 100 MH/s
- Vardiff starts with 100 MH/s targets
- Device quickly adjusts down once shares are rejected (or on pool update message)
- No resource starvation

---

## Why This Design Avoids the Last Failure Cascade

The November 2025 implementation attempt (Phase 3 of the old design) broke the translator. Here's the analysis:

### What Went Wrong Last Time

**The change:**
```rust
// roles/translator/src/args.rs
// OLD: config.downstream_difficulty_config.set_min_hashrate_from_difficulty(minimum_difficulty);
// NEW: config.downstream_difficulty_config.set_minimum_difficulty(minimum_difficulty);
```

**What this broke:**
1. `min_individual_miner_hashrate` field was no longer being set
2. It defaulted to 0.0 (or became uninitialized)
3. The SV1 server used this for vardiff calculations
4. SV1 sent invalid difficulty to mining device
5. Mining device panicked, miners couldn't connect

**The cascade:**
```
Remove derivation
  ↓
min_individual_miner_hashrate = 0.0 or uninitialized
  ↓
SV1 server: "I need to set vardiff, let me use min_individual_miner_hashrate"
  ↓
Vardiff gets 0.0, calculates wrong targets
  ↓
Mining device: receives impossible difficulty
  ↓
extranonce1 panic at client.rs:368
  ↓
Multiple attempts to fix message ordering failed
  ↓
Root cause: Removed a value the SV1 state machine depended on
```

### How This Design Avoids It

**Principle 1: Don't Remove, Only Add**

The new design is **purely additive** in Phase 1:
- Add share validation check
- Keep all existing SV1 code unchanged
- SV1 state machine continues to work exactly as before

**Principle 2: Break Mathematical Coupling, Not Dependencies**

Instead of:
```
❌ minimum_difficulty → (math) → min_individual_miner_hashrate → vardiff
```

Do:
```
✅ minimum_difficulty → (validation only)
   min_individual_miner_hashrate → (kept from elsewhere) → vardiff
```

The `min_individual_miner_hashrate` field is **decoupled from minimum_difficulty mathematically**, but the FIELD STILL EXISTS AND IS SET.

**Principle 3: Clear Separation of Concerns**

| Component | Responsibility | Dependency Chain |
|-----------|---|---|
| **SV1 Server** | Initialize vardiff, send targets to mining device | `min_individual_miner_hashrate` (from config) → vardiff → targets |
| **Share Validation** | Check if share meets minimum bits | `minimum_difficulty_bits` (from config) → validation → accept/reject |
| **Channel Creation** | (Pool only) Optionally constrain weak devices | `min_downstream_hashrate` (from pool config) → channel policy |

**Each component has its own input, no sharing.**

**Concrete comparison:**

```rust
// ❌ WHAT FAILED (last attempt):
// Tried to remove set_min_hashrate_from_difficulty() entirely
// SV1 server was left without a value for min_individual_miner_hashrate
// → vardiff broke

// ✅ WHAT THIS DESIGN DOES:
// Phase 1: Add share validation (SV1 unchanged)
// Phase 2: Stop deriving min_individual_miner_hashrate from minimum_difficulty
//          BUT ensure it still has a value (default or separate config)
// Phase 3: Add pool-level min_downstream_hashrate (pool code only, translator untouched)
// Result: Three independent systems, no dependencies removed
```

### Why This is Safer

1. **Isolated failures:** If Phase 1 breaks, it's share validation logic (new code)
2. **Verified safeguard:** SV1 server still gets what it needs before we change anything
3. **No cascades:** Each phase is independent; later phases don't require earlier ones to work
4. **Rollback friendly:** Can disable any phase without affecting others

### Testing Strategy to Catch Regressions

**Before Phase 2, verify Phase 1 works:**
1. CPU miner with 8-bit shares → rejected (good)
2. Normal miner with 32-bit shares → accepted (good)
3. **SV1 device connects and mines** ← This is the critical test
4. Hashrate metrics are correct ← Verify vardiff still works

**If the hashrate metrics are wrong in step 4, DO NOT proceed to Phase 2.** It means the SV1 state machine is still broken, and we need to debug further before decoupling.

---

## Comparison with Baseline SRI

The baseline SRI (at `/home/evan/work/stratum/`) handles this differently:

1. **SV1 protocol**: Includes optional `minimum-difficulty` in the server's `mining.configure` message (boolean, not bits)
   - This is a SV1-specific feature for negotiating share difficulty
   - Not present in SV2 (which uses targets instead)

2. **SV2 pool behavior**:
   - No minimum difficulty enforcement
   - Vardiff is purely based on nominal hashrate and actual share rate
   - Pools can reject low-difficulty shares in application logic

3. **Hashpool variant**:
   - Uses ehash to tokenize shares
   - Needs minimum difficulty to ensure economic viability (32 bits ≈ $0.001 value in typical scenarios)
   - Should not let this requirement leak into channel creation

**The fix aligns Hashpool with SRI principles while adding ehash-specific validation.**

---

## Implementation Path

### Phase 0: Revert the Broken Commit (COMPLETE ✓)

**Status:** COMPLETE - System is stable and eHash minting is operational

**Commit reverted:** `9f3c27b382d75c7a9123e97ceb6b2ed16a46ee8a` ("align downstream difficulty with mint minimum")

**Outcome:** Restored working baseline with functioning eHash minting pipeline

**Steps:**

1. **Handle merge conflicts manually** (git revert will conflict because later commits depend on it)
   ```bash
   # Start the revert
   git revert 9f3c27b382d75c7a9123e97ceb6b2ed16a46ee8a

   # If conflicts occur, git will mark them. Resolve as follows:
   # - Keep the REVERTED state (remove the minimum_difficulty derivation logic)
   # - Remove calls to set_min_hashrate_from_difficulty()
   # - Keep min_individual_miner_hashrate field (just don't set it from minimum_difficulty)
   ```

2. **For conflicting files:**
   - `roles/translator/src/args.rs` - Remove the minimum_difficulty derivation, keep min_individual_miner_hashrate at default
   - `roles/pool/src/lib/mining_pool/mod.rs` - Remove the derive_min_hashrate_from_difficulty() function and calls
   - `config/shared/pool.toml` - Revert to pre-9f3c27b state
   - Let git handle other files automatically

3. **Verify the revert compiles:**
   ```bash
   cd roles && cargo build
   ```

4. **Test that miners connect:**
   ```bash
   devenv shell
   devenv up
   # SV1 devices should connect, receive targets, and submit shares
   # Hashrate metrics should be correct
   ```

5. **Commit the revert:**
   ```bash
   git commit -m "Revert commit 9f3c27b: align downstream difficulty with mint minimum

   This reverts the mathematical coupling of minimum_difficulty to min_individual_miner_hashrate
   which broke the translator by leaving vardiff without proper initialization values.

   Restores working state. New architecture will be implemented cleanly in subsequent phases.

   Reverts: 9f3c27b382d75c7a9123e97ceb6b2ed16a46ee8a"
   ```

**Expected outcome:**
- System compiles
- SV1 miners connect and mine
- Hashrate metrics display correctly
- No vardiff errors or extranonce1 panics

**If conflicts during revert are too complex:**
- Manual revert is acceptable: git will identify conflicted files, you manually edit them to remove the problematic code
- The goal is simple: remove the `set_min_hashrate_from_difficulty()` calls and the `derive_min_hashrate_from_difficulty()` function
- Ensure `min_individual_miner_hashrate` field still exists but is set to a default value (e.g., 0.0)

---

### Phase 1: Add Absolute Share Difficulty Filter (Non-Breaking, Isolated)

**Objective:** Reject shares with insufficient leading zero bits to prevent low-difficulty spam

**Configuration files:**
- Pool-side: Add to `config/shared/pool.toml`
- Miner-side: Add to `config/shared/miner.toml`

```toml
[validation]
# Absolute share difficulty filter: minimum leading zero bits required
# 0 = no check, 32 = typical production value
minimum_share_difficulty_bits = 32
```

**Files to modify:**

**Pool side (Completed):**
1. **config/shared/pool.toml** - ✅ Added `[validation]` section with `minimum_share_difficulty_bits`
2. **roles/pool/src/lib/config.rs** - ✅ Read setting and expose as getter
3. **roles/pool/src/lib/mining_pool/message_handler.rs** - ✅ Added check in share validation

**Translator side (Removed):**
- **config/shared/miner.toml** - ~~Add `[validation]` section~~ (Completely removed)
- **roles/translator/src/lib/config.rs** - ~~Read setting and expose as getter~~ (Removed)
- **roles/translator/src/args.rs** - ~~Load setting from shared config~~ (Removed)
- **roles/translator/src/lib/sv1/sv1_server/sv1_server.rs** - ~~Add check in share validation~~ (Removed)

**Implementation status:**

**Pool-level validation (Completed & Verified):**
- ✅ Modified `config/shared/pool.toml` with `[validation]` section
- ✅ Added config field to PoolConfig and reads from TOML
- ✅ Implemented leading zero bits check in pool's message_handler.rs during share validation
- ✅ Low-difficulty shares rejected via SV2
- ✅ High-difficulty shares accepted
- ✅ Verified: SV1 device connects and mines normally (no vardiff errors)
- ✅ Unit tests pass (count_leading_zero_bits and validate_share_difficulty)

**Translator-level validation (Completely removed):**
- **Decision:** Remove all translator-side validation code and config in favor of pool-only approach
- **What was removed:**
  - `minimum_share_difficulty_bits` field from TranslatorConfig
  - `set_minimum_share_difficulty_bits()` setter method
  - Code in args.rs that loads config from global file
  - TODO comment in sv1_server.rs
  - `[validation]` section from config/shared/miner.toml
- **Rationale:**
  - Pool-level validation is sufficient for current requirements
  - Translator-side validation requires block header reconstruction (SHA256 per share)
  - Performance overhead not justified until JDC integration
  - Better architecture: Pool validates; if JDC added later, JDC handles validation natively
  - Error forwarding mechanism documented in `TRANSLATOR_REJECTED_SHARE_ERROR_FORWARDING.md` for future reference
- **Impact:** Translator is now back to pre-Phase1 state; forwards all shares to pool unchanged

**Helper function:** Count leading zero bits in share hash (add to utility module)
```rust
fn count_leading_zero_bits(hash: &[u8; 32]) -> u32 {
    // Returns number of leading zero bits in the hash
}
```

**Share rejection:** When bits < minimum:
- Pool: Return error code `"share-difficulty-too-low"`
- Translator: Same error propagation to SV1 miner

**Critical safeguard:** Pool-level validation is **purely additive** at the protocol layer. The existing SV1 protocol state machine remains untouched. No translator code was modified.

**Translator-side validation:** Removed after analysis determined:
1. Pool-level validation is sufficient
2. Translator-side adds SHA256 overhead per share
3. Pool already returns error codes via SV2 protocol
4. Better to defer to JDC architecture (which constructs templates natively)
5. Error forwarding design captured for future reference

**Impact:** None on channel creation or vardiff—pool adds validation without changing resource management logic. Translator behavior unchanged (forwards all shares to pool).

**Why this avoids the last failure:** The SV1 server's `min_individual_miner_hashrate` field and its vardiff logic are completely unaffected. No translator code was removed or modified.

**Metrics:**
- Track "shares rejected for low difficulty" separately for operator debugging (pool-side)

**Testing checklist (completed):**
- [✅] Low-difficulty share (8 bits) gets rejected by pool
- [✅] Minimum-difficulty share (32 bits) accepted by pool
- [✅] High-difficulty share (64 bits) accepted by pool
- [✅] **Verified:** SV1 device connects normally and gets correct targets
- [✅] Hashrate metrics are still correct (no vardiff errors)
- [✅] No translator changes needed; forwarding works as before

### Phase 2: Decouple Configuration (Pool-Only)

**Objective:** Remove the mathematical coupling between `minimum_difficulty` and `min_individual_miner_hashrate` at the pool level. Keep translator unchanged (already removed from translator in Phase 1 cleanup).

**Files to modify (Pool only):**
- `config/shared/pool.toml` - Keep `[ehash] minimum_difficulty` and `[validation] minimum_share_difficulty_bits`
- `roles/pool/src/lib/config.rs` - Ensure `minimum_difficulty_bits` is read directly (already done)
- `roles/pool/src/lib/mining_pool/` - Verify share validation uses only `minimum_difficulty_bits`, not derived from `minimum_difficulty`

**Translator status:**
- ✅ Translator does NOT read or apply `minimum_difficulty_bits` (removed in Phase 1)
- ✅ Translator forwards all shares to pool; pool validates and rejects if needed
- ✅ No translator changes needed for Phase 2

**Changes (Pool):**
1. Verify `minimum_difficulty_bits` is read directly from config (not derived from `minimum_difficulty`)
2. Pass only to pool's share validation layer
3. Do NOT change `min_individual_miner_hashrate` usage in pool (still exists, just not derived)

**Why this is different from last time:**
- Last attempt: Removed `set_min_hashrate_from_difficulty()` entirely → `min_individual_miner_hashrate` became 0.0 → SV1 broke
- This attempt: Only remove the DERIVATION at pool level, keep the FIELD and its usage; translator already removed in Phase 1

**Example change (what NOT to do):**
```rust
// ❌ DON'T do this (what failed last time):
// Completely remove this line
// config.downstream_difficulty_config.set_min_hashrate_from_difficulty(minimum_difficulty);
// Now min_individual_miner_hashrate is uninitialized → vardiff breaks
```

**Example change (what TO do):**
```rust
// ✅ DO this instead:
// Read minimum_difficulty_bits but DON'T derive min_individual_miner_hashrate
config.downstream_difficulty_config.minimum_difficulty_bits = minimum_difficulty;

// min_individual_miner_hashrate is still set elsewhere:
// Either initialized to default value in config.rs, or read from separate config
// The SV1 server can still use it for vardiff
```

**Impact:** Config remains the same for users; internal logic cleanly separates concerns

### Phase 3: Create Independent Pool Policy

**Files to modify:**
- `config/shared/pool.toml` - Add new `[pool]` section with `min_downstream_hashrate`
- `roles/pool/src/lib/config.rs` - Add field
- `roles/pool/src/lib/mining_pool/mod.rs` - Use new field, remove derivation
- `roles/pool/src/lib/mining_pool/message_handler.rs` - Use new field in channel open

**Changes:**
1. New config option (optional, defaults to `None`)
2. Update channel clamping logic to use new field
3. Remove the `derive_min_hashrate_from_difficulty()` function
4. Update documentation

**Impact:** Channel creation behavior changes only if `min_downstream_hashrate` is set

### Phase 4: Cleanup

**Files to modify:**
- `roles/translator/src/args.rs` - Remove derivation logic
- `roles/translator/src/lib/config.rs` - Remove `set_min_hashrate_from_difficulty()` method
- Documentation updates

**Impact:** Simplifies translator code

---

## Configuration Examples

### Example 1: Typical Pool (Production)

```toml
[validation]
minimum_share_difficulty_bits = 32  # Share validation: 32 leading zero bits minimum

[ehash]
minimum_difficulty = 32  # eHash calculation: 32 bits = 1 unit

[pool]
port = 34254
min_downstream_hashrate = 100000000  # Channel policy: 100 MH/s minimum to prevent resource waste
```

**Effect:**
- **Share validation:** Shares with <32 bits rejected early, error "share-difficulty-too-low"
- **eHash calculation:** Each share with ≥32 bits earns proportional ehash amount
- **Channel creation:** Weak devices claiming <100 MH/s get clamped to 100 MH/s
- **Vardiff:** Uses actual claimed hashrate (not derived), so high-speed miners get correct targets
- **Metrics:** Hashrate display is accurate for all device types

### Example 2: Development/Testing

```toml
[validation]
minimum_share_difficulty_bits = 16  # Lower threshold for testing

[ehash]
minimum_difficulty = 16  # Easier ehash calculation for CPU miners

[pool]
port = 34254
min_downstream_hashrate = 1000000  # 1 MH/s (almost no constraint)
```

**Effect:**
- Permissive share validation (16 bits minimum)
- Lower eHash unit values (for faster testing)
- Permissive channel creation (1 MH/s minimum)
- Useful for testing with CPU miners and low-power devices

### Example 3: No Minimum (Ultra Permissive)

```toml
[ehash]
minimum_difficulty = 0  # No share validation

[pool]
# No min_downstream_hashrate field → defaults to None
```

**Effect:**
- All shares accepted regardless of difficulty
- Channels created for any device
- Full backward compatibility with old code

---

## Why This Preserves SRI Architecture

The proposed solution respects the core SRI design principles:

1. **Minimal SRI changes** - Only adds a validation check in share processing (similar to share.rs changes)
2. **Channel logic untouched** - `StandardChannel` and `ExtendedChannel` remain unchanged
3. **Vardiff preserved** - Nominal hashrate and vardiff work as designed
4. **Clean separation** - Protocol-level validation (share bits) separate from pool policy (channel creation)
5. **Rebase-friendly** - Changes are localized; future SRI updates won't conflict

The `min_downstream_hashrate` is a pool-specific feature, not an SRI feature, so it belongs in hashpool-specific code (pool config, pool.rs), not in protocol modules.

---

## Alternative Approaches Considered

### Alternative 1: Share Validation Only, No Channel Constraint

**Pros:**
- Simplest implementation
- Completely preserves SRI

**Cons:**
- CPU miners can still open channels, just get rejected on first share
- Resource exhaustion from connection spam
- Poor UX (miners connect then disconnect)

**Decision:** Not sufficient for production pool operation

### Alternative 2: Make Minimum Difficulty Dynamic per Channel

**Idea:** Vary the minimum difficulty based on the miner's claimed hashrate

**Pros:**
- Could allow weaker devices to earn lower-difficulty shares

**Cons:**
- Complicates the protocol significantly
- Creates economic perverse incentives (lower hashrate = lower payment requirement)
- Harder to understand/operate

**Decision:** Over-engineered; better to set a single pool-wide minimum and reject low-difficulty shares

### Alternative 3: Use UpdateChannel to Enforce Minimum After Opening

**Idea:** Open channel permissively, then send UpdateChannel to clamp hashrate

**Pros:**
- Doesn't reject connections

**Cons:**
- Doesn't save resources (channel is already open)
- Complex message sequencing

**Decision:** Enforcement at channel opening is cleaner

---

## Migration and Backward Compatibility

### For Existing Deployments

The fix is **backward compatible** when `min_downstream_hashrate` is not set:

- Default `min_downstream_hashrate = None` means "no constraint" (more permissive than current)
- Existing configs continue to work
- `[ehash] minimum_difficulty` still read and used for share validation
- No protocol changes

### Upgrade Path

1. **Phase 1 & 2** (add share validation, decouple config)
   - Deploy with `minimum_difficulty` used only for shares (not channel creation)
   - Miners continue to work exactly as before
   - No configuration changes needed

2. **Phase 3** (add new pool config)
   - Operators can optionally set `min_downstream_hashrate` if desired
   - If not set, behaves like Phase 1 (no constraint)
   - Solves the resource exhaustion concern for operators who want it

3. **Inform users** that minimum_difficulty no longer affects channel creation
   - This is actually a feature (fixes the hashrate display bug)
   - Document the new `min_downstream_hashrate` setting

---

## Implementation Decisions

Based on operator feedback:

1. **Minimum difficulty:** Configurable per pool instance (default: 32 bits for development). Pools can specialize for different target markets.
2. **Min downstream hashrate:** Configurable per pool instance (no hardcoded default). Testing with 1.2 TH/s BitAxe. Progressive fee structures with miner-selectable hashrate are a future goal.
3. **Share rejection behavior:** Simple rejection (miner can renegotiate by adjusting settings and reconnecting).
4. **Validation location:** Both translator and pool (defense in depth).
5. **Metrics tracking:** Track "shares rejected for low difficulty" separately for operator debugging.

---

## Critical Invariants (Safeguards Against Regression)

To ensure this design doesn't fail like the last attempt, maintain these invariants:

### Invariant 1: SV1 State Machine Independence
```
The SV1 server's vardiff logic MUST always have access to a valid
min_individual_miner_hashrate value, regardless of what minimum_difficulty is set to.

If this breaks, DO NOT proceed. Debug until SV1 devices connect and mine correctly.
```

**How to verify:**
- SV1 device connects → receives subscribe response with extranonce1
- Device sends authorize → pool responds
- Device receives mining.notify with target and job
- Device submits shares → pool accepts and metrics show correct hashrate

### Invariant 2: No Shared State Between Layers
```
Share validation and channel creation logic MUST NOT share mutable state.

Each should read its own config value independently:
- Share validation: reads minimum_difficulty_bits
- Channel creation: reads min_downstream_hashrate
- Vardiff: reads its own value (from default or separate config)
```

**How to verify:**
- Change `[ehash] minimum_difficulty` in config
- Verify share validation changes but channel creation doesn't
- Change `[pool] min_downstream_hashrate`
- Verify channel creation changes but share validation doesn't

### Invariant 3: Phases Are Independently Deployable
```
Phase 1 must work perfectly before attempting Phase 2.
Phase 2 must work perfectly before attempting Phase 3.

If any phase breaks, the previous phases must still work when reverted.
```

**How to verify:**
- After Phase 1: Revert Phase 2 & 3, system still works
- After Phase 2: Revert Phase 3, system still works
- After Phase 3: System works (end state)

### Invariant 4: No Silent Failures
```
If a component loses a dependency (like last time), it MUST fail loudly.

For example: If min_individual_miner_hashrate becomes 0.0 unintentionally,
the vardiff code should either:
- Panic (obvious failure)
- Log an error (obvious failure)
- Use a sensible default (intentional design decision)

NOT silently produce wrong values (like difficulty 0.00000000023...)
```

**Implementation suggestion:**
```rust
// In translator config initialization:
pub fn validate_config(&self) -> Result<()> {
    if self.downstream_difficulty_config.min_individual_miner_hashrate == 0.0 {
        // This is OK if explicitly configured
        if !self.uses_default_min_hashrate {
            error!("min_individual_miner_hashrate is 0.0 but not from default!");
            return Err(ConfigError::MissingMinHashrate);
        }
    }
    Ok(())
}
```

---

## Summary of Changes

**ALL PHASES COMPLETE (Phase 0-3 ✅)**

The following changes have been implemented:

| Concern | Before (Post-9f3c27b) | After (Phases 1-3) |
|---------|---|---|
| **Minimum difficulty** | Used for both share validation AND channel creation | `minimum_difficulty`: eHash amount only; `minimum_share_difficulty_bits`: share validation only |
| **Channel minimum** | Derived mathematically from `minimum_difficulty`, breaks vardiff | NEW: `min_downstream_hashrate` (optional), independent from share validation |
| **Vardiff input** | Receives clamped nominal_hash_rate (breaks calculation) | Receives actual claimed nominal_hash_rate (works correctly) |
| **Hashrate metrics** | Shows wrong values for high-speed miners | Shows correct values |
| **CPU miner spam** | Prevented at channel level (but breaks vardiff) | Prevented at share validation level (clean separation) |
| **Config complexity** | Single confusing parameter | Three independent parameters, clear intent |

**Key Achievement:** Mathematical coupling completely removed. `minimum_difficulty` no longer affects channel creation or vardiff calculations.

---

## Recommendation

**Complete implementation path with revert-first strategy:**

1. **Phase 0: Revert commit 9f3c27b** (CRITICAL - restores working baseline)
2. **Phase 1-4: Implement new architecture** (clean, separated concerns)

**Rationale:**
1. **Revert first** - Restores working system, unblocks development
2. **Correct by design** - New architecture aligns with SRI principles
3. **Solves the bug** - High-speed miners get correct vardiff targets
4. **Preserves pool policy** - Operators can still prevent resource exhaustion
5. **Simple to understand** - Clear separation of concerns
6. **Backward compatible** - Existing configs continue to work
7. **Rebase-friendly** - Future SRI updates won't conflict

---

## Implementation Status and Next Steps

### PHASES 0-3 COMPLETE ✅

All core architectural phases have been implemented and tested:

1. **Phase 0 ✅** - Reverted broken commit, restored working baseline
2. **Phase 1 ✅** - Pool-level share validation filter (`minimum_share_difficulty_bits`)
3. **Phase 2 ✅** - Config decoupling (separate eHash and validation parameters)
4. **Phase 3 ✅** - Optional pool policy (`min_downstream_hashrate`)

### Testing Completed for Phases 1-3:
- ✅ CPU miner with 8-bit shares gets rejected (validation layer)
- ✅ Normal miner with 32+ bit shares accepted
- ✅ High-speed miners work correctly with proper hashrate metrics
- ✅ SV1 device connects normally without vardiff errors
- ✅ Pool builds without errors
- ✅ Config loads from shared config system

### Final Configuration in Use:

**Pool-side (`config/shared/pool.toml`):**
```toml
[validation]
minimum_share_difficulty_bits = 32

[ehash]
minimum_difficulty = 32

[pool]
port = 34254
# min_downstream_hashrate = 100000000  # Optional, uncomment to enable
```

**Status:** Ready for production deployment with or without `min_downstream_hashrate` policy.

