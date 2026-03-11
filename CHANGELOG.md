Hashpool Changelog

---

## v0.1.1 (2026-03-09)

### Breaking Changes

- **Template Provider replaced**: the Sjors SV2 fork of bitcoind is no longer
  supported. Replace it with Bitcoin Core 30.2 (`bitcoin-node`) and
  sv2-tp v1.0.6. See the Debian 12 deployment guide in `docs/deployment.md`
  for the new setup procedure.

- **SRI 1.7.0 migration**: internal protocol crate APIs changed significantly.
  Downstream consumers of vendored crates should review
  `docs/archive/sri-1.7.0-upgrade-plan-v2.md` for detailed API change notes.

### Added

- Flake packages for the full stack: `pool`, `mint`, `translator`, `jd-server`,
  `jd-client`, `bitcoin-node`, `sv2-tp`. All reachable via `nix build .#<name>`.
- **Stats + web roles**: added stats services and web dashboards for pool/proxy
  monitoring and ehash wallet visibility.
- **Testnet deployment**: introduced a dedicated testnet instance (configs,
  deploy flow, and docs) for staging upgrades before mainnet.

### Fixed

- **Production deployment configs**: corrected template provider port for the
  pool/JDS/JDC prod configs (TP listens on 127.0.0.1:48442 on testnet4).

- **Mint HTTP bind**: mint now binds to IPv4 localhost to match nginx's
  127.0.0.1 upstream (fixes 502 Bad Gateway on mint quote status).

- **Share difficulty formula** (`roles/roles-utils/stratum-translation/src/sv2_to_sv1.rs`):
  `build_sv1_set_difficulty_from_sv2_target` now uses the SV2 formula
  `2^256 / target` instead of the Bitcoin formula `genesis_target / target`.
  The old formula produced difficulty ≈ 0.000778 for typical vardiff targets,
  causing miners to clamp to difficulty=1.0 and submit every hash as a share.

- **CoinbaseOutputConstraints encoding**: the pool now sends the correct 6-byte
  little-endian format required by SRI 1.7.0's pool↔Template Provider protocol.

### Changed

- **SRI 1.7.0 migration complete**: `roles_logic_sv2` (deprecated upstream) and
  all unmodified vendored crates replaced with crates.io imports. Remaining
  vendored crates are limited to those with hashpool-specific modifications:
  - `protocols/v2/mining_sv2`: custom 0xC0/0xC1 message types (ehash accounting)
  - `protocols/v2/parsers_sv2`: Mining enum variants for custom messages
  - `protocols/v2/channels_sv2`: `ValidWithAcknowledgement` variant and
    `header_hash_bytes()` method (not yet in crates.io)
  - `protocols/v2/handlers_sv2`: depends on custom parsers_sv2 via path dep

- **Template Provider**: replaced Sjors fork (sv2-tp-0.1.17, 4-byte
  CoinbaseOutputDataSize) with official Bitcoin Core 30.2 + sv2-tp v1.0.6.
  bitcoin-node connects via IPC unix socket; sv2-tp auto-discovers it.

- **Deployment scripts**: overhauled to support Debian 12 VPS workflows,
  including build-in-place and ship-only flows, staged rsync, and systemd/nginx
  orchestration.
- **Ehash mint flow**: redesigned end-to-end minting flow and message plumbing
  between pool, translator, and mint for more reliable share accounting.
- **Crate layout**: refactored hashpool + ehash logic into dedicated crates to
  reduce role coupling and speed up iteration.

- **Bench suite**: removed broken sv2 bench targets that referenced
  roles_logic_sv2 API removed during the SRI 1.7.0 migration.

### Removed

- sv2-apps cherry-picks (HTTP monitoring/Swagger UI, hotpath perf metrics) —
  deferred indefinitely, not needed for v0.1.1.

- Async handler trait migration (Phase 3 of the SRI plan) — deferred to post-v0.1.1.

---

## v0.1.0

Initial release: Stratum V2 pool with Cashu ecash share accounting, based on
the Sjors SV2 fork of Bitcoin Core and SRI codebase pre-1.7.0.
