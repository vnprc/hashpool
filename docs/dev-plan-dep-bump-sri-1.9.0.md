# Dev Plan: Dependency Bump to SRI 1.9.0 + CDK Update

**Created:** 2026-05-14
**Scope:** Bump all SRI protocol crates to v1.9.0 release level, un-vendor channels_sv2,
update stratum-core/stratum-apps, and update CDK fork.

---

## Preamble

You are a 1337 protocol engineer and Rust systems hacker. You live and breathe binary
wire protocols, SV2 message framing, Noise handshakes, and cashu ecash primitives.
You know that a botched dependency bump in a mining pool means real hashrate hitting
the floor and real miners losing money. You treat every `cargo check` like defusing a
bomb, every `cargo test` like a penetration test, and every human full-stack test like
a mainnet deployment. Precision. Discipline. No YOLO merges.

You are working on **hashpool**, a Stratum V2 mining pool with integrated cashu ecash
payouts. The codebase lives at `/home/vnprc/work/hashpool`. It has two cargo workspaces:
- `roles/Cargo.toml` - the main application workspace (pool, translator, mint, etc.)
- `protocols/Cargo.toml` - the protocol library workspace (ehash, parsers, handlers, etc.)

---

## Current State

### SRI Protocol Crates (crates.io versions)

| Crate | Hashpool Current | crates.io Latest | Delta |
|-------|-----------------|------------------|-------|
| binary_sv2 | 5.0.1 | 5.0.1 | current |
| codec_sv2 | 4.0.1 | **5.0.0** | MAJOR |
| framing_sv2 | 6.0.1 | 6.0.1 | current |
| noise_sv2 | 1.4.1 | **1.4.2** | patch |
| common_messages_sv2 | 6.0.2 | **7.1.0** | MAJOR |
| mining_sv2 | 7.0.0 | **9.0.0** | 2x MAJOR |
| template_distribution_sv2 | 4.0.2 | **5.0.0** | MAJOR |
| job_declaration_sv2 | 6.0.0 | **7.0.0** | MAJOR |
| buffer_sv2 | 3.0.0 | **3.0.1** | patch |
| const_sv2 | 4.0.1 | 4.0.1 | current |
| derive_codec_sv2 | 1.1.2 | 1.1.2 | current |
| stratum-core | 0.2.1 | **0.3.0** | minor |
| stratum-apps | 0.3.0 (vendored) | **0.4.0** | minor |

### Vendored Protocol Crates (local path deps)

