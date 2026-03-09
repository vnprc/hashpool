Hashpool v0.2 Release Plan

Context

The SRI 1.7.0 migration is complete (Steps 2.1–2.13). The roles workspace builds cleanly, the TP
was replaced with bitcoin-node + sv2-tp v1.0.6, and the invalid-share-spam bug was fixed. The
integration test (devenv up with full stack + miner + ehash minting) is passing on regtest.

v0.2 is the first release on SRI 1.7.0. The goals for this release are:

1. Code cleanup — tidy up loose ends from the migration
2. Deterministic builds — make `nix build` produce the full stack reliably
3. NixOS deployment — write a NixOS module so the pool can be deployed as a system service
4. Live pool instance — stand up the pool on testnet4, validate end-to-end
5. Documentation — update README and write an operator guide covering all of the above
6. Release tag — tag v0.2 with all of the above in place

This plan intentionally excludes sv2-apps cherry-picks (Phase 4 of the SRI migration plan) and
the async handler trait migration (Phase 3). Neither is necessary for v0.2.

---
Phase 1: Code Cleanup (COMPLETE — 2026-03-09)

Step 1.1 — Fix or remove broken sv2 benches (DONE)

Removed sv2 bench targets from benches/Cargo.toml and deleted benches/benches/src/sv2/.
`cd benches && cargo build` passes.

Step 1.2 — Fix compiler warnings in roles workspace (DONE)

cargo fix applied to jd_client_sv2 (4 files), pool_sv2 (1 file), quote-dispatcher (1 file).
Mint dead-code warnings suppressed with #[allow(dead_code)] on infrastructure not yet wired up.
Remaining warnings in stats_proxy/stats_pool are pre-existing and out of scope.

Step 1.3 — Update the SRI 1.7.0 migration plan doc (DONE)

Verification Checkpoint 5 updated to "commit 66b56dac". Phase 1 heading updated to note
"Sjors fork fully retired".

Step 1.4 — Update README (DONE)

Replaced "bitcoind (Sjors' SV2 Fork)" with bitcoin-node (Bitcoin Core 30.2) and
sv2-tp (v1.0.6) as separate numbered components.

---
Phase 2: Deterministic Builds (flake.nix) (COMPLETE — 2026-03-09)

The flake.nix exists and has the right structure (crane + rust-overlay + pinned Rust), but it
will fail to build because:

  commonArgs.src = craneLib.cleanCargoSource (craneLib.path ./roles)

The roles workspace has path deps outside roles/ — protocols/ehash, protocols/v2/subprotocols/
mint-quote, protocols/v1/, and protocols/v2/roles-logic-sv2. These sources are not included in
the crane source set, so `nix build` fails when cargo tries to resolve them.

Step 2.1 — Fix flake.nix source path

Change the src to include both roles/ and protocols/ from the repo root. Use a path filter to
avoid pulling in build artifacts, docs, config, logs, etc.:

  src = lib.cleanSourceWith {
    src = craneLib.path ./.;
    filter = path: type:
      (craneLib.filterCargoSources path type) ||
      (lib.hasInfix "/protocols/" path) ||
      (lib.hasInfix "/roles/" path);
  };

Also set cargoLock to the roles Cargo.lock:

  cargoLock = ./roles/Cargo.lock;

Checkpoint: `nix build .#pool` succeeds.

Step 2.2 — Expose bitcoin-node and sv2-tp as flake packages

bitcoin-node.nix and sv2-tp.nix are already written as self-contained Nix derivations. Wire
them into flake.nix as named outputs:

  packages.bitcoin-node = import ./bitcoin-node.nix { inherit pkgs lib; stdenv = pkgs.stdenv; };
  packages.sv2-tp = import ./sv2-tp.nix { inherit pkgs lib; stdenv = pkgs.stdenv; };

This makes the full infrastructure stack fetchable with `nix build .#bitcoin-node` etc., and
allows a NixOS module to reference them cleanly.

Checkpoint: `nix build .#bitcoin-node` and `nix build .#sv2-tp` succeed.

Step 2.3 — Build all hashpool binaries from the flake

Add build targets for all roles that are deployed in production:

  packages.jd-server = ...
  packages.jd-client = ...
  packages.pool = ...       (already exists)
  packages.translator = ... (already exists)
  packages.mint = ...       (already exists)

Optionally add stats-pool, stats-proxy, web-pool, web-proxy if they will be deployed.

Step 2.4 — Add Rust version sanity check

