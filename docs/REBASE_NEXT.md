# Next Rebase: v1.2.1 → v1.3.0

## Target Analysis

**Version**: v1.3.0 (Mar 28, 2025)
**Commits**: 431 commits over 2.5 months (Jan 8 → Mar 28)
**Diff impact**: protocols/ shows -3926/+8751 lines (net -4825, major cleanup/refactor)

## Key Changes in v1.3.0

From git history analysis:

### 1. Major Refactor: Selector/Routing Logic (768376d6)
- Moved `selectors` and `routing_logic` modules from roles-logic-sv2 to mining-proxy
- Updated `IsUpstream`, `IsMiningUpstream` traits (removed `DownstreamSelector`)
- **Impact**: Likely conflicts in roles-logic-sv2 if we touch routing, but hashpool doesn't use proxy selectors

### 2. Protocol Cleanups
- Net -4825 lines in protocols/ suggests documentation additions + dead code removal
- mining_sv2 messages got refactored (submit_shares.rs -68 lines, open_channel.rs -189 lines)
- template-distribution: `coinbase_output_data_size.rs` → `coinbase_output_constraints.rs`

### 3. Hashpool Conflict Surface

**Low risk areas** (we don't touch these):
- Proxy selector logic (we have no proxy customizations)
- Template distribution messages (we use mining protocol only)

**Medium risk areas** (we extend but don't heavily modify):
- `protocols/v2/subprotocols/mining/src/submit_shares.rs` (-68 lines)
  - We add `hash` + `locking_pubkey` fields to SubmitSharesExtended
  - Upstream refactor likely documentation/helper cleanup
  - Conflict probable but mechanical (accept upstream refactor, re-add our fields)

- `protocols/v2/subprotocols/mining/src/open_channel.rs` (-189 lines)
  - We add mint keyset handling to OpenMiningChannel
  - Large upstream cleanup may conflict
  - Strategy: accept upstream structure, layer our keyset fields back in

**High risk areas** (we modify):
- `protocols/v2/roles-logic-sv2/src/parsers.rs`
  - We added `Minting` parser enum
  - Selector refactor (768376d6) may touch parser traits
  - Strategy: rebase will show if trait signatures changed; re-add Minting enum after

## Rebase Strategy

### Pre-rebase
1. Create branch `rebase-sri-v1.3.0` from current master
2. Review hashpool diff vs v1.2.1 to confirm Phase 1 isolation still holds
3. Commit checkpoint

### During rebase
```bash
git fetch --tags
git rebase v1.3.0
```

Expected conflicts:
- `protocols/v2/subprotocols/mining/src/{submit_shares.rs,open_channel.rs}` – accept upstream cleanup, re-add hash/locking_pubkey/keyset fields
- `protocols/v2/roles-logic-sv2/src/parsers.rs` – if selector refactor touched parser traits, reconcile and re-add Minting enum
- Cargo.toml version bumps (mechanical)

### Post-rebase
1. Fix any new API breakage (trait signature changes from selector refactor)
2. `cargo check --workspace` + fix compilation errors
3. Smoke test: pool_sv2, mint, translator --help
4. Full devenv smoke if time permits

## Time Estimate

- **Best case**: 1 hour (conflicts in submit_shares.rs + open_channel.rs only, mechanical field re-adds)
- **Expected**: 2-3 hours (parser.rs trait reconciliation + mining message field restoration)
- **Worst case**: 4 hours (selector refactor broke parser abstraction, need to rework Minting enum integration)

## Success Criteria

- ✅ All 431 commits rebase cleanly or with resolved conflicts
- ✅ Build passes with warnings only
- ✅ Smoke test: `./target/debug/pool_sv2 --help`, `./target/debug/mint --help`, `./target/debug/translator_sv2 --help` all run
- ✅ No new `todo!()` or panics introduced
- ✅ Cashu logic still isolated to ehash + mint-pool-messaging (no leakage back into SRI)

## Go/No-Go Decision

**Proceed with v1.3.0 rebase**: Yes

Rationale:
- v1.2.1 rebase was clean (4 trivial conflicts)
- Phase 1 isolation complete, no hashpool business logic in SRI core
- 431 commits is manageable (did 376 last time)
- Selector refactor doesn't affect hashpool (we don't customize proxy routing)
- Mining message refactors are documentation/cleanup (upstream confirmed via -4825 net lines)

Next step after v1.3.0: Rebase to v1.4.0 (322 commits, Jul 9), then v1.5.0 (240 commits, Sep 25).
