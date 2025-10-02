# Rebase Strategy Guide

## Overview
This guide documents the rebase strategy for each SRI file we've modified. The goal is to make rebasing onto latest SRI as smooth as possible by understanding what we changed and why.

## Classification

### Type A: New Files (No Conflicts Expected)
These are entirely new Hashpool additions. Copy them forward as-is during rebase.

- `protocols/v2/subprotocols/mint-quote/` — entire new protocol crate
- `protocols/v2/subprotocols/mining/src/mint_quote_notification.rs` — new message type
- `protocols/ehash/` — Cashu logic isolation crate (not in SRI at all)

**Rebase Strategy**: `git add` these files after rebasing. No conflicts expected.

---

### Type B: Protocol Extensions (Merge Carefully)
These files add new fields/variants to existing SRI types. Conflicts likely if SRI modified the same structs.

#### `protocols/v2/subprotocols/mining/src/submit_shares.rs`
**Our changes:**
- Added `hash: PubKey<'decoder>` field to `SubmitSharesExtended`
- Added `locking_pubkey: CompressedPubKey<'decoder>` field to `SubmitSharesExtended`
- Added these fields to `GetSize` impl

**Conflict risk**: Medium. If SRI added other fields to `SubmitSharesExtended`, manual merge needed.

**Rebase strategy**:
1. Check if SRI modified `SubmitSharesExtended` struct
2. If yes: merge our two fields alongside their changes
3. Update `GetSize` calculation to include our fields + any new SRI fields

---

#### `protocols/v2/subprotocols/mining/src/open_channel.rs`
**Our changes:**
- Added `OpenMiningChannelError::NoMintKeyset` variant

**Conflict risk**: Low. Enum variants rarely conflict.

**Rebase strategy**: Add our variant to whatever the upstream enum looks like.

---

#### `protocols/v2/const-sv2/src/lib.rs`
**Our changes:**
- Added `SV2_MINT_QUOTE_PROTOCOL_DISCRIMINANT = 0x05`
- Added `MESSAGE_TYPE_MINT_QUOTE_*` constants
- Added `CHANNEL_BIT_MINT_QUOTE_*` constants

**Conflict risk**: Low. Constants are additive.

**Rebase strategy**: Copy our constants forward. Check for namespace collisions.

---

#### `protocols/v2/binary-sv2/no-serde-sv2/codec/src/` (multiple files)
**Our changes:**
- Added `CompressedPubKey` type support across codec stack (decodable, encodable, impls, datatypes)
- 6 files modified to add serialization/deserialization for compressed secp256k1 public keys

**Conflict risk**: Medium-High. Codec changes are invasive.

**Rebase strategy**:
1. Check if upstream added `CompressedPubKey` or similar
2. If not, carefully merge our additions
3. If yes, adapt to use upstream's implementation
4. This is the riskiest set of changes; consider proposing upstream

---

#### `protocols/v2/binary-sv2/serde-sv2/` (Cargo.toml, error.rs, lib.rs)
**Our changes:**
- Added secp256k1 dependency
- Added error variant for compressed key issues
- Minor imports

**Conflict risk**: Low. Supporting changes for CompressedPubKey.

**Rebase strategy**: Apply alongside no-serde-sv2 CompressedPubKey changes

---

#### `protocols/v2/roles-logic-sv2/src/errors.rs`
**Our changes:**
- Added `Error::MissingPremintSecret` variant
- Added `Error::KeysetError(String)` variant

**Conflict risk**: Low.

**Rebase strategy**: Add our variants to the upstream enum.

---

### Type C: Behavioral Changes (Review Carefully)
These files modify existing SRI behavior. Conflicts likely if SRI refactored the same code.

#### `protocols/v2/roles-logic-sv2/src/channel_logic/channel_factory.rs`
**Our changes:**
- Modified `OnNewShare` to initialize `hash` and `locking_pubkey` fields in `SubmitSharesExtended` construction
- Added hash/locking key capture logic (~30 lines)
- Fixed endianness bug in target comparison (may already be fixed upstream)

**Conflict risk**: High. This is core share validation logic.

