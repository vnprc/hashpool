# Hashpool Deployment Guide (Debian 12)

This guide is the canonical deployment path for Hashpool. It assumes a Debian 12 VPS, the repository deployment scripts, and an existing `hashpool` system user.

---

## Overview

Hashpool ships two deployment scripts for Debian 12:

**Note:** Each deployment now requires a local Prometheus or VictoriaMetrics instance scraping `/metrics` from the pool or translator. Web dashboards query that metrics store directly.

1. **`scripts/build.sh`** — ship source to the VPS and build there (recommended).
2. **`scripts/ship.sh`** — build locally, then rsync prebuilt artifacts to the VPS.

---

## Deployment Paths

### 1) Build-in-place (Debian 12, recommended)

Build and install directly on the VPS:

```bash
./scripts/build.sh
```

Why this is the canonical path:
- It guarantees a Debian 12 ABI build.
- Avoids shipping binaries compiled against a non-Debian ABI.

### 2) Ship prebuilt artifacts (local build + deploy)

Build locally and deploy to the VPS:

```bash
./scripts/ship.sh all
```

This builds local debug binaries from `roles/target/debug/*` and stages/configures services on the VPS. It performs an ABI safety check to reject Nix-built binaries.

---

## ship.sh Reference

`./scripts/ship.sh <subcommand> [flags]`

### Subcommands

- `scripts` — Rsync `hashpool-ctl.sh` only, no restart.
- `config` — Deploy configs + systemd + nginx, restart services.
- `binaries` — Ship prebuilt binaries only, restart services.
- `all` — Full deploy: build + ship everything.

### Flags

- `--no-restart` — Skip service stop/start cycle (`config`, `binaries`, `all`).
- `--skip-build` — Use existing binaries in `roles/target/debug` (`binaries`, `all` only).
- `--clean` — Cargo clean before building (`binaries`, `all` only).
- `--dry-run` — Preflight checks only; no deploy.
- `-h | --help` — Print usage.

---

## build.sh Reference

`./scripts/build.sh [config] [flags]`

### Subcommands

- *(default)* — Sync source to VPS, build there, and install.
- `config` — Deploy configs only (no source sync, no build).

### Flags

- `--no-restart` — Skip service stop/start cycle.
- `--clean` — Cargo clean on VPS before building (default subcommand only).
- `--dry-run` — VPS preflight checks only (default subcommand only).
- `-h | --help` — Print usage.

---

## What the Scripts Do (high-level)

- Stage binaries, configs, systemd services, and nginx configs.
- Upload a deployment bundle to the VPS (or build there for `build.sh`).
- Stop services, install files, reload nginx/systemd, restart services.
- Ensure certbot SSL config files exist and symlink nginx sites.

---

## Cashu Wallet (cashu.me) on the VPS

Hashpool's nginx config expects the Cashu wallet SPA at:

```
/opt/cashu.me/dist/spa
```

### Install & build

```bash
# On the VPS
sudo apt-get update
sudo apt-get install -y git nodejs npm

sudo mkdir -p /opt/cashu.me
sudo chown -R hashpool:hashpool /opt/cashu.me

cd /opt/cashu.me

git clone https://github.com/vnprc/cashu.me .

npm install

# Build SPA (matches nginx root /opt/cashu.me/dist/spa)
./node_modules/.bin/quasar build
```

### Deploy static files

```bash
sudo systemctl reload nginx
```

The wallet should now be available at:

```
https://wallet.hashpool.dev
```

---

## Service Management

After deployment:

```bash
sudo hashpool-ctl start
sudo hashpool-ctl stop
sudo hashpool-ctl restart
sudo hashpool-ctl status
```

Enable services at boot:

```bash
sudo systemctl enable hashpool-{bitcoin-node,sv2-tp,mint,pool,jd-server,jd-client,proxy,web-pool,web-proxy}
```

---

## Nginx Notes

Nginx sites are deployed automatically by the deployment scripts. The wallet host config expects an SPA root at `/opt/cashu.me/dist/spa`.
