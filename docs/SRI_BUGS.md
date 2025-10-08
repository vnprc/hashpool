# SRI Baseline Bugs Found During Hashpool Development

This document tracks bugs found in the upstream [SRI (Stratum Reference Implementation)](https://github.com/stratum-mining/stratum) codebase that also affect our hashpool implementation.

## Critical: Authorization Bug in Translator

**File:** `roles/translator/src/lib/downstream_sv1/downstream.rs`

**Description:** The `handle_authorize()` method returns `true` (success) but never calls `self.authorize()` to add the worker name to the `authorized_names` vector. This causes the `is_a` check (which verifies `!d.authorized_names.is_empty()`) to always be false, preventing downstreams from entering the select loop to receive jobs.

**Impact:**
- Miners receive authorization success but never get jobs
- Difficulty adjustment never runs (requires job reception)
- Hashrate stays at configured default and never adjusts to actual miner performance

**Root Cause:**
```rust
// In handle_authorize() - returns true but doesn't modify authorized_names
fn handle_authorize(&self, request: &client_to_server::Authorize) -> bool {
    // ... validation logic ...
    true  // Returns success but authorized_names stays empty!
}
```

**Fix:** After `handle_message()` succeeds for an authorize request, explicitly call `self.authorize()`:
```rust
// Check if this is an authorize message and extract the worker name
let worker_name = if let json_rpc::Message::StandardRequest(ref req) = message_sv1 {
    if let Ok(auth) = client_to_server::Authorize::try_from(req.clone()) {
        Some(auth.name.clone())
    } else {
        None
    }
} else {
    None
};

let response = self_.safe_lock(|s| s.handle_message(message_sv1)).unwrap();

// If it was an authorize message and it succeeded, add to authorized names
if let Some(name) = worker_name {
    if response.is_ok() {
        self_.safe_lock(|s| s.authorize(&name)).ok();
    }
}
```

**Verified:** This same bug exists in the upstream SRI repository as of our last check.

---

## Target Comparison Endianness Bug in SV2 CPU Miner

**File:** `roles/test-utils/mining-device-sv2/src/main.rs`

**Description:** The CPU miner's target comparison logic was not properly handling endianness when comparing share hashes against the target threshold. Targets need to be stored in little-endian format to match the pool's validation logic.

**Impact:**
- Share validation could fail incorrectly
- Miners might submit shares that don't meet the target
- Inconsistent behavior between miner and pool validation

**Root Cause:**
```rust
// In new_target() - target wasn't being stored in correct endian format
pub fn new_target(&mut self, mut new_target: Target) {
    // Missing proper byte order handling
    self.target = new_target;
}
```

**Fix (commit 975163a4):**
```rust
pub fn new_target(&mut self, mut new_target: Target) {
    // Store targets in little-endian format
    new_target.0.reverse();
    self.target = new_target;
}

// Updated next_share() for proper byte order handling
fn next_share(&mut self) -> Result<Share, ()> {
    // ... nonce iteration ...
    if hash.as_ref() < self.target.0.as_ref() {
        // Proper little-endian comparison
        return Ok(share);
    }
}
```

**Verified:** This endianness issue exists in upstream SRI CPU miner implementation.

---

## Share Hash Conditional Compilation Bug

**File:** `roles/translator/src/lib/upstream_sv2/upstream.rs`

**Description:** The share hash was being set on `SubmitSharesExtended` messages inside a debug logging conditional block. This caused the hash field to only be populated when debug logging was enabled, breaking share submission in production builds or with different log levels.

**Impact:**
- Shares submitted without hash when debug logging disabled
- Pool rejects shares due to missing/invalid hash
- Bug only appears in production or when log level changes
- Silent failure that's hard to diagnose

**Root Cause:**
```rust
// Hash assignment was INSIDE the logging conditional
if tracing::level_enabled!(tracing::Level::DEBUG) {
    let mut hash = hash;
    hash.reverse();
    match &mut m {
        Share::Extended(extended_share) => {
            extended_share.hash = hash.into();  // Only set if debug enabled!
        }
        Share::Standard(_) => (),
    };
    debug!("Share hash: {:?}", hash);
}
```

**Fix (commit b91c4bff):**
```rust
// Move hash assignment OUTSIDE the conditional
let mut hash = hash_.as_hash().into_inner();
hash.reverse();
match &mut m {
    Share::Extended(extended_share) => {
        extended_share.hash = hash.into();  // Always set now
    }
    Share::Standard(_) => (),
};

// Keep only the logging inside the conditional
if tracing::level_enabled!(tracing::Level::DEBUG) {
    debug!("Share hash: {:?}", hash);
}
```

**Verified:** This same pattern exists in upstream SRI translator - hash assignment is incorrectly scoped to logging conditionals.

---

## Protocol Library Bug: work_selection and version_rolling Flag Helpers Swapped

**Status:** FIXED in hashpool fork, needs upstream PR to SRI
**Priority:** CRITICAL - Breaks all work selection detection
**Fixed:** 2025-10-07

**Bug Location:** `protocols/v2/subprotocols/common-messages/src/setup_connection.rs:206-219`

**The Problem:**
The helper functions `has_work_selection()` and `has_version_rolling()` have their bit shift values swapped:

```rust
// WRONG (original):
pub fn has_version_rolling(flags: u32) -> bool {
    let flags = flags.reverse_bits();
    let flags = flags << 1;  // Checks bit 1 (work_selection) ❌
    let flag = flags >> 31;
    flag != 0
}

pub fn has_work_selection(flags: u32) -> bool {
    let flags = flags.reverse_bits();
    let flags = flags << 2;  // Checks bit 2 (version_rolling) ❌
    let flag = flags >> 31;
    flag != 0
}

// CORRECT (fixed):
pub fn has_version_rolling(flags: u32) -> bool {
    let flags = flags.reverse_bits();
    let flags = flags << 2;  // Bit 2 for version_rolling ✓
    let flag = flags >> 31;
    flag != 0
}

pub fn has_work_selection(flags: u32) -> bool {
    let flags = flags.reverse_bits();
    let flags = flags << 1;  // Bit 1 for work_selection ✓
    let flag = flags >> 31;
    flag != 0
}
```

**Impact:**
- JD Client (flags=100, bit 2 set) was detected as having work_selection=true ❌
- Translator (flags=110, bits 1+2 set) was detected as having work_selection=true ✓ (accidentally correct!)
- All downstream connections with version_rolling enabled were misidentified as template providers
- Impossible to correctly distinguish JDC from regular proxies

**Per SV2 Spec:**
- Bit 0: REQUIRES_STANDARD_JOBS
- Bit 1: REQUIRES_WORK_SELECTION
- Bit 2: REQUIRES_VERSION_ROLLING

**Discovery:**
Found while debugging dashboard Template Provider column - both translator (flags=110) and JDC (flags=100) were showing work_selection=true even though only translator should have bit 1 set.

**Action Items:**
1. ✅ Fixed in hashpool fork
2. ⬜ Submit PR to stratum-mining/stratum upstream
3. ⬜ Check if other SRI roles are affected by this bug
4. ⬜ Add unit tests for flag helper functions

---

## Notes

- We should submit PRs to SRI for all these bug fixes once we've confirmed they're stable in our implementation
- Check for similar authorization patterns in other SRI roles that might have the same issue
- The conditional compilation bug pattern may exist in other locations where logging and message modification are mixed
