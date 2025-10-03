# Hashpool Reapplication Plan - Git Worktree Approach

## Overview

**Current State:**
- Hashpool master: 1,149 commits ahead of SRI v1.0.0
- Target: SRI upstream/main @ fa5138a4 (v1.5.0-28)
- Worktree created: `/home/evan/work/hashpool-reapply`
- Rebase to v1.3.0 abandoned at 8.5% due to major architectural changes

**Strategy:**
Instead of incremental rebasing, reapply hashpool's custom changes onto the latest SRI codebase cleanly.

---

## Hashpool-Specific Changes Analysis

### Core Infrastructure Additions

1. **protocols/ehash/** - New protocol crate (9 files)
   - `keyset.rs` - Cashu keyset management
   - `quote.rs` - Mint quote types + building functions
   - `quote_handler.rs` - Generic quote handling with callbacks ⭐ NEW
   - `callbacks.rs` - Callback trait for quote events ⭐ NEW
   - `share.rs` - Mining share hash types
   - `sv2.rs` - SV2 message integration
   - `work.rs` - Work value calculations

2. **protocols/v2/stats-sv2/** - Stats message protocol ⭐ NEW
   - SV2-style TCP messaging for stats (not HTTP)
   - `PoolStatsMessage` enum (Encodable/Decodable)
   - `ProxyStatsMessage` enum (Encodable/Decodable)
   - Follows SV2 patterns for consistency

3. **roles/mint/** - New mint role (10 files)
   - `main.rs` - Mint service entry point
   - `mint_manager/` - Cashu mint operations
   - `sv2_connection/` - SV2 protocol handling for quotes

4. **roles/pool-stats/** - Pool stats service ⭐ NEW
   - **TCP server** (listens for pool connections, receives `PoolStatsMessage`)
   - **SQLite database** (time-series storage: hashrate_samples, quote_history)
   - **HTTP server** (dashboard only - serves HTML/CSS/JS + API endpoints)
   - **Hashrate aggregation** (5-minute samples, graphs over time)
   - **Independent deployment** (can restart without affecting pool)
   - **Replaces:** pool's embedded web.rs (621 lines deleted from pool)

5. **roles/proxy-stats/** - Proxy stats service ⭐ NEW
   - **TCP server** (listens for translator connections, receives `ProxyStatsMessage`)
   - **SQLite database** (time-series storage: miner_hashrate, share_history)
   - **HTTP server** (dashboard only - serves HTML/CSS/JS + API endpoints)
   - **Miner hashrate aggregation** (per-miner stats, 5-minute samples)
   - **Independent deployment** (can restart without affecting translator)
   - **Replaces:** translator's embedded web.rs (997 lines deleted from translator)

6. **roles/roles-utils/mint-pool-messaging/** - New messaging layer (5 files)
   - `channel_manager.rs` - TCP channel management
   - `message_hub.rs` - Quote routing hub (378 lines)
   - `message_codec.rs` - Message serialization

7. **roles/roles-utils/config/** - Shared config utilities
   - Extracted from pool/translator for reuse

### Core Integration Points

5. **Pool Role Modifications**
   - `roles/pool/src/lib/mod.rs` - Hub integration, TCP client to pool-stats
   - `roles/pool/src/lib/mining_pool/mod.rs` - Quote handler integration (uses callbacks)
   - `roles/pool/src/lib/mining_pool/quote_callbacks.rs` - Stats callback implementation ⭐ NEW
   - **DELETED:** `roles/pool/src/lib/web.rs` (621 lines - moved to pool-stats service)
   - **DELETED:** `roles/pool/src/lib/stats.rs` (stats now sent via TCP to pool-stats)

6. **Translator Role Modifications**
   - `roles/translator/src/lib/mod.rs` - Hub integration, TCP client to proxy-stats
   - `roles/translator/src/lib/proxy/bridge.rs` - Wallet and quote handling
   - **DELETED:** `roles/translator/src/lib/web.rs` (997 lines - moved to proxy-stats service)
   - **DELETED:** `roles/translator/src/lib/miner_stats.rs` (stats now sent via TCP to proxy-stats)

7. **JD-Client Role Modifications**
   - `roles/jd-client/src/lib/downstream.rs` - Minimal changes
   - `roles/jd-client/src/lib/proxy_config.rs` - Config handling

### Development Environment

8. **Nix/Devenv Setup**
   - `flake.nix`, `flake.lock`, `devenv.nix`, `devenv.lock`
   - `bitcoind.nix` - Bitcoin regtest setup
   - `justfile` - Build automation (130 lines)

9. **Configuration Files**
   - `config/` - Centralized config directory
   - Multiple `.toml` files for each role

10. **Documentation**
    - `README.md` - Hashpool-specific intro
    - `REBASE_*.md`, `EHASH_PROTOCOL.md`, etc.
    - Various planning docs

---

## Reapplication Strategy

### Phase 1: Foundation (Minimal Dependencies)
**Goal:** Establish new crates and basic infrastructure

1. Add `protocols/ehash/` crate
   - Pure domain logic, no SRI dependencies beyond basic types
   - Should compile independently

2. Add `roles/roles-utils/config/` crate
   - Shared config utilities
   - May need adjustment for v1.5.0 config patterns

3. Add development environment
   - `flake.nix`, `devenv.nix`, `bitcoind.nix`
   - `justfile`, `config/` directory
   - Verify builds with new SRI structure

### Phase 2: Messaging Infrastructure
**Goal:** Establish pool↔mint communication + stats services

4. Add `protocols/v2/stats-sv2/` crate
   - SV2-style stats message protocol
   - `PoolStatsMessage` and `ProxyStatsMessage` enums
   - Encodable/Decodable for TCP messaging

5. Add `roles/roles-utils/mint-pool-messaging/` crate
   - TCP channel management
   - Message hub for quote routing
   - Likely needs updates for v1.5.0 async patterns

6. Add mint role
   - `roles/mint/` - new workspace member
   - SV2 connection handling
   - Quote processing

7. Add stats services
   - `roles/pool-stats/` - TCP server + SQLite + HTTP dashboard
   - `roles/proxy-stats/` - TCP server + SQLite + HTTP dashboard
   - Move web.rs code from pool/translator (1,618 lines total)

### Phase 3: Pool Integration
**Goal:** Integrate quote handler + TCP stats client into pool

8. Modify pool role
   - Add mint hub to `roles/pool/src/lib/mod.rs`
   - Add `QuoteHandler` with callbacks in `mining_pool/mod.rs` message handler
   - Add TCP client connection to pool-stats service
   - **Delete:** `web.rs` (621 lines - moved to pool-stats)
   - **Delete:** `stats.rs` (stats now via TCP messages)
   - **Key risk:** Pool architecture may have changed significantly in v1.5.0

### Phase 4: Translator Integration
**Goal:** Integrate wallet + TCP stats client into translator

9. Modify translator role
   - Add wallet and hub integration
   - Update bridge for quote handling
   - Add TCP client connection to proxy-stats service
   - **Delete:** `web.rs` (997 lines - moved to proxy-stats)
   - **Delete:** `miner_stats.rs` (stats now via TCP messages)
   - **Key risk:** Translator may have architectural changes

### Phase 5: JD-Client Integration
**Goal:** Minimal JDC changes if needed

10. Modify JD-client (if necessary)
    - Review if any hashpool changes are needed
    - Likely minimal modifications

### Phase 6: Testing & Documentation
**Goal:** Verify everything works

11. Update documentation
    - README.md with hashpool specifics
    - Configuration examples
    - Stats service deployment guide

12. Smoke testing
    - Run devenv environment
    - Verify quote flow: miner → pool → mint → wallet
    - Verify stats flow: pool → pool-stats (TCP), translator → proxy-stats (TCP)
    - Test dashboard access (pool-stats on :8080, proxy-stats on :8081)
    - Test service independence: restart pool without killing pool-stats

---

## Key Risks & Considerations

### High-Risk Areas
1. **Pool message handling** - SRI v1.5.0 may have refactored share submission flow
2. **Translator bridge** - Likely architectural changes between v1.0.0 and v1.5.0
3. **Async patterns** - SRI may have updated async/await patterns
4. **Config structure** - Config file formats may have changed

### Medium-Risk Areas
1. **Mint-pool messaging** - TCP channel patterns may need updates
2. **Stats services** - New architecture with TCP + SQLite + HTTP (not in SRI upstream)
3. **Stats protocol** - New `protocols/v2/stats-sv2/` crate needs SV2 encoding integration
4. **Cargo.toml dependencies** - Version conflicts likely

### Low-Risk Areas
1. **protocols/ehash** - Pure domain logic, should port cleanly
2. **Development environment** - Nix setup is hashpool-specific
3. **Documentation** - Just needs updating

---

## Execution Plan

### Step 1: Create clean branch in worktree
```bash
cd /home/evan/work/hashpool-reapply
git checkout -b reapply-hashpool-clean
```

### Step 2: Apply Phase 1 (Foundation)
```bash
# Copy protocols/ehash/ from master
# Copy roles/roles-utils/config/ from master
# Copy devenv files, justfile, config/
# Attempt build, fix compilation errors
```

### Step 3: Apply Phase 2 (Messaging + Stats Services)
```bash
# Copy protocols/v2/stats-sv2/
# Copy roles/roles-utils/mint-pool-messaging/
# Copy roles/mint/
# Copy roles/pool-stats/
# Copy roles/proxy-stats/
# Update for v1.5.0 patterns
# Attempt build
```

### Step 4: Apply Phase 3-5 (Integration)
```bash
# Incrementally apply pool modifications:
#   - Add QuoteHandler integration
#   - Add TCP client to pool-stats
#   - DELETE web.rs and stats.rs
# Test compilation at each step
# Apply translator modifications:
#   - Add TCP client to proxy-stats
#   - DELETE web.rs and miner_stats.rs
# Apply JDC modifications (if needed)
```

### Step 5: Testing
```bash
# Update devenv.nix to include pool-stats and proxy-stats processes
# Run justfile targets
# Verify quote flow end-to-end
# Verify stats flow: pool → pool-stats, translator → proxy-stats
# Test dashboards accessible on :8080 and :8081
# Test service independence: restart pool while pool-stats running
# Update documentation
```

---

## Decision Points

**Before starting Phase 3:**
- Review how pool handles `SubmitSharesExtended` in v1.5.0
- Check if share submission flow has changed
- Verify async patterns match expectations

**Before starting Phase 4:**
- Review translator bridge architecture in v1.5.0
- Check if upstream connection patterns changed
- Verify wallet integration points

---

## Success Criteria

1. ✅ All hashpool-specific code compiles on v1.5.0 base
2. ✅ Devenv environment starts successfully (including pool-stats and proxy-stats)
3. ✅ Mining device → translator → pool → mint flow works
4. ✅ Quotes are created for shares via `QuoteHandler` with callbacks
5. ✅ Mint processes quotes and returns proofs
6. ✅ Wallet stores proofs correctly
7. ✅ Stats services receive messages via TCP:
   - Pool sends `PoolStatsMessage` to pool-stats
   - Translator sends `ProxyStatsMessage` to proxy-stats
8. ✅ Dashboards accessible and display data:
   - Pool dashboard on :8080 (hashrate graphs, quote history)
   - Proxy dashboard on :8081 (miner stats, hashrate graphs)
9. ✅ Service independence verified:
   - Restart pool without killing pool-stats
   - Restart translator without killing proxy-stats

---

## Comparison vs Incremental Rebase

| Aspect | Incremental Rebase | Clean Reapply |
|--------|-------------------|---------------|
| Conflicts | 377 commits to resolve | Focused on integration points |
| Architecture | Fight with intermediate changes | Adapt directly to v1.5.0 |
| Reverted code | Must handle revert commits | Ignore reverts entirely |
| Progress tracking | Commit-by-commit | Phase-by-phase |
| Estimated time | 30-50 hours | 10-20 hours |
| Risk of errors | High (manual conflict resolution) | Medium (architectural changes) |

---

## Next Steps

1. Review pool and translator architecture in worktree
2. Start Phase 1: copy `protocols/ehash` and attempt compilation
3. Fix any type mismatches or API changes
4. Proceed incrementally through phases
