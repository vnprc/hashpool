# Hashpool Deployment Scripts

This directory contains scripts for deploying and managing Hashpool on a VPS.

## Scripts

### `deploy.sh` 
Complete VPS deployment script that:
- Installs build dependencies (Rust, system packages)
- Creates hashpool user and directories
- Builds all binaries from source
- Installs configuration files
- Creates and enables systemd services

**Usage:**
```bash
# On your VPS (Ubuntu/Debian)
git clone <hashpool-repo>
cd hashpool
./scripts/deploy.sh
```

### `health-check.sh`
Health monitoring script that checks:
- Systemd service status
- Port connectivity 
- Database file existence
- Provides log viewing commands

**Usage:**
```bash
./scripts/health-check.sh
```

### `validate.sh`
Development script to validate deployment scripts for:
- Package name consistency
- Config file references
- Port number accuracy
- Binary name matching

**Usage:**
```bash
./scripts/validate.sh
```

## Services Created

The deployment creates these systemd services:
- `hashpool-mint` - Cashu eCash mint (port 3338)
- `hashpool-pool` - Stratum V2 pool (port 34254) 
- `hashpool-translator` - SV1 to SV2 proxy (port 34255)
- `hashpool-jd-server` - Job Declaration server (port 34264)
- `hashpool-jd-client` - Job Declaration client (port 34265)

## Post-Deployment

After running `deploy.sh`, start the services:
```bash
sudo systemctl start hashpool-mint
sudo systemctl start hashpool-pool  
sudo systemctl start hashpool-translator
sudo systemctl start hashpool-jd-server
sudo systemctl start hashpool-jd-client
```

Monitor with:
```bash
./scripts/health-check.sh
sudo journalctl -u hashpool-mint -f
```