The flake pins rustVersion = "1.87.0" while devenv uses the nixpkgs-provided Rust (1.86.0 per
memory notes, with pinned Cargo.lock workarounds for MSRV). Confirm 1.87.0 does not require the
Cargo.lock pins for time@0.3.41, home@0.5.11. If the locks are still needed, document why.

Checkpoint: `nix flake check` passes.

---
Phase 3: NixOS Deployment Module (COMPLETE — 2026-03-09)

Goal: A pool operator running NixOS can add hashpool as a flake input and enable all services
in their configuration.nix with a few lines. This is the right deployment target — devenv is
for development, NixOS is for production.

Architecture for a live deployment (testnet4 or mainnet):

  bitcoin-node  →  sv2-tp  →  pool  →  jd-server
                                    ←  jd-client  →  sv2-tp
  mint  (receives quote requests from pool and translator)
  translator (proxy, miner-facing)

Step 3.1 — Write nixosModules.hashpool (DONE)

Created nix/hashpool-module.nix

The module should provide:
- Options for: network (regtest/testnet4/mainnet), data directories, ports, config file paths
- systemd.services for each process: bitcoin-node, sv2-tp, pool, jd-server, jd-client,
  mint, translator
- Correct After=/Wants= ordering to reproduce the devenv waitForPort dependencies:
    bitcoin-node → sv2-tp → pool → jd-server
                          → jd-client
    mint (no hard dep, starts independently)
    translator (waits for pool)
- A single enable option (services.hashpool.enable) to turn the whole stack on
- Network-specific defaults (ports, chain flag)

Design decision: Do NOT hard-code config TOML paths inside the module. Instead, expose a
configDir option that points to a directory containing pool.config.toml, jdc.config.toml, etc.
This keeps the module flexible and avoids baking in testnet vs mainnet config.

Step 3.2 — Wire the module into flake.nix (DONE)

  nixosModules.hashpool = import ./nix/hashpool-module.nix self;
  nixosModules.default = self.nixosModules.hashpool;

Step 3.3 — Write a NixOS deployment example (DONE)

Created docs/nixos-deployment.md covering:

1. Prerequisites: NixOS with flakes enabled
2. Adding hashpool as a flake input
3. Minimal configuration.nix snippet to enable the full stack
4. How to set network=testnet4 and point config files to your config/ dir
5. How to override the coinbase_reward_script and other operator-specific settings
6. First-run steps: wallet creation, initial block download, confirming sv2-tp connects
7. Monitoring: log files, bitcoin-cli commands to verify node status, pool log output to
   confirm template receipt and share validation

---
Phase 4: Live Pool Instance (testnet4)

Goal: Deploy hashpool on the user's own server using the NixOS module from Phase 3. This
validates the deployment docs are accurate and produces a working testnet4 instance that can
be included in the v0.2 release announcement.

Step 4.1 — Provision server

Choose a VPS or dedicated server running NixOS. Minimum specs for testnet4:
- 2 vCPU, 4 GB RAM, 50 GB SSD (testnet4 chain is ~10 GB and growing)
- Public IP or DNS for miner connections (translator port)

Step 4.2 — Generate operator keys

The pool requires an authority keypair. The dev config uses a known test keypair — do not
use it in production. Generate fresh keys and set in pool.config.toml:
  authority_public_key = "<fresh>"
  authority_secret_key = "<fresh>"

Also set a real coinbase_reward_script pointing to a wallet you control.

Step 4.3 — Deploy via NixOS module

Follow the docs written in Step 3.3. Any gaps or errors discovered here feed back into the
docs before the release tag.

Step 4.4 — Validate end-to-end on testnet4

- Confirm bitcoin-node syncs to testnet4 chain tip
- Confirm sv2-tp connects via IPC and serves templates on port 8443 (testnet4 default)
- Connect a test miner (mining-device-sv1 or cpuminer-opt) to the translator port
- Confirm shares are accepted and ehash tokens are minted
- Confirm block found (mine a regtest block if testnet4 is slow) → ecash redeemable

Step 4.5 — Document production-specific config differences

The prod configs in config/prod/ cover most of this but may be stale. Update them to reflect:
- testnet4 ports (sv2-tp default: 8443 for testnet4, 18447 for regtest)
- Real coinbase_reward_script and pool_signature
- shares_per_minute tuned for real hardware (6.0 for a modern GPU miner, adjust per miner hashrate)

