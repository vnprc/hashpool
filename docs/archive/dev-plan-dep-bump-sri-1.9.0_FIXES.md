# Plan: Fix cdk-ehash Git URL Before Merging M4

## Context

M4 (dep-bump/m4-cdk-update) is ready to merge. Before merging, `cdk-ehash` in
`roles/mint/Cargo.toml` still points to GitHub instead of forge.anarch.diy.

The `cdk-ehash` repo lives locally at `~/work/cdk-ehash` with `origin` pointing to
`ssh://git@forge.anarch.diy:2222/vnprc/cdk-ehash.git` — so the forge copy exists.

The `vnprc/cdk` fork (for CDK patches) is intentionally left on GitHub for now (see
below). That fork is still needed because the translator runs a CDK wallet that uses
P2PK signing key auto-detection (`send-p2pk-signing-keys` branch, PR #1835 open).
Once PR #1835 merges upstream, all CDK `[patch.crates-io]` entries can be dropped.
cdk-ehash itself does NOT require the fork — its production deps are vanilla cdk-common
0.16.0 and its own `[patch]` section (which consumers ignore) points to cashubtc/cdk.

## P2PK Branch Status

The `send-p2pk-signing-keys` branch was rebased by Codex during M4. The maintainer
(`thesimplekid`) had previously force-pushed their version of the branch (commit
`d66bab61`). Codex then rebased and force-pushed again (`9523a003`).

**Verdict: no code was lost.** The maintainer's changes are fully preserved in content:
- `eb0532de` (maintainer's P2PK commit) → `3a39c4fb` (Codex rebase): identical diff
- `d66bab61` (maintainer's coin selection fix) → `2c898e17` (Codex rebase): identical
  except one CI workflow file change that came in from upstream via `db0f01c5`

The only addition is `9523a003 Fix cashu standalone hashes std feature`, needed for
hashpool's standalone pool CDK feature build.

The maintainer should be notified of the force push so they're not surprised by the
changed SHAs. Point them to `d66bab61` in reflog to verify content is preserved.

---

## Fix 1: cdk-ehash URL — in M4 branch

**File:** `roles/mint/Cargo.toml:25`

Change:
```toml
cdk-ehash  = { git = "https://github.com/vnprc/cdk-ehash", rev = "b7edd52" }
```
To:
```toml
cdk-ehash  = { git = "https://forge.anarch.diy/vnprc/cdk-ehash.git", rev = "b7edd52" }
```

The CDK fork patches (`vnprc/cdk`) stay on GitHub for now — needed for the translator
wallet's P2PK spending. Revisit when PR #1835 merges upstream.

---

## Fix 2: Quote poller status inference — in M4 branch

**File:** `roles/pool/src/lib/mining_pool/quote_poller.rs`

The `MintQuoteStatusResponse` has an `amount: Option<u64>` field (total expected).
Use it to guard the `Issued` transition: only declare `Issued` when `issued >= expected`.
If `paid == issued` but `issued < expected`, keep polling (treat as `Paid`).

Current `status()` method (`quote_poller.rs:396-404`):
```rust
fn status(&self) -> MintQuoteStatus {
    if self.amount_paid == 0 && self.amount_issued == 0 {
        MintQuoteStatus::Unpaid
    } else if self.amount_paid > self.amount_issued {
        MintQuoteStatus::Paid
    } else {
        MintQuoteStatus::Issued
    }
}
```

Replace with:
```rust
fn status(&self) -> MintQuoteStatus {
    if self.amount_paid == 0 && self.amount_issued == 0 {
        MintQuoteStatus::Unpaid
    } else if self.amount_paid > self.amount_issued {
        MintQuoteStatus::Paid
    } else if let Some(expected) = self.amount {
        if self.amount_issued >= expected {
            MintQuoteStatus::Issued
        } else {
            MintQuoteStatus::Paid  // partial issuance, keep polling
        }
    } else {
        MintQuoteStatus::Issued
    }
}
```

Update the test `test_mint_quote_status_response_issued_for_custom_response` to verify
the new guard works correctly.

---

## Fix 3: Translator extranonce split — new branch after M4 merges

**Problem:** Pool and JD-client migrated to `ExtranonceAllocator` (upstream API) in M2.
The translator was not migrated and still uses `ExtendedExtranonce` from vendored
`protocols/v2/roles-logic-sv2/src/extranonce.rs`.

The APIs differ fundamentally:
- `ExtranonceAllocator` is designed for **pool-level** multi-channel allocation
  (methods: `allocate_extended(min_size)`, `allocate_standard()`)
- The translator uses **per-channel** `ExtendedExtranonce` factories
  (methods: `from_upstream_extranonce(prefix, range0, range1, range2)`,
  `get_range2_len()`, `next_prefix_extended(size)`, `get_range0_len()`)

**Why it's a separate branch:** This is architecturally non-trivial — per-channel
factories may not map cleanly to the multi-channel `ExtranonceAllocator`. Migrating
incorrectly could break SV1 miner extranonce assignment. Needs its own branch and
careful testing isolated from the CDK changes.

**Also fixes Fix 4:** Once the vendored `extranonce.rs` module is gone, the three
conflicting `MAX_EXTRANONCE_LEN` definitions can be reconciled to one.

Files involved:
- `roles/translator/src/lib/sv2/channel_manager/channel_manager.rs:96`
  (extranonce_factories field type)
- `roles/translator/src/lib/sv2/channel_manager/mining_message_handler.rs:157,228`
  (ExtendedExtranonce::from_upstream_extranonce call sites)
- `protocols/v2/roles-logic-sv2/src/extranonce.rs` (remove once translator migrated)
- `protocols/v2/roles-logic-sv2/src/lib.rs` (remove extranonce re-exports)
- `protocols/v2/roles-logic-sv2/src/errors.rs` (remove ExtendedExtranonceError)

---

## Verification

After M4 fixes (1 + 2):
- `cargo check --manifest-path roles/Cargo.toml` passes
- `cargo check --manifest-path protocols/Cargo.toml` passes
- `cargo test --manifest-path roles/Cargo.toml -p pool` passes (quote_poller tests)
- Commit to `dep-bump/m4-cdk-update`, merge to master

After translator extranonce branch (Fix 3):
- `cargo check` passes in both workspaces
- Full stack smoke test with SV1 miners connecting (critical path for extranonce)
- Verify per-channel extranonce assignment is correct
