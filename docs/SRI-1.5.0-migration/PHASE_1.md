# SRI 1.5.0 Migration: Reapply Hashpool Changes

**Objective:** Get hashpool running on SRI 1.5.0 with **clean commit history**
**Strategy:** Start fresh from SRI 1.5.0, then manually reapply hashpool changes
**Philosophy:** SRI 1.5.0 is the base; hashpool changes layer on top

---

## Critical Context: Major SRI Changes from 1.2 → 1.5.0

Between the SRI 1.2.0 baseline (when hashpool forked) and 1.5.0:

- **Translator completely rewritten** (PR #1791): New modular architecture with channel aggregation and failover
- **Pool APIs changed**: Message dispatch, channel tracking, and routing logic
- **Job dispatch logic refactored**: New JDC (Job Dispatch Controller) with static dispatch
- **Error handling refactored**: Many new error types in roles_logic_sv2
- **Vardiff**: Changed from dynamic to static dispatch

**This means:** You cannot simply cherry-pick or rebase hashpool commits. You must **manually reapply hashpool logic on top of the new SRI 1.5.0 architecture**.

---

## Git Strategy: Fresh Base + Reapplication

**Start point:** SRI v1.5.0 (clean state)
**Work location:** hashpool repo, branch `migrate/sri-1.5.0`
**Commits:** All commits stay in hashpool (you control them)

```bash
cd /home/evan/work/hashpool

# 1. Add SRI as a remote (one time)
git remote add sri https://github.com/stratum-mining/stratum.git

# 2. Fetch SRI v1.5.0
git fetch sri v1.5.0

# 3. Create migration branch
git checkout -b migrate/sri-1.5.0 sri/v1.5.0

# 4. At this point you're on SRI 1.5.0 clean state
# The commits that follow will be hashpool-only
```

**Expected commit history after migration:**

```
SRI v1.5.0 (base from upstream)
    ↓
+1. Add hashpool-only protocols (ehash, mint-quote, stats-sv2)
+2. Add hashpool-only roles (mint, utilities)
+3. Reapply Pool integration: quote routing and channel tracking
+4. Reapply Translator integration: handle quotes and route to miners
+5. Add hashpool configuration system
+6. Integration tests and validation
```

All commits numbered "+1" through "+6" are new (yours), reapplying hashpool logic.

---

## Phase 0: Setup Branch (5 min)

```bash
cd /home/evan/work/hashpool

# Add SRI remote if not present
git remote add sri https://github.com/stratum-mining/stratum.git 2>/dev/null || true

# Fetch SRI v1.5.0
git fetch sri v1.5.0

# Create branch on top of SRI 1.5.0
git checkout -b migrate/sri-1.5.0 sri/v1.5.0

# Verify you're on clean SRI 1.5.0
cargo build --workspace 2>&1 | head -20
cargo test --lib --workspace 2>&1 | head -20
```

**Expected:** Clean build, all tests pass.

---

## Phase 1: Add Hashpool-Only Protocols & Roles (1-2 hours)

These crates are **100% new** (no SRI equivalents), so copy them directly:

```bash
# Copy hashpool-only code from master
git checkout master -- \
  protocols/ehash/ \
  protocols/v2/subprotocols/mint-quote/ \
  protocols/v2/subprotocols/stats-sv2/ \
  roles/mint/ \
  roles/roles-utils/mint-pool-messaging/ \
  roles/roles-utils/stats/ \
  roles/roles-utils/quote-dispatcher/ \
  roles/roles-utils/config/

# Update Cargo.toml workspace members
# Edit protocols/Cargo.toml to add new crate members
# Edit roles/Cargo.toml to add new role and utility members

# Build and validate
cargo build --workspace
cargo test --lib --workspace
```

**Commit:**
```
git add protocols/ roles/roles-utils/
git commit -m "Add hashpool-only protocols and roles (ehash, mint, stats-sv2)"
```

**Expected:** Clean build, all new crate tests pass.

---

## Phase 2: Reapply Pool Integration (3-4 hours)

**Goal:** Pool routes mint quotes to Translator, handles quote responses.
**Strategy:** Read SRI 1.5.0 Pool code, understand new architecture, then reapply hashpool logic.

### 2a. Study the new Pool Architecture

```bash
# Understand SRI 1.5.0 Pool structure
grep -n "pub struct Pool" roles/pool/src/lib.rs
grep -n "pub async fn.*message\|fn.*handle" roles/pool/src/lib.rs | head -20

# Understand message routing
grep -n "enum Message\|impl.*Message" roles/pool/src/lib.rs | head -20

# Understand channel tracking
grep -n "channel\|downstream" roles/pool/src/lib.rs | head -30
```

### 2b. Identify Integration Points

Compare with hashpool's current Pool code:

```bash
# What does hashpool Pool currently do?
git show master:roles/pool/src/lib/mining_pool/mod.rs | head -100
```

Key questions:
- How does SRI 1.5.0 Pool handle downstream connections?
- Where does message dispatching happen?
- What's the new channel tracking mechanism?
- How are messages routed to translators?

### 2c. Manually Add Hashpool Integration

Create new modules for hashpool-specific logic:

```bash
# Create hashpool integration modules
touch roles/pool/src/lib/mining_pool/ehash_handler.rs
touch roles/pool/src/lib/mining_pool/quote_dispatcher_integration.rs
```

Then edit `roles/pool/src/lib/mining_pool/mod.rs`:

1. Add new fields to Pool struct (quote dispatcher, mint connection, channel tracking)
2. Initialize them in Pool::new()
3. Add message handlers for mint quote responses
4. Add channel tracking when downstreams connect/disconnect

**Critical:** Don't modify SRI Pool code—only add new modules and delegate to them.

### 2d. Handle SRI 1.5.0 Breaking Changes

Expected issues and solutions:

| Issue | Solution |
|-------|----------|
| Pool struct API changed | Extend Pool with new fields, don't modify SRI struct fields |
| Message dispatch is different | Create wrapper handler that fits new message dispatch |
| Channel tracking different | Track channels in new hashpool module, not SRI code |
| Translator message format changed | Adapt to new Translator message types |

**Commit:**
```
git add roles/pool/
git commit -m "Reapply Pool integration for SRI 1.5.0

- Add quote dispatcher and mint connection management
- Handle mint quote responses and route to translators
- Track channel ownership for quote routing
- Integrate with new SRI 1.5.0 message dispatch"
```

---

## Phase 3: Reapply Translator Integration (3-4 hours)

**Goal:** Translator receives quote responses from Pool, routes to SV1 miners.
**Strategy:** Similar to Phase 2—study new architecture, then reapply.

### 3a. Study the New Translator Architecture

```bash
# Understand new translator structure
grep -n "pub struct Translator\|pub struct.*Translator" roles/translator/src/lib.rs
grep -n "fn.*message\|pub async fn" roles/translator/src/lib.rs | head -20

# Understand new upstream/downstream handling
grep -n "upstream\|downstream" roles/translator/src/lib.rs | head -30

# Understand message flow
grep -n "Message::" roles/translator/src/lib.rs | head -20
```

### 3b. Identify Integration Points

Key questions:
- How does new Translator structure differ from SRI 1.2.0?
- What's the new message handling pattern?
- How are SV1 connections managed?
- How does it connect to Pool?

### 3c. Manually Add Hashpool Integration

Similar to Pool:

```bash
touch roles/translator/src/lib/upstream_sv2/quote_handler.rs
touch roles/translator/src/lib/wallet_integration.rs
```

Then edit core Translator code to:

1. Add wallet field (for quote tracking)
2. Add quote response handler
3. Route quotes to miners
4. Handle quote confirmations

**Commit:**
```
git add roles/translator/
git commit -m "Reapply Translator integration for SRI 1.5.0

- Handle quote notifications from Pool
- Route quotes to SV1 miners
- Integrate wallet for quote tracking
- Adapt to new SRI 1.5.0 message dispatch architecture"
```

---

## Phase 4: Integration Tests & Validation (2-3 hours)

```bash
# Build everything
cargo build --workspace

# Run all unit tests
cargo test --lib --workspace

# Start devenv (even partial)
devenv shell
devenv up

# Verify:
# 1. Pool starts
# 2. Translator starts
# 3. Mint starts
# 4. No fatal errors in logs
# 5. Can connect miner and get shares accepted
```

**Expected:** Shares flow through pool→translator. Mint service ready. No quote flow yet (can add in Phase 2).

---

## What We're DEFERRING to Phase 2

**DO NOT DO in this migration:**

- ❌ Stats snapshots (keep basic logging only)
- ❌ Web dashboards (can use CLI tools)
- ❌ Advanced error handling improvements
- ❌ Performance optimizations
- ❌ Configuration refactoring
- ❌ Tests beyond unit/integration
- ❌ Documentation updates (just ensure code builds)

These can all happen **after** we're on 1.5.0.

---

## Timeline: Days Not Weeks

| Phase | Duration | Output |
|-------|----------|--------|
| Phase 0 | 5 min | Migration branch created |
| Phase 1 | 1-2 hrs | Hashpool-only crates added |
| Phase 2 | 3-4 hrs | Pool integration reapplied |
| Phase 3 | 3-4 hrs | Translator integration reapplied |
| Phase 4 | 2-3 hrs | Integration tests and validation |
| **TOTAL** | **~12-17 hours over 2-3 days** | **Hashpool on 1.5.0** |

---

## Success Criteria

Migration succeeds when:

✅ Branch `migrate/sri-1.5.0` is created from SRI v1.5.0
✅ `cargo build --workspace` succeeds
✅ `cargo test --lib --workspace` passes all tests
✅ All hashpool-only crates compile correctly
✅ Pool compiles with new integration logic
✅ Translator compiles with new integration logic
✅ devenv stack starts without fatal errors
✅ Miner can connect and submit shares
✅ Pool routes shares correctly
✅ Clean commit history: each commit does one logical thing

**NOT required for Phase 1:**
- ❌ Full quote→sweep flow working
- ❌ Web dashboards responsive
- ❌ Stats collection working
- ❌ Stats snapshots
- ❌ Advanced reliability features

---

## Expected Commit History

After completing all phases, `git log sri/v1.5.0..migrate/sri-1.5.0` should show:

```
+6 Integration tests and validation
+5 Reapply Translator integration for SRI 1.5.0
+4 Reapply Pool integration for SRI 1.5.0
+3 Add hashpool-only protocols and roles
```

Each commit is a complete, working state. No commits break the build.

---

## Why Not Rebase? Lessons Learned

The initial plan attempted to rebase hashpool commits on SRI v1.5.0. This failed because:

1. **Breaking API changes**: SRI 1.2 → 1.5.0 has major architectural changes (Translator rewrite, Pool dispatch refactor)
2. **Rebase conflicts multiply**: 3 years of divergent development creates cascading conflicts
3. **Git history becomes unreadable**: Rebasing on top of SRI means mixing "what SRI did" with "what hashpool added"
4. **Losing control**: If SRI is a remote upstream, you're constantly fighting upstream history

**This approach solves all of these:**

- ✅ Start from clean SRI v1.5.0 (no conflicts)
- ✅ Manually reapply hashpool logic (you control each piece)
- ✅ Clean commit history (each commit is yours, clear intent)
- ✅ Full ownership (all commits in hashpool repo)
- ✅ Easy to review (diff against SRI 1.5.0 baseline)

---

## Critical Principles

**DO NOT:**
- ❌ Try to rebase hashpool commits on top of SRI
- ❌ Modify SRI's core code (Pool, Translator, roles_logic_sv2)
- ❌ Change SRI test files
- ❌ Cherry-pick SRI commits
- ❌ Refactor SRI architecture

**DO:**
- ✅ Add new modules for hashpool-specific code
- ✅ Extend SRI types with new fields when needed
- ✅ Use composition: wrap SRI components, don't modify them
- ✅ Keep hashpool code clearly separated
- ✅ Make clean, reviewable commits with clear intent

---

## Phase 4 Success = Ready for Phase 2

When Phase 4 passes:
- ✅ `cargo build --workspace` succeeds
- ✅ `cargo test --lib --workspace` passes
- ✅ devenv stack starts without fatal errors
- ✅ Miner can connect and submit shares
- ✅ Pool routes shares correctly

You're ready for **Phase 2: Enhance & Deploy** (see docs/SRI-1.5.0-migration/PHASE_2.md)

---

**Created:** 2025-10-24
**Strategy:** Manual reapplication on clean SRI 1.5.0 base
**Next:** Phase 0 setup, then Phase 2
