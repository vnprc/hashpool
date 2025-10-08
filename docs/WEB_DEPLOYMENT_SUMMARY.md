# Web Services Deployment - Implementation Summary

## Completed Tasks

All tasks from the deployment plan have been implemented successfully.

### 1. ✅ Created Nginx Configuration Files

**Location:** `scripts/nginx/sites-available/`

- `pool.hashpool.dev` - Routes HTTPS traffic to web-pool on 127.0.0.1:8080
- `proxy.hashpool.dev` - Routes HTTPS traffic to web-proxy on 127.0.0.1:3000
- `mint.hashpool.dev` - Routes HTTPS traffic to mint on 127.0.0.1:3338
- `README.md` - Documentation for nginx configuration

### 2. ✅ Created Systemd Service Files

**Location:** `scripts/systemd/`

- `hashpool-web-pool.service` - Manages web-pool dashboard service
- `hashpool-web-proxy.service` - Manages web-proxy dashboard service

Both services:
- Depend on their respective stats services (stats-pool/stats-proxy)
- Run as `hashpool:hashpool` user
- Auto-restart on failure
- Log to systemd journal

### 3. ✅ Updated Deployment Script

**File:** `scripts/deploy.sh`

Changes made:
- Added `web_pool` and `web_proxy` to cargo build command
- Added web binaries to staging and deployment
- Created nginx staging directory
- Added nginx config deployment section
- Added nginx config symlink creation
- Added nginx test and reload
- Updated service stop order (web services first)
- Updated service start order (web services last)
- Updated systemctl enable documentation

### 4. ✅ Updated Service Control Script

**File:** `scripts/hashpool-ctl.sh`

Changes made:
- Added `hashpool-web-pool` to SERVICES array
- Added `hashpool-web-proxy` to SERVICES array

The services are added at the end of the array, ensuring they start after their dependencies.

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│ Internet (Port 80/443)                                       │
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ Nginx Reverse Proxy                                          │
│  ├─ mint.hashpool.dev   → 127.0.0.1:3338 (mint HTTP)       │
│  ├─ pool.hashpool.dev   → 127.0.0.1:8080 (web-pool) ✨NEW  │
│  └─ proxy.hashpool.dev  → 127.0.0.1:3000 (web-proxy) ✨NEW │
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ Web Dashboard Layer (NEW)                                    │
│  ├─ web-pool:8080   polls → stats-pool:8081                │
│  └─ web-proxy:3000  polls → stats-proxy:3030               │
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ Stats API Layer                                              │
│  ├─ stats-pool:8081  ← Pool sends snapshots (TCP:4000)     │
│  └─ stats-proxy:3030 ← Translator sends snapshots(TCP:4001)│
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ Core Services                                                │
│  ├─ Pool:34254 (SV2 mining protocol)                       │
│  ├─ Translator:34255 (SV1/SV2 bridge)                      │
│  └─ Mint:3338 (Cashu mint HTTP API)                        │
└─────────────────────────────────────────────────────────────┘
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
9. **`hashpool-web-pool`** - Pool dashboard ✨NEW
10. **`hashpool-web-proxy`** - Proxy dashboard ✨NEW

## Files Created

```
scripts/
├── nginx/
│   ├── README.md
│   └── sites-available/
│       ├── mint.hashpool.dev
│       ├── pool.hashpool.dev
│       └── proxy.hashpool.dev
└── systemd/
    ├── hashpool-web-pool.service
    └── hashpool-web-proxy.service

docs/
├── WEB_DEPLOYMENT_PLAN.md
└── WEB_DEPLOYMENT_SUMMARY.md
```

## Files Modified

```
scripts/
├── deploy.sh (added web binaries, nginx deployment, service order)
└── hashpool-ctl.sh (added web services to array)
```

## Deployment Instructions

### To Deploy

From your local machine in the hashpool repository root:

```bash
cd /home/evan/work/hashpool
./scripts/deploy.sh
```

The script will:
1. Build all binaries including web_pool and web_proxy
2. Stage all files locally
3. rsync to VPS
4. Stop all services
5. Deploy binaries, configs, systemd services
6. Deploy and activate nginx configs
7. Test and reload nginx
8. Reload systemd
9. Start all services in order

### Post-Deployment Verification

On the VPS:

```bash
# Check all services are running
sudo hashpool-ctl status

# Check web services specifically
systemctl status hashpool-web-pool
systemctl status hashpool-web-proxy

# Test local endpoints
curl http://127.0.0.1:8080/
curl http://127.0.0.1:3000/

# Check nginx config
sudo nginx -t

# View web service logs
journalctl -u hashpool-web-pool -f
journalctl -u hashpool-web-proxy -f
```

### Public Endpoints

After deployment, these URLs will serve the web dashboards:

- **Pool Dashboard:** https://pool.hashpool.dev (was showing JSON API, now shows HTML dashboard)
- **Miner Dashboard:** https://proxy.hashpool.dev (was showing JSON API, now shows HTML dashboard)
- **Mint API:** https://mint.hashpool.dev (unchanged)

## What Changed

### Before

- `pool.hashpool.dev` → stats-pool JSON API (port 8081)
- `proxy.hashpool.dev` → stats-proxy JSON API (port 3030)
- No web dashboards deployed
- nginx misconfigured (pointing to stats services instead of web services)

### After

- `pool.hashpool.dev` → web-pool HTML dashboard (port 8080)
- `proxy.hashpool.dev` → web-proxy HTML dashboard (port 3000)
- Web services deployed and managed by systemd
- nginx correctly configured to proxy to web services
- Web services poll stats services internally

## Key Benefits

1. **Separation of concerns:** Stats APIs remain on internal ports, web dashboards on separate ports
2. **Public-facing dashboards:** Users can view pool/miner stats via web browser
3. **Proper architecture:** Follows the 4-tier design (Core → Stats → Web → Nginx)
4. **No mining impact:** Web services are purely additive, don't affect mining operations
5. **Independent restarts:** Web services can restart without affecting core services

## Rollback Plan

If issues occur after deployment:

```bash
# Stop web services
systemctl stop hashpool-web-pool hashpool-web-proxy

# Revert nginx configs (optional)
# Old configs are preserved in /etc/nginx/sites-available/*.bak

# Core services continue operating normally
```

## Next Steps

1. Deploy to VPS using `./scripts/deploy.sh`
2. Verify services are running
3. Test HTTPS endpoints
4. Monitor logs for any issues
5. Enable services at boot if desired:
   ```bash
   sudo systemctl enable hashpool-web-pool hashpool-web-proxy
   ```

## Notes

- SSL certificates already exist (managed by certbot)
- No changes required to core mining services
- Web services are stateless and can be restarted anytime
- Stats services were already deployed and remain unchanged
- This deployment fixes the nginx misconfiguration that was routing public traffic directly to internal APIs
