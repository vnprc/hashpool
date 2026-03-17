# Deploy Script Refactor: ship.sh + build.sh

## Context

`scripts/deploy.sh` and `scripts/deploy-build-in-place.sh` have grown into a single overloaded entry point with conflicting flags (`--build-in-place`, `--skip-build`, `--build-only`, `--config-only`, `--clean`, `--dry-run`) and 10+ invalid-combination checks. The scripts, config, and binary deploy targets are all mixed together. A `--no-restart` option doesn't exist yet.

The refactor separates two orthogonal concerns:
- **What** gets deployed (scripts, config, binaries, all)
- **How** it's built (locally and shipped as prebuilt artifacts vs. source shipped and built on target)

## New Script Structure

### `scripts/ship.sh` (replaces `deploy.sh`)
Build locally, ship prebuilt artifacts. Subcommands describe what to deploy:

```
./scripts/ship.sh scripts              # rsync hashpool-ctl.sh only, no restart
./scripts/ship.sh config               # configs + systemd + nginx, restart services
./scripts/ship.sh binaries             # prebuilt binaries only, restart services
./scripts/ship.sh all                  # full deploy: build + ship everything

Flags (where applicable):
  --no-restart     skip service stop/start cycle
  --skip-build     use existing binaries in roles/target/debug (binaries/all only)
  --clean          cargo clean before building (binaries/all only)
  --dry-run        preflight checks only, no deploy
```

### `scripts/build.sh` (replaces `deploy-build-in-place.sh`)
Ship source to target, build there. Simpler — always a full build+deploy:

```
./scripts/build.sh                     # sync source, build on target, install
./scripts/build.sh config              # deploy configs only (no build, no source sync)

Flags:
  --no-restart     skip service stop/start cycle
  --clean          cargo clean on target before building
  --dry-run        VPS preflight checks only
```

## Files to Change

| File | Action |
|------|--------|
| `scripts/deploy.sh` | Replace with `scripts/ship.sh` |
| `scripts/deploy-build-in-place.sh` | Replace with `scripts/build.sh` |
| `docs/deployment.md` | Update all references and examples |
| `scripts/nginx/README.md` | Update script name reference |

## Implementation Notes

### ship.sh structure
```bash
case "$SUBCOMMAND" in
  scripts)  stage_ctl_script; rsync_to_vps; install_ctl_script ;;
  config)   stage_configs; rsync_to_vps; install_configs; [restart] ;;
  binaries) [build]; stage_binaries; abi_check; rsync_to_vps; install_binaries; [restart] ;;
  all)      [build]; stage_all; abi_check; rsync_to_vps; install_all; [restart] ;;
esac
```

- `scripts` subcommand: never restarts, no build, no ABI check
- `config` subcommand: restarts by default; `--no-restart` skips it
- `binaries`/`all`: always ABI-check; `--no-restart` skips restart
- Shared helpers: `require_cmd`, `check_nix_abi`, staging and install functions
- Download of bitcoin-core and sv2-tp binaries only happens for `binaries`/`all`

### build.sh structure
```bash
case "$SUBCOMMAND" in
  config) rsync_configs; ssh install_configs; [restart] ;;
  all|"") rsync_source; ssh build_and_install; [restart] ;;
esac
```

- `config` subcommand skips source sync and build entirely
- Default (no subcommand) = full build+deploy
- `--no-restart` flag suppresses service stop/start

### Shared behavior to preserve
- `set -euo pipefail`
- ABI check on all shipped binaries (Nix interp rejection)
- bitcoin-core + sv2-tp download with cache check (`[ ! -f /tmp/bitcoin ]`)
- Staged deploy via `/tmp/hashpool-deploy-$$` then rsync
- Service stop → sleep → pkill → install → daemon-reload → start sequence
- nginx test + reload
- certbot SSL file fallback logic

## Verification
1. `./scripts/ship.sh --dry-run` — preflight passes without deploying
2. `./scripts/ship.sh scripts` — only `hashpool-ctl.sh` lands on VPS, no restart
3. `./scripts/ship.sh config --no-restart` — configs update, services keep running
4. `./scripts/ship.sh all --skip-build` — ships existing binaries, no cargo invoked
5. `./scripts/build.sh --dry-run` — VPS preflight passes
6. `./scripts/build.sh config` — configs only, no source sync or build
7. `./scripts/build.sh --clean` — full build+deploy with clean
