# Hashpool Deployment Guide (Debian 12)

This guide is the canonical deployment path for Hashpool. It assumes a Debian 12 VPS, the repository deployment scripts, and an existing `hashpool` system user.

---

## Overview

Hashpool ships two deployment workflows for Debian 12:

1. **Build-in-place (recommended)** — build on the VPS and install from there.
2. **Ship-only** — build locally, then rsync binaries/configs to the VPS.

The primary entrypoint is `scripts/deploy.sh`, which can run either flow depending on flags.

---

## Deployment Paths

### 1) Build-in-place (Debian 12, recommended)

Build and install directly on the VPS:

```bash
./scripts/deploy.sh --build-in-place
```

Why this is the canonical path:
- It guarantees a Debian 12 ABI build.
- Avoids shipping binaries compiled against a non-Debian ABI.

### 2) Ship-only (local build + deploy)

Build locally and deploy to the VPS:

```bash
./scripts/deploy.sh
```

This uses local debug binaries from `roles/target/debug/*` and then stages/configures services on the VPS. It performs an ABI safety check to reject Nix-built binaries.

---

## deploy.sh Reference (all flags)

`./scripts/deploy.sh` accepts the following flags:

- `--build-in-place`
  - Build and deploy on the VPS using `scripts/deploy-build-in-place.sh`.
  - **Cannot be combined** with `--skip-build`, `--build-only`, or `--config-only`.
- `--skip-build`
  - Skip local build and deploy existing binaries.
  - **Cannot be combined** with `--build-only` or `--clean`.
- `--build-only`
  - Build local binaries and exit without deploying.
  - **Cannot be combined** with `--skip-build`, `--config-only`, or `--dry-run`.
- `--config-only`
  - Deploy configs/systemd/nginx only; skip builds/binaries.
  - Still **restarts services** and can take ~15–30s.
  - **Cannot be combined** with `--build-only`, `--clean`, or `--build-in-place`.
- `--dry-run`
  - Run preflight checks only; no deploy.
  - **Cannot be combined** with `--build-only` or `--clean`.
- `--clean`
  - Clean build artifacts before building.
  - **Cannot be combined** with `--skip-build`, `--config-only`, or `--dry-run`.
- `-h | --help`
  - Print usage.

---

## What deploy.sh Does (high-level)

- Stages binaries, configs, systemd services, and nginx configs.
- Uploads a single deployment bundle to the VPS.
- Stops services, installs files, reloads nginx/systemd, restarts services.
- Ensures certbot SSL config files exist and symlinks nginx sites.

---

## Cashu Wallet (cashu.me) on the VPS

Hashpool’s nginx config expects the Cashu wallet SPA at:

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
sudo systemctl enable hashpool-{bitcoin-node,sv2-tp,stats-pool,stats-proxy,mint,pool,jd-server,jd-client,proxy,web-pool,web-proxy}
```

---

## Nginx Notes

Nginx sites are deployed automatically by the deployment scripts. The wallet host config expects an SPA root at `/opt/cashu.me/dist/spa`.
