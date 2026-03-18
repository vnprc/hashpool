# Migration Plan: vnprc/cdk Fork → Standalone cdk-ehash + Upstream CDK

## Background

Hashpool currently depends on a custom fork of CDK at `https://github.com/vnprc/cdk.git`
(rev `9a634ec0`). This fork was originally needed while NUT-29 batch minting and custom
payment method support were being developed. Both features have since landed in upstream
`cashubtc/cdk` main. The fork is no longer needed.

The `cdk-ehash` payment processor has been extracted into a standalone crate at
`~/work/cdk-ehash` (to be published as `https://github.com/vnprc/cdk-ehash`).

---

## Goal

Drop the vnprc/cdk fork entirely. Replace all `git = "https://github.com/vnprc/cdk.git"`
dependencies with:

- Crate-level version deps (`"0.15.1"`)
- Workspace-level `[patch.crates-io]` redirecting to **upstream** `cashubtc/cdk` at a
  pinned commit

| Dep | Current | After migration |
|---|---|---|
| `cdk` (core) | git dep → vnprc fork | `"0.15.1"` + `[patch]` → upstream |
| `cashu` | transitive via fork | `[patch]` → upstream |
| `cdk-common` | git dep → vnprc fork | `"0.15.1"` + `[patch]` → upstream |
| `cdk-ehash` | git dep → vnprc fork | standalone crate (path/git) |
| `cdk-axum` | git dep → vnprc fork | `"0.15.1"` + `[patch]` → upstream |
| `cdk-mintd` | git dep → vnprc fork | `"0.15.1"` + `[patch]` → upstream |
| `cdk-sqlite` | git dep → vnprc fork | `"0.15.1"` + `[patch]` → upstream |

When upstream CDK cuts a release that includes NUT-29, the exit path is to delete the
`[patch]` block and switch version deps to that release. Until then, the patch pins
to a known-good upstream commit.

---

## Why the Fork Is Not Needed

Every API hashpool uses was verified against upstream `cashubtc/cdk` at
`584c5015` (current `origin/main` as of 2026-03-18).

The pinned rev used throughout this plan is `290a473b` — one commit ahead of
`origin/main`, containing the `fix/extra-json-db-persistence` fix (persist
`MintQuote.extra_json` through SQLite/Postgres). This commit is the subject of
an upstream PR to `cashubtc/cdk`. Hashpool is tested against this branch to
validate the fix before the PR is opened. Once the PR merges, update the pin
to the resulting merge commit on `origin/main`.

| API | Introduced | In upstream main? |
|---|---|---|
| NUT-29: `with_batch_minting`, `UnitConfig`, `configure_unit` | `c78329af` | ✓ |
| NUT-29: `wallet.batch_mint`, `get_unissued_mint_quotes`, `fetch_mint_quote` | `c78329af` | ✓ |
| `Nut29Settings` | `c78329af` | ✓ |
| `hashed_derivation_index` (custom unit keyset derivation) | `046ea2b8` | ✓ |
| `create_mint_router_with_custom_cache` | `255db0c3` | ✓ |

The only functional commit in the fork that is not in upstream is `mint_with_signing_key`
(`9a634ec0`). Hashpool never calls this function; it can be ignored.

---

## Key Mechanism: `[patch.crates-io]` Instead of Direct Git Deps

Declare **version deps** in each crate's `Cargo.toml` and redirect them at the workspace
level:

```toml
# protocols/Cargo.toml and roles/Cargo.toml
[patch.crates-io]
cashu           = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk             = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-common      = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-axum        = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-mintd       = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-signatory   = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-sql-common  = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-sqlite      = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-http-client = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
```

Cargo applies these patches workspace-wide. A crate declaring `cdk-sqlite = "0.15.1"`
will be built against the pinned upstream git commit, no per-crate annotation required.
All CDK crate types resolve from the same source, so there are no type unification issues.

**Why pin the rev?** Unpinned git deps (`git = "..."` with no `rev`) silently change on
every `cargo update`, making builds non-reproducible. The pin should be updated
deliberately when a new upstream CDK commit is needed.

---

## Scope

Two workspaces need changes:
- `protocols/` workspace (`protocols/Cargo.toml`) — contains `protocols/ehash`
- `roles/` workspace (`roles/Cargo.toml`) — contains `roles/mint` and `roles/translator`

---

## Step 1 — Update `protocols/Cargo.toml`

Add a `[patch.crates-io]` section:

```toml
[patch.crates-io]
cashu           = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk             = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-common      = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-http-client = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-signatory   = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-sql-common  = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-sqlite      = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
```

---

## Step 2 — Update `roles/Cargo.toml`

Add CDK entries to the existing `[patch.crates-io]` section:

```toml
[patch.crates-io]
channels_sv2    = { path = "../protocols/v2/channels-sv2" }   # already present
cashu           = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk             = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-common      = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-axum        = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-mintd       = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-http-client = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-signatory   = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-sql-common  = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-sqlite      = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
```

---

## Step 3 — Update `protocols/ehash/Cargo.toml`

Change:
```toml
cdk-common = { git = "https://github.com/vnprc/cdk", rev = "9a634ec0" }
```
To:
```toml
cdk-common = { version = "0.15.1", features = ["mint"] }
```

The workspace patch from Step 1 redirects this to upstream at build time.

---

## Step 4 — Update `roles/mint/Cargo.toml`

### cdk-ehash: switch from fork to standalone

