# Hashpool Quick Deployment Plan (2 Hour Demo)

## Goal
Deploy a working hashpool demo on VPS (188.40.233.11) that allows:
- ASICs to connect to stratum endpoint from anywhere
- View dashboards at pool.hashpool.dev and proxy.hashpool.dev
- Basic functionality, not production-ready

## Stack Components
1. **Pool** - SV2 mining pool (port 34255)
2. **Translator/Proxy** - SV1 to SV2 translator (port 34256 for SV1)
3. **Mint** - CDK mint for issuing ecash
4. **Dashboards** - Web UIs on ports 3000 (pool) and 3001 (proxy)

## Deployment Timeline (2 Hours)

### Phase 1: VPS Setup (15 mins) ✅ COMPLETE
```bash
# SSH to VPS
ssh root@188.40.233.11

# Update system
apt update && apt upgrade -y

# Install dependencies
apt install -y git build-essential curl nginx certbot python3-certbot-nginx

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env

# Clone repositories
cd /opt
git clone https://github.com/vnprc/hashpool.git
git clone https://github.com/vnprc/cdk.git
```

### Phase 2: Build & Configure (30 mins) ✅ COMPLETE
```bash
cd /opt/hashpool

# Build all components
cargo build --release --bin pool
cargo build --release --bin translator_sv2
cargo build --release --bin mint

# Create config directories
mkdir -p /etc/hashpool/config
mkdir -p /var/lib/hashpool/{pool,translator,mint}
mkdir -p /var/log/hashpool

# Copy and adjust configs
cp -r config/* /etc/hashpool/config/

# Edit pool config
cat > /etc/hashpool/config/pool.toml << 'EOF'
listen_address = "0.0.0.0:34255"
mint_address = "127.0.0.1:5050"
web_port = 3000
[database]
path = "/var/lib/hashpool/pool/pool.db"
EOF

# Edit translator config  
cat > /etc/hashpool/config/translator.toml << 'EOF'
upstream_address = "127.0.0.1:34255"
listen_address = "0.0.0.0:34256"
web_port = 3001
[wallet]
mint_url = "http://127.0.0.1:8000"
EOF
```

### Phase 3: Systemd Services (15 mins) ⏳ IN PROGRESS
```bash
# Create systemd service for mint
cat > /etc/systemd/system/hashpool-mint.service << 'EOF'
[Unit]
Description=Hashpool Mint
After=network.target

[Service]
Type=simple
User=root
WorkingDirectory=/opt/hashpool
ExecStart=/opt/hashpool/target/release/mint
Restart=always
Environment=CDK_MINT_DB_PATH=/var/lib/hashpool/mint/mint.sqlite

[Install]
WantedBy=multi-user.target
EOF

# Create systemd service for pool
cat > /etc/systemd/system/hashpool-pool.service << 'EOF'
[Unit]
Description=Hashpool Pool
After=network.target hashpool-mint.service

[Service]
Type=simple
User=root
WorkingDirectory=/opt/hashpool
ExecStart=/opt/hashpool/target/release/pool -c /etc/hashpool/config/pool.toml
Restart=always

[Install]
WantedBy=multi-user.target
EOF

# Create systemd service for translator
cat > /etc/systemd/system/hashpool-translator.service << 'EOF'
[Unit]
Description=Hashpool Translator
After=network.target hashpool-pool.service

[Service]
Type=simple
User=root
WorkingDirectory=/opt/hashpool
ExecStart=/opt/hashpool/target/release/translator_sv2 -c /etc/hashpool/config/translator.toml
Restart=always

[Install]
WantedBy=multi-user.target
EOF

# Enable and start services
systemctl daemon-reload
systemctl enable hashpool-mint hashpool-pool hashpool-translator
systemctl start hashpool-mint
sleep 5
systemctl start hashpool-pool  
sleep 5
systemctl start hashpool-translator
```

### Phase 4: Nginx & Domain Setup (30 mins) ✅ COMPLETE
```bash
# Configure nginx for pool dashboard
cat > /etc/nginx/sites-available/pool.hashpool.dev << 'EOF'
server {
    server_name pool.hashpool.dev;
    
    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;
    }
}
EOF

# Configure nginx for proxy dashboard
cat > /etc/nginx/sites-available/proxy.hashpool.dev << 'EOF'
server {
    server_name proxy.hashpool.dev;
    
    location / {
        proxy_pass http://127.0.0.1:3001;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;
    }
}
EOF

# Enable sites
ln -s /etc/nginx/sites-available/pool.hashpool.dev /etc/nginx/sites-enabled/
ln -s /etc/nginx/sites-available/proxy.hashpool.dev /etc/nginx/sites-enabled/
nginx -t && systemctl reload nginx

# Get SSL certificates (requires DNS already pointing to 188.40.233.11)
certbot --nginx -d pool.hashpool.dev -d proxy.hashpool.dev --non-interactive --agree-tos -m your-email@example.com
```

### Phase 5: Firewall & Testing (30 mins) ✅ COMPLETE
```bash
# Open required ports
ufw allow 22/tcp    # SSH
ufw allow 80/tcp    # HTTP
ufw allow 443/tcp   # HTTPS  
ufw allow 34256/tcp # Stratum (SV1)
ufw allow 34255/tcp # Pool (SV2) - optional, only if direct SV2
ufw --force enable

# Check services
systemctl status hashpool-mint
systemctl status hashpool-pool
systemctl status hashpool-translator

# View logs
journalctl -u hashpool-mint -f &
journalctl -u hashpool-pool -f &
journalctl -u hashpool-translator -f &

# Test endpoints
curl http://localhost:8000/info  # Mint
curl http://localhost:3000        # Pool dashboard
curl http://localhost:3001        # Proxy dashboard
```

## DNS Configuration (Do this FIRST!)
Add these DNS records at your registrar:
```
A record: pool.hashpool.dev -> 188.40.233.11
A record: proxy.hashpool.dev -> 188.40.233.11
```

## ASIC Configuration
Configure your ASIC miner with:
- **URL**: `stratum+tcp://188.40.233.11:34256`
- **Worker**: `your_worker_name`
- **Password**: (any, not used)

## Quick Validation Checklist
- [ ] DNS records propagated (check with `dig pool.hashpool.dev`)
- [ ] Services running (`systemctl status hashpool-*`)
- [ ] Dashboards accessible at https://pool.hashpool.dev and https://proxy.hashpool.dev
- [ ] Stratum port open (`nc -zv 188.40.233.11 34256`)
- [ ] ASIC can connect and submit shares
- [ ] Shares appear in dashboard

## Troubleshooting Commands
```bash
# Check logs
tail -f /var/log/hashpool/*.log
journalctl -u hashpool-pool -n 100

# Test connectivity
telnet 188.40.233.11 34256
curl -I https://pool.hashpool.dev

# Restart everything
systemctl restart hashpool-mint
systemctl restart hashpool-pool
systemctl restart hashpool-translator
```

## Security Note
This setup is for DEMO only. For production:
- Don't run as root
- Use proper secrets/keys
- Enable authentication
- Set up monitoring
- Configure backups
- Use environment-specific configs