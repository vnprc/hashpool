# REBASE_SRI.md

## Goal
Prepare the existing hashpool codebase (built on an older SRI commit) for a smooth rebase onto the current SRI without losing the mining → mint quote → translator sweep flow. We reshape what we already have first, then rebase once the diff surface is tiny and well understood.

## Strategy Overview
1. **Tighten the current fork**: isolate Cashu logic, keep the share hash explicit, add regression coverage.
2. **Document the minimal patch set**: know exactly which SRI files we touch and why.
3. **Rebase only after the fork is “rebase-ready”**: once the custom logic lives in small shims, the rebase becomes mostly conflict resolution and API updates.

## Status – 2025-10-01
- Phase 0 snapshot/baseline tasks are complete and captured in `REBASE_NOTES.md`.
- Phase 1 keeps rolling. Recent commits (`b4b3ca4e`, `5d78302f`, `115c1f89`, `b73a3985`, `d23880b9`, `cf871e9b`) now centralize share-hash math, quote builders, and keyset parsing inside `protocols/ehash`; pool + translator call sites consume those helpers end-to-end, and the mint bridge now reuses the same helpers.
- Outstanding cleanup: replace the two `todo!()` guards in `roles/pool::message_handler`, retire any dead Cashu adapters left under `mining_sv2` once every caller moves across, and fan out the new quote helpers to `mint_pool_messaging` so TCP + channel paths stay in sync.
- Next chunk: audit `mint_pool_messaging` / integration harnesses for lingering raw-byte conversions, delete the redundant quote conversion code under `roles/mint` tests, then address the pool-side `todo!()` cases and close the loop with a regression test.

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
1. Use the existing `devenv` stack (`devenv shell` → `devenv up`) as the canonical ehash flow harness. Let it boot the pool, mint, translator, job declarator, and mock miner.
2. Document the manual verification steps while the stack runs:
   - Monitor `logs/` for translator share submissions and mint quote creation.
   - Hit `http://127.0.0.1:3030/api/miners` (translator) and `http://127.0.0.1:8081/api/connections` (pool) to confirm share counts and quote totals advance.
   - Optional: query the Cashu wallet (`just balance`) or SQLite databases to confirm issued quotes.
3. When we’re ready, script the above devenv interactions (curl checks) into CI-friendly probes, but treat the full stack as the single source of truth.

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