```toml
cdk-ehash = { git = "https://github.com/vnprc/cdk-ehash" }
```

### Core CDK and satellite crates: switch to version deps

Remove all `git = "https://github.com/vnprc/cdk.git" ...` annotations:

```toml
cdk        = "0.15.1"
cdk-axum   = "0.15.1"
cdk-mintd  = "0.15.1"
cdk-sqlite = "0.15.1"
```

The workspace patch handles the redirect to upstream git.

---

## Step 5 — Update `roles/translator/Cargo.toml`

```toml
cdk        = "0.15.1"
cdk-sqlite = "0.15.1"
```

---

## Step 6 — Pin `~/work/cdk-ehash/Cargo.toml`

The standalone cdk-ehash has its own `[patch.crates-io]` section that currently uses
unpinned git deps. Pin them to the same revision for reproducible local builds and tests:

```toml
[patch.crates-io]
cashu           = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk             = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-common      = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-http-client = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-signatory   = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-sql-common  = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
cdk-sqlite      = { git = "https://github.com/cashubtc/cdk", rev = "290a473b" }
```

Note: `[patch.crates-io]` entries in dependency crates are ignored by Cargo when that
crate is used as a library — only the root workspace's `[patch]` takes effect. The
cdk-ehash patch section only matters when running `cargo test` directly inside that repo.

---

## Step 7 — Build and Catalog Errors

```bash
cargo build -C protocols
cargo build -C roles
```

Since all CDK crates are patched to the same upstream commit, type unification issues are
unlikely. Most probable error sources:

- **`cdk-mintd::config::Settings` struct fields**: the upstream main version has added
  fields (`limits`, `auth`, `auth_database`, `mint_management_rpc`) since the crates.io
  `0.15.1` release. These are all `Option<_>` or `#[serde(default)]`, so existing TOML
  config files should still parse. Verify `config::Settings`, `config::Info`, and
  `config::MintInfo` against the field accesses in `setup.rs` and `main.rs`.
- **`cdk-axum` API changes**: `create_mint_router_with_custom_cache` signature is confirmed
  present in upstream main. Check for any parameter type changes.

---

## Step 8 — Update Lock Files

```bash
cargo update -C protocols
cargo update -C roles
```

Commit both updated lock files.

---

## Step 9 — Smoke Test

```bash
just clean cashu
devenv up
```

Verify:
1. Mint starts and registers the ehash payment method
2. Pool connects to mint and translator
3. Test miner submits shares → batch quotes created → tokens minted via NUT-29
4. Faucet API returns tokens

**Testing sequence with the upstream PR:** Steps 1–8 pin all CDK deps to
`290a473b` (the `fix/extra-json-db-persistence` branch tip). A passing smoke
test here validates both the migration and the upstream fix simultaneously.
After confirming green, open the PR to `cashubtc/cdk`. Once merged, update
every `rev = "290a473b"` entry to the resulting commit on `origin/main` and
run `cargo update -C protocols && cargo update -C roles` to regenerate lock
files.

---

## Decision: cdk-ehash Dependency Source

| Option | Pros | Cons |
|---|---|---|
| `path = "..."` | Instant local iteration | Machine-specific; breaks CI/CD |
| `git = "https://github.com/vnprc/cdk-ehash"` | Reproducible everywhere | Must push before testing |
| `cdk-ehash = "0.1"` (crates.io) | Production-standard | Requires publish workflow |

**Recommendation:** Use the git dep now. Publish `v0.1.0` to crates.io once the upstream
CDK patch is pinned to a specific release tag.

---

## Future: Removing the Upstream Patch

When upstream CDK cuts a release that includes NUT-29 and `hashed_derivation_index`:

1. Update all version deps (`cdk = "0.15.1"` etc.) to the new release version
2. Delete the `[patch.crates-io]` CDK entries from both workspace files
3. Run `cargo update` and fix any minor API deltas
4. Done

---

## Risk Summary

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `cdk-mintd` config struct has new required fields | Low | Low | New fields use `Option`/`default`; verify TOML parses |
| Upstream CDK main breaks between now and release | Medium | Medium | Rev is pinned; only moves when deliberately updated |
| Upstream CDK API breaks between pin update and smoke test | Low | Medium | Only update pin deliberately; test immediately after |
| Upstream CDK NUT-29 API differs from current main when released | Low | Low | Adapt the few call sites at that time |

---

## Files Changed Summary

| File | Change |
|---|---|
| `protocols/Cargo.toml` | Add `[patch.crates-io]` (upstream CDK pinned) |
| `protocols/ehash/Cargo.toml` | `cdk-common`: fork git dep → `"0.15.1"` version dep |
| `roles/Cargo.toml` | Add CDK entries to existing `[patch.crates-io]` |
| `roles/mint/Cargo.toml` | `cdk-ehash` → standalone path/git; `cdk`/`cdk-axum`/`cdk-mintd`/`cdk-sqlite` → `"0.15.1"` |
| `roles/translator/Cargo.toml` | `cdk`/`cdk-sqlite` → `"0.15.1"` |
| `~/work/cdk-ehash/Cargo.toml` | Pin `[patch.crates-io]` refs to `rev = "290a473b"` (pre-merge PR branch tip; update after merge) |
| `roles/mint/src/lib/mint_manager/setup.rs` | Fix any `cdk-mintd` / `cdk-axum` API deltas |
| `protocols/Cargo.lock` | Updated |
| `roles/Cargo.lock` | Updated |
