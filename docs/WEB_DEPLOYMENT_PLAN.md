# Web Services Deployment Plan

## Overview

This document describes the deployment plan for adding web-pool and web-proxy dashboard services to the hashpool production deployment.

## Current State

The testnet pool has nginx configured but is incorrectly proxying directly to stats services:
- `pool.hashpool.dev` → `127.0.0.1:8081` (stats-pool HTTP API) ❌
- `proxy.hashpool.dev` → `127.0.0.1:3030` (stats-proxy HTTP API) ❌

The web services (web-pool and web-proxy) are not being built or deployed.

## Target State

Web services will be deployed and nginx will proxy to them:
- `pool.hashpool.dev` → `127.0.0.1:8080` (web-pool dashboard) ✅
- `proxy.hashpool.dev` → `127.0.0.1:3000` (web-proxy dashboard) ✅

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Nginx (Port 80/443) - Public Internet                       │
│  ├─ mint.hashpool.dev → 127.0.0.1:3338 (mint HTTP API)     │
│  ├─ pool.hashpool.dev → 127.0.0.1:8080 (web-pool)          │
│  ├─ proxy.hashpool.dev → 127.0.0.1:3000 (web-proxy)        │
│  └─ wallet.hashpool.dev → static files                      │
└─────────────────────────────────────────────────────────────┘
                          ↓
    ┌─────────────────────────────────────────┐
    │  Web Services (Dashboard Layer)         │
    ├─ web-pool:8080  → stats-pool:8081      │
    └─ web-proxy:3000 → stats-proxy:3030     │
                          ↓
    ┌─────────────────────────────────────────┐
    │  Stats Services (API Layer)             │
    ├─ stats-pool:8081  ← Pool (TCP:4000)    │
    └─ stats-proxy:3030 ← Translator(TCP:4001)│
                          ↓
    ┌─────────────────────────────────────────┐
    │  Core Services                          │
    ├─ Pool:34254 (SV2 mining)               │
    ├─ Translator:34255 (SV1/SV2 bridge)     │
    └─ Mint:3338 (Cashu HTTP API)            │
```

## Implementation Tasks

### 1. Build Web Binaries
**File:** `scripts/deploy.sh`

Add `web_pool` and `web_proxy` to the cargo build command:
```bash
cargo build \
  --bin pool_sv2 \
  --bin translator_sv2 \
  --bin mint \
  --bin jd_server \
  --bin jd_client \
  --bin stats_pool \
  --bin stats_proxy \
  --bin web_pool \
  --bin web_proxy
```

### 2. Create Systemd Service Files

**File:** `scripts/systemd/hashpool-web-pool.service`
```ini
[Unit]
Description=Hashpool Pool Web Dashboard
After=network.target hashpool-stats-pool.service
Requires=hashpool-stats-pool.service

[Service]
Type=simple
User=hashpool
Group=hashpool
WorkingDirectory=/opt/hashpool
ExecStart=/opt/hashpool/bin/web_pool \
  --stats-pool-url http://127.0.0.1:8081 \
  --web-address 127.0.0.1:8080
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=hashpool-web-pool

[Install]
WantedBy=multi-user.target
```

**File:** `scripts/systemd/hashpool-web-proxy.service`
```ini
[Unit]
Description=Hashpool Proxy Web Dashboard
After=network.target hashpool-stats-proxy.service
Requires=hashpool-stats-proxy.service

[Service]
Type=simple
User=hashpool
Group=hashpool
WorkingDirectory=/opt/hashpool
ExecStart=/opt/hashpool/bin/web_proxy \
  --stats-proxy-url http://127.0.0.1:3030 \
  --web-address 127.0.0.1:3000 \
  --config /opt/hashpool/config/tproxy.config.toml \
  --shared-config /opt/hashpool/config/shared/miner.toml
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=hashpool-web-proxy

[Install]
WantedBy=multi-user.target
```

### 3. Create Nginx Configuration Files

**Directory:** `scripts/nginx/sites-available/`

**File:** `pool.hashpool.dev`
```nginx
server {
    server_name pool.hashpool.dev;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;
    }

    listen 443 ssl;
    ssl_certificate /etc/letsencrypt/live/pool.hashpool.dev/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/pool.hashpool.dev/privkey.pem;
    include /etc/letsencrypt/options-ssl-nginx.conf;
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;
}

server {
    if ($host = pool.hashpool.dev) {
        return 301 https://$host$request_uri;
    }

    listen 80;
    server_name pool.hashpool.dev;
    return 404;
}
```

**File:** `proxy.hashpool.dev`
```nginx
server {
    server_name proxy.hashpool.dev;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;
    }

    listen 443 ssl;
    ssl_certificate /etc/letsencrypt/live/pool.hashpool.dev/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/pool.hashpool.dev/privkey.pem;
    include /etc/letsencrypt/options-ssl-nginx.conf;
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;
}