**Rebase strategy**:
1. Check if SRI fixed the endianness bug (our fix at line 858-870)
2. If yes, drop our fix and use theirs
3. For hash/locking_pubkey initialization: these only exist if Type B changes are present
4. If upstream refactored `on_submit_shares_extended`, carefully re-apply field initialization

**Key insight**: Our changes are tightly coupled to the `SubmitSharesExtended` field additions. If those fields don't exist upstream, these changes are meaningless.

---

#### `protocols/v2/roles-logic-sv2/src/parsers.rs`
**Our changes:**
- Added `Minting` enum (~90 lines)
- Added `MintQuoteNotification` to `Mining` enum
- Added routing logic for `Minting` and `MintQuoteNotification` messages

**Conflict risk**: Medium. Parser changes are localized but invasive.

**Rebase strategy**:
1. Check if upstream refactored parser structure
2. Re-apply our `Minting` enum as a new addition
3. Add `MintQuoteNotification` variant to whatever `Mining` enum looks like
4. Re-apply message routing logic in the new parser structure

---

#### `protocols/v2/roles-logic-sv2/src/handlers/mining.rs`
**Our changes:**
- Changed `error!` to `debug!` for "difficulty-too-low" share rejections
- Changed `info!` to `debug!` for some mining job messages
- Minor logging improvements

**Conflict risk**: Low. Logging changes are cosmetic.

**Rebase strategy**:
1. Check if upstream made similar logging improvements
2. If yes, drop ours
3. If no, consider this a "nice to have" — not essential for rebase

**Recommendation**: Drop these changes. They're UX improvements, not functional requirements.

---

#### `protocols/v2/roles-logic-sv2/src/job_creator.rs`
**Our changes:**
- Test constant: `Network::Testnet` → `Network::Regtest`

**Conflict risk**: None.

**Rebase strategy**: Drop this change. It's test config for local development.

---

#### `protocols/v2/roles-logic-sv2/src/utils.rs`
**Our changes:**
- Clippy fix: removed unnecessary `as_mut()` call

**Conflict risk**: None.

**Rebase strategy**: Drop this change. If upstream has it, great. If not, irrelevant to rebase.

---

#### `protocols/v2/roles-logic-sv2/src/handlers/common.rs`
**Our changes:**
- Comment reformatting (line length changes)
- No functional changes

**Conflict risk**: None.

**Rebase strategy**: Drop these changes entirely. They're just formatting.

---

#### `protocols/v2/subprotocols/common-messages/src/setup_connection.rs`
**Our changes:**
- Added `SetupConnectionMint` struct wrapping `SetupConnection` + `keyset_id` field
- Delegates most methods to inner `SetupConnection`

**Conflict risk**: Low. New struct addition.

**Rebase strategy**:
1. Copy `SetupConnectionMint` struct forward
2. Check if upstream changed `SetupConnection` interface
3. Update delegation methods if needed

**Note**: This is currently unused. Consider removing if mint connection setup doesn't need it.

---

### Type D: Dependency Updates (Low Risk)
Cargo.toml changes to add dependencies.

#### `protocols/v2/roles-logic-sv2/Cargo.toml`
**Our changes:**
- Added `mint_quote_sv2` dependency

**Conflict risk**: Low. Dependency additions are additive.

**Rebase strategy**: Add our dependency to upstream's dependency list.

---

#### `protocols/v2/subprotocols/mining/Cargo.toml`
**Our changes:**
- Added `mint-quote-sv2` and Cashu deps

**Conflict risk**: Low.

**Rebase strategy**: Add our dependencies.

---

## Target: SRI v1.2.1

**Why v1.2.1:**
- Only 3 weeks of changes from our fork point (e8d76d68, Dec 19 2024)
- Includes critical timestamp bug fix
- Minimal conflict risk compared to v1.5.0 (which is 9 months ahead with 761 commits)
- Stable tagged release

**Two-Phase Strategy:**
1. **Phase 1**: Rebase onto v1.2.1 (this guide)
2. **Phase 2**: Later incremental rebase v1.2.1 → v1.5.0 (future work)

