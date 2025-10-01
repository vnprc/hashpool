# REBASE_SRI.md

## Goal
Prepare the existing hashpool codebase (built on an older SRI commit) for a smooth rebase onto the current SRI without losing the mining → mint quote → translator sweep flow. We reshape what we already have first, then rebase once the diff surface is tiny and well understood.

## Strategy Overview
1. **Tighten the current fork**: isolate Cashu logic, keep the share hash explicit, add regression coverage.
2. **Document the minimal patch set**: know exactly which SRI files we touch and why.
3. **Rebase only after the fork is “rebase-ready”**: once the custom logic lives in small shims, the rebase becomes mostly conflict resolution and API updates.

## Phase 0 – Snapshot & Baseline (1 day)
1. Create a dedicated worktree rooted at `e8d76d68642ea28aa48a2da7e41fb4470bbe2681` (e.g., `git worktree add ../sri-baseline e8d76d6`) to make comparisons easy while keeping master untouched.
2. Record the current diff against that fork point (list modified files) using the worktree as reference.
3. Catalog existing ehash-related unit/integration tests, document the missing scenarios, and note any low-effort checks we can add immediately versus ones to tackle in Phase 1.
4. Run the available automated tests plus a manual share → mint quote → sweep smoke; log both the results and any coverage gaps surfaced by step 3.

## Phase 1 – Isolate Cashu Logic (2–3 days)
1. Create or update a `hashpool-cashu` module/crate that owns:
   - Share hash calculations (using the explicit `share_hash` field).
   - Mint quote request/response handling.
   - Translator wallet polling/sweep helpers.
2. Replace inline Cashu code in SRI files with thin calls to this module. Keep edits small and localized.
3. Add unit tests inside the module (hash determinism, quote builder).
4. Add/extend integration tests that simulate share submission and mock mint responses.
5. Checkpoint: `cargo fmt`, `cargo test --workspace`, manual share flow.

## Phase 2 – Document the Rebase Surface (1 day)
1. Produce `REBASE_NOTES.md` (or update if existing) listing each SRI file we modify, the reason, and the replacement shim function.
2. Identify any patches that could be upstreamed later (pure bug fixes) versus those we keep local.
3. Confirm there are no lingering “big” diffs outside the documented list.

## Phase 3 – Harden with Automation (ongoing)
1. Add a regression script (can be a `just` target) that spins up translator + pool + mock miner, submits a share, and verifies:
   - Share hash captured correctly.
   - Mint quote created and stored.
   - Translator sweep loop processes the quote.
2. Integrate this smoke test into CI so future changes can be validated quickly.

## Phase 4 – Rebase onto Latest SRI (after Phase 1–3 complete)
1. Fetch latest SRI (`git fetch upstream`) and create a rebase branch.
2. Rebase hashpool onto upstream, resolving conflicts mostly in the documented files.
3. Update any shim code to match new APIs.
4. Run full test suite + regression script + manual mining smoke.
5. Once green, land the rebased branch.

## Success Criteria
- Share hash field remains available and feeds Cashu logic directly (no TLV dependency).
- Cashu-specific logic lives in a small, well-tested module; SRI changes limited to thin adapters.
- `REBASE_NOTES.md` accurately describes all intentional diffs.
- Regression automation proves the end-to-end Cashu flow before and after the rebase.
- Rebase completes with manageable conflicts and minimal surprises.

## Commands & Checks
```bash
# Format + lint + test
cargo fmt
cargo clippy --workspace --all-targets
cargo test --workspace

# Integration smoke (example placeholder)
just run-share-smoke
```