| Crate | Location | Vendored Version | crates.io Latest | Why Vendored |
|-------|----------|-----------------|------------------|--------------|
| channels_sv2 | protocols/v2/channels-sv2 | 3.0.0 | **5.0.0** | Clamp fix (now upstreamed via PR #2118) |
| parsers_sv2 | protocols/v2/parsers-sv2 | 0.1.1 | 0.3.0+ | Extended with mint_quote message types (0xC0, 0xC1) |
| handlers_sv2 | protocols/v2/handlers-sv2 | 0.1.0 | 0.3.0 | Path dep of roles_logic_sv2 |
| roles_logic_sv2 | protocols/v2/roles-logic-sv2 | 4.0.0 | 5.0.0 | Uses vendored parsers/handlers |

### CDK (Cashu Development Kit)

| Item | Status |
|------|--------|
| cdk version | 0.16.0 (latest release, no 0.17.x exists yet) |
| Patched via | `[patch.crates-io]` pointing to `vnprc/cdk` fork @ rev `04c584e3` |
| Fork branch | `send-p2pk-signing-keys` |
| Fork changes | ~10 commits adding P2PK signing key auto-detection and proof filtering |
| Upstream PR | cashubtc/cdk#1835 - **STILL OPEN, NOT MERGED** |
| Upstream main | 87+ commits since v0.16.0 tag, no new release |

### Custom Hashpool Crates (NOT vendored from upstream)

- `mint_quote_sv2` (protocols/v2/subprotocols/mint-quote) - hashpool-only SV2 extension
- `stats_sv2` (protocols/v2/subprotocols/stats-sv2) - hashpool-only SV2 extension
- `ehash` (protocols/ehash) - hashpool ecash+hash utilities
- `stratum-apps` (common/stratum-apps) - vendored from sv2-apps with 2 hashpool-specific changes

### Key PR Status

| PR | Repo | Status | Impact |
|----|------|--------|--------|
| #2118 "channels_sv2: clamp target" | stratum-mining/stratum | **MERGED** Apr 22 | Can un-vendor channels_sv2 |
| #1835 "P2PK signing keys" | cashubtc/cdk | **OPEN** | Cannot un-vendor cdk fork yet |

### Clarification: "Messages Crate"

The user asked about un-vendoring "the messages crate." This refers to **channels_sv2**
(documented in `docs/SHIT_TO_FIX.md` under "Unvendor channels-sv2 After Upstreaming the
Clamp Fix"). The upstream PR #2118 that contained the clamp fix was merged on Apr 22, 2026.
The fix is included in the crates.io channels_sv2 5.0.0 release (SRI v1.9.0, May 5, 2026).
**channels_sv2 can now be un-vendored.**

Note: `roles_logic_sv2` was historically called `messages_sv2` (renamed in 2022), but
"messages crate" in this context means channels_sv2 based on the SHIT_TO_FIX.md entry.

The other vendored crates (parsers_sv2, handlers_sv2, roles_logic_sv2) CANNOT be
un-vendored yet because parsers_sv2 contains hashpool-specific mint_quote message type
injections (MintQuoteNotification 0xC0, MintQuoteFailure 0xC1) in its Mining enum.

---

## Setup Instructions

Before starting any milestone work:

### 1. Clone Reference Repos

```bash
cd /home/vnprc/work

# SRI protocol reference (for API diff investigation)
git clone https://github.com/stratum-mining/stratum.git stratum-upstream

# sv2-apps reference (for stratum-apps comparison)
git clone https://github.com/stratum-mining/sv2-apps.git sv2-apps-upstream

# CDK upstream reference
git clone https://github.com/cashubtc/cdk.git cdk-upstream
```

### 2. Create Worktrees for Each Milestone

Each milestone gets its own git worktree branched from master. This keeps the
working master clean for the human to test against, and isolates each change set.

```bash
cd /home/vnprc/work/hashpool

# Milestone 1: SRI crate bumps
git worktree add ../hashpool-m1-sri-bump -b dep-bump/m1-sri-crates master

# Milestone 2: Un-vendor channels_sv2
# (create AFTER M1 is merged to master)
git worktree add ../hashpool-m2-unvendor-channels -b dep-bump/m2-unvendor-channels master

# Milestone 3: stratum-core + stratum-apps
# (create AFTER M2 is merged to master)
git worktree add ../hashpool-m3-stratum-core -b dep-bump/m3-stratum-core master

# Milestone 4: CDK fork update
# (create AFTER M3 is merged to master)
git worktree add ../hashpool-m4-cdk-update -b dep-bump/m4-cdk-update master

# Milestone 5: parsers_sv2 un-vendor investigation (stretch)
# (create AFTER M4 is merged to master)
git worktree add ../hashpool-m5-unvendor-parsers -b dep-bump/m5-unvendor-parsers master
```

**IMPORTANT:** Create each worktree ONLY after the previous milestone has been merged
to master by the human. Each milestone builds on the previous one.

---

## Milestone 1: Bump SRI Protocol Crates to v1.9.0

**Branch:** `dep-bump/m1-sri-crates`
**Worktree:** `/home/vnprc/work/hashpool-m1-sri-bump`
**Risk:** HIGH - multiple major version bumps with likely API breakage
**Estimated scope:** mining_sv2 7→9 is the scariest; common_messages_sv2 6→7.1 is next

### Strategy

All SRI crates are co-released. They must be bumped atomically. The approach:

1. **Research first.** Before changing ANY code, diff the upstream SRI source between the
   tag versions hashpool currently uses and v1.9.0. Identify every public API change in:
   - mining_sv2 (7.0.0 → 9.0.0) - 2 major versions, expect significant changes
   - common_messages_sv2 (6.0.2 → 7.1.0) - major bump
   - template_distribution_sv2 (4.0.2 → 5.0.0) - major bump
   - job_declaration_sv2 (6.0.0 → 7.0.0) - major bump
   - codec_sv2 (4.0.1 → 5.0.0) - major bump
   - channels_sv2 (3.0.0 → 5.0.0) - 2 major versions (vendored copy needs update too)

   Use the cloned `stratum-upstream` repo:
   ```bash
   cd /home/vnprc/work/stratum-upstream
   # Find tags for old and new versions
   git log --oneline --all --decorate | grep -i "mining_sv2\|v1.7\|v1.8\|v1.9"
   # Diff the relevant crate source between versions
   ```

2. **Bump versions in Cargo.toml files.** Update every Cargo.toml in both workspaces.
   Files that need version bumps (search for each old version string):
   - `roles/pool/Cargo.toml`
   - `roles/mint/Cargo.toml`
   - `roles/translator/Cargo.toml` (if it exists and uses SRI crates)
   - `roles/jd-server/Cargo.toml`
   - `roles/jd-client/Cargo.toml`
   - `roles/roles-utils/*/Cargo.toml` (several)
   - `protocols/v2/parsers-sv2/Cargo.toml`
   - `protocols/v2/handlers-sv2/Cargo.toml`
   - `protocols/v2/roles-logic-sv2/Cargo.toml`
   - `protocols/v2/channels-sv2/Cargo.toml` (vendored, update deps)
   - `protocols/ehash/Cargo.toml`
   - `protocols/v2/subprotocols/mint-quote/Cargo.toml`
   - `common/stratum-apps/Cargo.toml` (if it uses SRI crates)
   - `test/integration-tests/Cargo.toml`

   Use grep to find ALL occurrences:
   ```bash
   cd /home/vnprc/work/hashpool-m1-sri-bump
   grep -rn 'mining_sv2.*7\.0\.0' --include='Cargo.toml'
   grep -rn 'common_messages_sv2.*6\.0' --include='Cargo.toml'
   # ... etc for each crate
   ```

3. **Fix compilation errors.** Run `cargo check` in both workspaces and fix API breakage:
   ```bash
   cd /home/vnprc/work/hashpool-m1-sri-bump/protocols && cargo check 2>&1
   cd /home/vnprc/work/hashpool-m1-sri-bump/roles && cargo check 2>&1
   ```
   Use the upstream SRI source as reference for new API signatures.

4. **Update the vendored channels_sv2 source.** The vendored copy at
   `protocols/v2/channels-sv2/` is at v3.0.0. It needs to be synced to v5.0.0 so
   the [patch.crates-io] override is consistent. Copy the upstream channels_sv2 source
   from `stratum-upstream` at the v1.9.0 tag, then re-apply the clamp fix if it's not
   already included (it should be - PR #2118 was merged before v1.9.0).

   Actually, since Milestone 2 will un-vendor channels_sv2 entirely, just update the
   vendored Cargo.toml deps to match the new versions and ensure the code compiles.
   Don't bother making the vendored source pristine - it's about to be deleted.

5. **Run tests:**
   ```bash
   cd /home/vnprc/work/hashpool-m1-sri-bump/protocols && cargo test 2>&1
   cd /home/vnprc/work/hashpool-m1-sri-bump/roles && cargo test 2>&1
   ```

### Completion Criteria
- `cargo check` passes in both workspaces
- `cargo test` passes (or failures are pre-existing / unrelated)
- Commit to `dep-bump/m1-sri-crates` branch

### Human Testing
**ASK THE HUMAN TO:**
1. Merge `dep-bump/m1-sri-crates` into master
2. Run full stack: bitcoind, pool, translator, mint, web services
3. Connect a test miner and verify:
   - Shares are accepted
   - Vardiff adjustments work
   - Mint quotes are created and paid
   - Web dashboards show stats
4. If anything fails, report back with logs. DO NOT proceed to Milestone 2.

---

## Milestone 2: Un-vendor channels_sv2

**Branch:** `dep-bump/m2-unvendor-channels`
**Worktree:** `/home/vnprc/work/hashpool-m2-unvendor-channels`
**Risk:** LOW - the upstream crate now contains our fix
**Prereq:** Milestone 1 merged to master

### Steps

1. Delete the vendored directory:
   ```
   rm -rf protocols/v2/channels-sv2/
   ```

2. Remove `channels_sv2` from `[patch.crates-io]` in BOTH workspace Cargo.toml files:
   - `roles/Cargo.toml` line 29: `channels_sv2 = { path = "../protocols/v2/channels-sv2" }`
   - `protocols/Cargo.toml` (check if it has a channels_sv2 patch - it may not)

3. Ensure all dependents use the correct crates.io version. After M1, roles_logic_sv2
   should already reference channels_sv2 at the right version. Verify:
   ```bash
   grep -rn 'channels_sv2' --include='Cargo.toml'
   ```

4. Remove `protocols/v2/channels-sv2` from `protocols/Cargo.toml` workspace members list
   (if present - check line 8).

5. Run `cargo check` and `cargo test` in both workspaces.

6. Update `docs/SHIT_TO_FIX.md` - mark the "Unvendor channels-sv2" item as DONE.

### Completion Criteria
- No vendored channels_sv2 directory
- No channels_sv2 in [patch.crates-io]
- `cargo check` and `cargo test` pass
- Commit to branch

### Human Testing
**ASK THE HUMAN TO:**
1. Merge `dep-bump/m2-unvendor-channels` into master
2. Run full stack (same test as M1)
3. Pay special attention to: channel opening, vardiff, target clamping
4. If anything fails, report back. DO NOT proceed to Milestone 3.

---

## Milestone 3: Bump stratum-core and stratum-apps

**Branch:** `dep-bump/m3-stratum-core`
**Worktree:** `/home/vnprc/work/hashpool-m3-stratum-core`
**Risk:** MEDIUM - stratum-apps is vendored with 2 local modifications
**Prereq:** Milestone 2 merged to master

### Context

`stratum-apps` at `common/stratum-apps/` is vendored from sv2-apps. Per
`docs/dev-plan-stratum-apps-cleanup.md`, the vendored copy differs from upstream
in exactly two files:
- `src/monitoring/sv1.rs` - 4 extra fields on `Sv1ClientInfo`
- `src/key_utils/mod.rs` - re-export of local key-utils crate

stratum-apps on crates.io is now at 0.4.0 (was 0.3.0-based vendored copy).
stratum-core on crates.io is now at 0.3.0 (was 0.2.1).

### Steps

1. **Bump stratum-core** from 0.2.1 to 0.3.0 in `common/stratum-apps/Cargo.toml`.

2. **Investigate stratum-apps 0.4.0.** Compare the crates.io 0.4.0 source with the
   vendored copy. Check if the 2 local modifications can be upstreamed or if we
   still need to vendor:
   ```bash
   cd /home/vnprc/work/sv2-apps-upstream
   # Find the stratum-apps crate source
   # Diff against /home/vnprc/work/hashpool/common/stratum-apps/
   ```

3. **If un-vendoring stratum-apps is feasible:**
   - Replace the vendored directory with a crates.io dependency
   - Port the 2 modifications (Sv1ClientInfo fields, key_utils re-export)

4. **If un-vendoring is NOT feasible:**
   - Sync the vendored copy to 0.4.0 source
   - Re-apply the 2 local modifications
   - Bump stratum-core dep to 0.3.0

5. Fix compilation errors from stratum-core 0.3.0 API changes.

6. Run `cargo check` and `cargo test`.

### Completion Criteria
- stratum-core at 0.3.0
- stratum-apps updated (vendored or un-vendored)
- Both workspaces compile and pass tests

### Human Testing
**ASK THE HUMAN TO:**
1. Merge `dep-bump/m3-stratum-core` into master
2. Run full stack
3. Pay special attention to: monitoring endpoints, web dashboards, miner stats display
4. Verify `/metrics` and REST API responses are correct
5. If anything fails, report back. DO NOT proceed to Milestone 4.

---

## Milestone 4: Update CDK Fork

**Branch:** `dep-bump/m4-cdk-update`
**Worktree:** `/home/vnprc/work/hashpool-m4-cdk-update`
**Risk:** MEDIUM-HIGH - 87+ upstream commits to integrate, P2PK changes are unreleased
**Prereq:** Milestone 3 merged to master

### Context

Hashpool uses cdk 0.16.0 from crates.io, patched via `[patch.crates-io]` to
`vnprc/cdk` fork at rev `04c584e3`. The fork adds P2PK signing key enhancements
(auto-detection, proof filtering). The upstream PR (cashubtc/cdk#1835) is still OPEN.

Since there's no new cdk release (0.16.0 is still latest), the version number stays
the same. But upstream main has 87+ commits with improvements we may want:
- Async connection pooling (SQLite, PostgreSQL)
- Enhanced melt operations
- Various bug fixes

### Steps

1. **Check PR #1835 status** before starting. If it was merged since this doc was
   written, the approach changes significantly (can potentially remove the fork):
   ```bash
   gh pr view 1835 --repo cashubtc/cdk --json state
   ```

2. **If PR #1835 is STILL OPEN (likely):**
   - Go to the vnprc/cdk fork repo
   - Rebase the `send-p2pk-signing-keys` branch onto latest upstream main
   - Resolve any conflicts (the P2PK changes touch wallet/send logic)
   - Push the rebased branch
   - Update the rev in hashpool's `[patch.crates-io]` sections:
     - `roles/Cargo.toml` (lines 30-38)
     - `protocols/Cargo.toml` (lines 18-24)
   - Run `cargo update` to refresh Cargo.lock

3. **If PR #1835 WAS MERGED:**
   - Check if a new cdk release includes it
   - If yes: remove all cdk entries from `[patch.crates-io]`, bump cdk version
   - If no release yet: point fork rev to upstream main (or a specific commit post-merge)

4. Fix any compilation errors from upstream cdk API changes.

5. Run `cargo check` and `cargo test`.

### Completion Criteria
- CDK fork is rebased on latest upstream
- [patch.crates-io] rev is updated
- Both workspaces compile

### Human Testing
**ASK THE HUMAN TO:**
1. Merge `dep-bump/m4-cdk-update` into master
2. Run full stack
3. **Critical tests for this milestone:**
   - Mint starts and connects to pool
   - Mint quotes are created when shares are submitted
   - Wallet can receive ecash tokens
   - P2PK locked proofs work correctly (if testing P2PK flow)
   - Check mint logs for any new warnings/errors
4. If anything fails, report back. DO NOT proceed to Milestone 5.

---

## Milestone 5 (Stretch): Investigate Un-vendoring parsers_sv2 Chain

**Branch:** `dep-bump/m5-unvendor-parsers`
**Worktree:** `/home/vnprc/work/hashpool-m5-unvendor-parsers`
**Risk:** HIGH - architectural change, may not be feasible
**Prereq:** Milestone 4 merged to master

### Context

parsers_sv2 is vendored because it has two hashpool-specific message types injected
into the `Mining` enum:
- `MintQuoteNotification` (message type 0xC0)
- `MintQuoteFailure` (message type 0xC1)

These are defined in `mint_quote_sv2` (hashpool-only crate) and wired into
`protocols/v2/parsers-sv2/src/lib.rs` lines 281-286.

Because parsers_sv2 is vendored, handlers_sv2 and roles_logic_sv2 must also be
vendored (they depend on parsers_sv2 via path).

SRI v1.6.0 introduced `extensions_sv2` (crate on crates.io at 0.1.0), which provides
extension message negotiation. **Investigate whether this mechanism can replace the
hardcoded mint_quote injections.**

### Steps

1. **Study extensions_sv2 API:**
   ```bash
   cd /home/vnprc/work/stratum-upstream
   # Find extensions_sv2 source
   find . -name "Cargo.toml" -exec grep -l "extensions_sv2" {} \;
   # Read the source and understand the extension mechanism
   ```

2. **Study how upstream parsers_sv2 0.3.0 handles extensions.**
   Does it have hooks for custom message types? Can extensions register
   new variants without modifying the Mining enum?

3. **If extensions_sv2 can handle mint_quote messages:**
   - Migrate MintQuoteNotification/MintQuoteFailure to use the extensions mechanism
   - Remove the injected variants from vendored parsers_sv2
   - Switch to crates.io parsers_sv2
   - Switch to crates.io handlers_sv2
   - Switch to crates.io roles_logic_sv2
   - Delete vendored directories
   - Update workspace Cargo.toml files

4. **If extensions_sv2 CANNOT handle this:**
   - Document why (in SHIT_TO_FIX.md)
   - Consider upstreaming mint_quote support to SRI parsers_sv2 (long-term)
   - Keep vendored crates for now

5. Run `cargo check` and `cargo test`.

### Completion Criteria
- Either: all protocol crates un-vendored and using crates.io
- Or: documented why it's not feasible with a plan for eventual un-vendoring

### Human Testing (if changes were made)
**ASK THE HUMAN TO:**
1. Merge `dep-bump/m5-unvendor-parsers` into master
2. Run full stack
3. Critical: verify mint quote notifications flow correctly from pool to translator
4. Verify the translator processes MintQuoteNotification and MintQuoteFailure messages

---

## Reference: File Locations

### Workspace Cargo.toml files (version bumps + patch sections)
- `roles/Cargo.toml` - main workspace, [patch.crates-io] for channels_sv2 and cdk
- `protocols/Cargo.toml` - protocol workspace, [patch.crates-io] for cdk

### Key role Cargo.toml files (SRI crate consumers)
- `roles/pool/Cargo.toml` - heaviest SRI consumer
- `roles/mint/Cargo.toml` - cdk + SRI consumer
- `roles/translator/Cargo.toml`
- `roles/jd-server/Cargo.toml`
- `roles/jd-client/Cargo.toml`
- `roles/web-pool/Cargo.toml`
- `roles/web-proxy/Cargo.toml`

### Vendored protocol crates (internal deps to update)
- `protocols/v2/channels-sv2/Cargo.toml`
- `protocols/v2/parsers-sv2/Cargo.toml`
- `protocols/v2/handlers-sv2/Cargo.toml`
- `protocols/v2/roles-logic-sv2/Cargo.toml`

### Hashpool-specific protocol crates (check for SRI dep versions)
- `protocols/ehash/Cargo.toml` - uses binary_sv2, derive_codec_sv2, cdk-common
- `protocols/v2/subprotocols/mint-quote/Cargo.toml` - uses binary_sv2, derive_codec_sv2
- `protocols/v2/subprotocols/stats-sv2/Cargo.toml` - uses binary_sv2, derive_codec_sv2

### Utility crates
- `roles/roles-utils/mint-pool-messaging/Cargo.toml` - uses binary_sv2
- `roles/roles-utils/quote-dispatcher/Cargo.toml` - uses binary_sv2, roles_logic_sv2
- `roles/roles-utils/network-helpers/Cargo.toml`
- `common/stratum-apps/Cargo.toml` - uses stratum-core

### Existing documentation to update after completion
- `docs/SHIT_TO_FIX.md` - mark channels_sv2 un-vendor item as DONE
- `docs/dev-plan-stratum-apps-cleanup.md` - update if stratum-apps changes

---

## Reference: Upstream Repos

| Repo | URL | Purpose |
|------|-----|---------|
| SRI protocol | https://github.com/stratum-mining/stratum | API reference for crate bumps |
| sv2-apps | https://github.com/stratum-mining/sv2-apps | stratum-apps comparison |
| CDK upstream | https://github.com/cashubtc/cdk | cdk API reference |
| CDK fork | https://github.com/vnprc/cdk | hashpool's patched cdk |
| cdk-ehash | https://github.com/vnprc/cdk-ehash | hashpool's cdk-ehash adapter |

---

## Workflow Rules

1. **One milestone at a time.** Do not start the next milestone until the human has
   tested and merged the current one.

2. **Branch per milestone.** Each milestone gets its own branch from master. Create the
   worktree only after the previous milestone is merged.

3. **Compile early, compile often.** After every batch of Cargo.toml changes, run
   `cargo check`. Don't accumulate version bumps without checking compilation.

4. **Research before coding.** For Milestone 1 especially, spend time reading upstream
   API changes before touching hashpool code. Understanding the diff is half the battle.

5. **Never force-push master.** Milestone branches get merged to master only by the human
   after successful testing.

6. **Preserve mint_quote extensions.** The vendored parsers_sv2 has hashpool-specific
   message types. Do not lose these during the bump. They are critical protocol messages.

7. **Test both workspaces.** hashpool has TWO cargo workspaces (roles/ and protocols/).
   Both must compile and pass tests independently.

8. **cdk-ehash is a separate repo.** If cdk changes break cdk-ehash
   (https://github.com/vnprc/cdk-ehash), that needs fixing too. Check compatibility.