**Worktree Setup:**
- `/home/evan/work/stratum` — upstream SRI repository
- `/home/evan/work/sri-baseline` — worktree at v1.2.1 for reference during rebase
- `/home/evan/work/hashpool` — our fork

**Good News for v1.2.1 Rebase:**
SRI made **zero changes** between e8d76d68 and v1.2.1 to our critical files:
- `channel_factory.rs`
- `parsers.rs`
- `submit_shares.rs`

This means the v1.2.1 rebase should have minimal to zero conflicts in the high-risk areas!

---

## Rebase Execution Plan

### Step 1: Preparation
```bash
# Update baseline worktree to v1.2.1
cd /home/evan/work/sri-baseline
git fetch
git checkout v1.2.1

# In hashpool repo
cd /home/evan/work/hashpool
git checkout master
git pull

# Verify our fork point
git log --oneline e8d76d68 -1
# Should show: "Merge pull request #1310 from Sjors/2024/12/bump-tp" (Dec 19 2024)

# Create rebase branch
git checkout -b rebase-sri-v1.2.1
```

### Step 2: Rebase Attempt
```bash
# Rebase onto v1.2.1
git rebase v1.2.1

# Or if you prefer to fetch the tag from stratum worktree first:
git fetch /home/evan/work/stratum v1.2.1:refs/tags/v1.2.1
git rebase v1.2.1
```

### Step 3: Conflict Resolution (Per File Type)

**For Type A (New Files):**
- Conflicts impossible. If git complains, something went wrong.

**For Type B (Protocol Extensions):**
- Open conflict in editor
- Merge our fields/variants alongside upstream's
- Verify struct layout makes sense
- Update GetSize/serialization impls

**For Type C (Behavioral Changes):**
- **Priority 1**: `channel_factory.rs` — check endianness fix, re-apply field initialization
- **Priority 2**: `parsers.rs` — re-apply `Minting` enum and routing
- **Priority 3**: Drop logging changes in `handlers/mining.rs` unless trivial to merge

**For Type D (Dependencies):**
- Accept both upstream and our dependencies
- Resolve version conflicts by choosing newer version

### Step 4: Validation
```bash
# Build both workspaces
cd protocols && cargo build
cd ../roles && cargo build

# Run smoke test
# (follow devenv smoke test procedure from PROMPT_LOOP.md)
```

### Step 5: Update Documentation
- Update `REBASE_NOTES.md` with new SRI base commit
- Note any behavior differences from rebase
- Update `AGENTS.md` if SRI made relevant changes

---

## High-Risk Areas

### 1. CompressedPubKey codec changes
**Why risky**: Touches serialization layer, subtle bugs possible.

**Mitigation**:
- Propose to upstream first
- Extensive testing after rebase
- Consider runtime hex dump comparison

### 2. channel_factory.rs share validation
**Why risky**: Core mining logic, bugs here break share accounting.

**Mitigation**:
- Careful line-by-line review
- Regression test with known valid/invalid shares
- Check if upstream fixed endianness independently

### 3. Parser routing changes
**Why risky**: Message dispatch errors cause silent failures.

**Mitigation**:
- Trace message flow after rebase
- Test mint quote request/response round-trip
- Verify MintQuoteNotification reaches translator

---

## Upstream Contribution Opportunities

These changes might be accepted upstream with proper framing:

1. **CompressedPubKey support** — generic 33-byte compressed key type (Type B, codec)
2. **Endianness fix** — if not already fixed (Type C, channel_factory.rs)
3. **Logging improvements** — difficulty-too-low as debug not error (Type C, handlers/mining.rs)

Consider opening issues/PRs for these before rebasing to reduce diff surface.

---

## Success Criteria

Post-rebase, verify:
- [ ] Both workspaces build without errors
- [ ] Smoke test passes (share → mint quote → notification flow)
- [ ] No new warnings introduced
- [ ] Diff surface documented in updated REBASE_NOTES.md
- [ ] Phase 3 automation (if exists) still runs

---

## Notes

- Keep `protocols/ehash/` and `roles-utils/mint-pool-messaging/` unchanged during rebase
- These are pure Hashpool additions with no SRI counterpart
- If rebase touches them, something went wrong