server {
    if ($host = proxy.hashpool.dev) {
        return 301 https://$host$request_uri;
    }

    listen 80;
    server_name proxy.hashpool.dev;
    return 404;
}
```

**File:** `mint.hashpool.dev`
```nginx
server {
    listen 80;
    server_name mint.hashpool.dev;
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    server_name mint.hashpool.dev;

    ssl_certificate /etc/letsencrypt/live/mint.hashpool.dev/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/mint.hashpool.dev/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:3338;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

### 4. Update Deployment Script

**File:** `scripts/deploy.sh`

Add nginx directory to staging:
```bash
mkdir -p "$STAGING_DIR/nginx"
cp -r "$LOCAL_DIR/scripts/nginx/sites-available" "$STAGING_DIR/nginx/"
```

Add nginx deployment to SSH section:
```bash
# Deploy nginx configs
cp -r /tmp/hashpool-deploy/nginx/sites-available/* /etc/nginx/sites-available/
systemctl reload nginx
```

Update service stop/start lists to include web services.

### 5. Update Service Control Script

**File:** `scripts/hashpool-ctl.sh`

Update SERVICES array:
```bash
SERVICES=(
  "hashpool-bitcoind"
  "hashpool-stats-pool"
  "hashpool-stats-proxy"
  "hashpool-mint"
  "hashpool-pool"
  "hashpool-jd-server"
  "hashpool-jd-client"
  "hashpool-proxy"
  "hashpool-web-pool"
  "hashpool-web-proxy"
)
```

## Service Startup Order

1. `hashpool-bitcoind` - Bitcoin node
2. `hashpool-stats-pool` - Stats collection for pool
3. `hashpool-stats-proxy` - Stats collection for proxy
4. `hashpool-mint` - Cashu mint service
5. `hashpool-pool` - SV2 mining pool
6. `hashpool-jd-server` - Job declarator server
7. `hashpool-jd-client` - Job declarator client
8. `hashpool-proxy` - Translator/proxy service
9. `hashpool-web-pool` - Pool dashboard (NEW)
10. `hashpool-web-proxy` - Proxy dashboard (NEW)

## Port Allocations

| Service | Port | Protocol | Purpose |
|---------|------|----------|---------|
| Pool | 34254 | TCP (SV2) | Mining connections |
| Translator | 34255 | TCP (SV2) | Upstream pool connection |
| Mint | 3338 | HTTP | Cashu API |
| Faucet | 8083 | HTTP | Test token distribution |
| stats-pool TCP | 4000 | TCP | Pool → stats-pool |
| stats-pool HTTP | 8081 | HTTP | stats-pool API |
| stats-proxy TCP | 4001 | TCP | Translator → stats-proxy |
| stats-proxy HTTP | 3030 | HTTP | stats-proxy API |
| web-pool | 8080 | HTTP | Pool dashboard |
| web-proxy | 3000 | HTTP | Proxy dashboard |

## Deployment Procedure

1. Run `scripts/deploy.sh` from local machine
2. Script will:
   - Build all binaries including web services
   - Stage files locally
   - rsync to VPS
   - Stop all services
   - Deploy binaries, configs, systemd services, and nginx configs
   - Reload nginx
   - Start services in correct order

## Verification

After deployment:

1. Check service status:
   ```bash
   sudo hashpool-ctl status
   ```

2. Check web services are running:
   ```bash
   curl http://127.0.0.1:8080/
   curl http://127.0.0.1:3000/
   ```

3. Check public HTTPS endpoints:
   ```bash
   curl https://pool.hashpool.dev/
   curl https://proxy.hashpool.dev/
   ```

4. Check nginx configuration:
   ```bash
   sudo nginx -t
   ```

5. View logs:
   ```bash
   journalctl -u hashpool-web-pool -f
   journalctl -u hashpool-web-proxy -f
   ```

## Rollback Plan

If issues occur:

1. Stop web services:
   ```bash
   systemctl stop hashpool-web-pool hashpool-web-proxy
   ```

2. Restore old nginx configs (if needed):
   ```bash
   # Nginx configs are backed up by deploy script
   ```

3. Core services (pool, mint, translator) continue operating normally

## Notes

- Web services are purely additive - they don't affect core mining operations
- Stats services were already deployed and functional
- Nginx SSL certificates already exist (managed by certbot)
- No changes to mining protocols or core service behavior
- Web services can be restarted independently without affecting mining
