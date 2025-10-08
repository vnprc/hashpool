# Nginx Configuration Files

This directory contains nginx site configuration files for the hashpool testnet deployment.

## Files

- `sites-available/pool.hashpool.dev` - Pool dashboard (web-pool → 127.0.0.1:8080)
- `sites-available/proxy.hashpool.dev` - Miner dashboard (web-proxy → 127.0.0.1:3000)
- `sites-available/mint.hashpool.dev` - Mint HTTP API (mint → 127.0.0.1:3338)

## Deployment

These files are automatically deployed by `scripts/deploy.sh` to `/etc/nginx/sites-available/` on the VPS.

The deployment script creates symlinks in `/etc/nginx/sites-enabled/` and reloads nginx.

## SSL Certificates

SSL certificates are managed by certbot and already exist on the VPS:
- `/etc/letsencrypt/live/pool.hashpool.dev/`
- `/etc/letsencrypt/live/mint.hashpool.dev/`

The proxy.hashpool.dev subdomain shares the pool.hashpool.dev certificate (wildcard or SAN).

## Manual Updates

To manually update nginx configs on the VPS:

```bash
# Copy config
sudo cp pool.hashpool.dev /etc/nginx/sites-available/

# Test config
sudo nginx -t

# Reload nginx
sudo systemctl reload nginx
```

## Port Mappings

| Domain | Port | Service |
|--------|------|---------|
| pool.hashpool.dev | 8080 | web-pool dashboard |
| proxy.hashpool.dev | 3000 | web-proxy dashboard |
| mint.hashpool.dev | 3338 | mint HTTP API |