---
Phase 5: Documentation Updates

Step 5.1 — Update README.md

- Replace "Sjors' SV2 Fork" section with bitcoin-node + sv2-tp v1.0.6
- Update "Getting Started" to be accurate for current devenv setup
- Add a "Production Deployment" section with a pointer to docs/nixos-deployment.md
- Add a brief explanation of what v0.2 brings (SRI 1.7.0, new TP, fixed share accounting)

Step 5.2 — Write CHANGELOG entry for v0.2

Create or update CHANGELOG.md with v0.2 entries:
- SRI 1.7.0 migration complete (crates.io imports, roles_logic_sv2 deprecated)
- Template provider replaced: bitcoin-node 30.2 + sv2-tp v1.0.6 (replaces Sjors fork)
- Fixed: share difficulty formula in sv2_to_sv1.rs (SV2 formula 2^256/target)
- Fixed: CoinbaseOutputConstraints 6-byte format (SRI 1.7.0 pool↔TP protocol)
- Added: NixOS deployment module
- Removed: sv2-apps cherry-pick scope (deferred indefinitely)

Step 5.3 — Archive the SRI migration plan

Move docs/sri-1.7.0-upgrade-plan-v2.md to docs/archive/sri-1.7.0-upgrade-plan-v2.md to
signal that the migration work is complete and no longer an active plan. Keep it for reference.

---
Phase 6: Release Tag

Step 6.1 — Final verification checklist

Before tagging, confirm:
- [ ] cd roles && cargo build --workspace exits 0 with no errors
- [ ] nix build .#pool succeeds
- [ ] nix build .#bitcoin-node and .#sv2-tp succeed
- [ ] nix flake check passes
- [ ] devenv up runs the full regtest stack (bitcoin-node + sv2-tp + pool + mint + proxy + miner)
- [ ] Miner submits shares, proxy logs show accepted shares, mint logs show token issuance
- [ ] No benches compile errors (sv2 benches removed in Step 1.1)
- [ ] Live testnet4 pool is running (Step 4.4)
- [ ] README and docs are updated (Phase 5)

Step 6.2 — Tag v0.2

  git tag -s v0.2.0 -m "hashpool v0.2: SRI 1.7.0 migration, NixOS deployment, live testnet4 pool"
  git push origin v0.2.0

Create a GitHub release from the tag with the CHANGELOG entry as the release body.

---
Dependency Graph

  Phase 1 (cleanup)
      ↓
  Phase 2 (flake)
      ↓
  Phase 3 (NixOS module)  ←→  Phase 4 (live pool, validates Phase 3)
           ↓
  Phase 5 (docs, depends on 3+4 for accuracy)
           ↓
  Phase 6 (release tag)

Phase 1 is independent and can start immediately. Phases 2–4 should be done in order since the
NixOS module builds on the flake packages and the live deployment validates the module.

---
Out of Scope for v0.2

The following items were identified during the SRI 1.7.0 migration but deferred:

- sv2-apps cherry-picks (HTTP monitoring/Swagger UI, hotpath perf metrics) — not needed
- Async handler trait migration (Phase 3 of SRI plan) — low priority, post-v0.2
- CDK CLI as a Nix derivation (currently built via runtime git clone in devenv) — nice to have
- aarch64/macOS hashes for bitcoin-node.nix and sv2-tp.nix — only x86_64-linux has real hashes

---
Key Files Reference

  File                                          Phase   Change
  ─────────────────────────────────────────────────────────────────────────────────────
  benches/benches/src/sv2/                      1.1     Remove or stub broken sv2 benches
  benches/Cargo.toml                            1.1     Remove sv2 bench targets
  docs/sri-1.7.0-upgrade-plan-v2.md             1.3     Mark 2.12 committed, minor fixups
  README.md                                     1.4     Update TP section, add prod pointer
  flake.nix                                     2.1-2.4 Fix source path, expose all packages
  bitcoin-node.nix                              2.2     Expose as flake package
  sv2-tp.nix                                    2.2     Expose as flake package
  nix/hashpool-module.nix                       3.1     New NixOS module (all services)
  flake.nix (nixosModules output)               3.2     Wire module into flake
  docs/nixos-deployment.md                      3.3     New operator guide
  config/prod/                                  4.5     Update for testnet4 production use
  CHANGELOG.md                                  5.2     v0.2 entries
  docs/archive/sri-1.7.0-upgrade-plan-v2.md     5.3     Archived migration doc
