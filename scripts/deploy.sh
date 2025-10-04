#!/usr/bin/env bash
set -euo pipefail

# Hashpool VPS Deployment Script
# Builds binaries locally and deploys to production VPS

VPS_HOST="188.40.233.11"
VPS_USER="root"
VPS_DIR="/opt/hashpool"
LOCAL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "🚀 Starting hashpool deployment..."

# Download bitcoind if not present
echo "📥 Downloading bitcoind..."
BITCOIN_VERSION="sv2-tp-0.1.17"
BITCOIN_URL="https://github.com/Sjors/bitcoin/releases/download/${BITCOIN_VERSION}/bitcoin-${BITCOIN_VERSION}-x86_64-linux-gnu.tar.gz"
BITCOIN_DIR="/tmp/bitcoin-${BITCOIN_VERSION}"

if [ ! -f "/tmp/bitcoind" ]; then
  curl -L "$BITCOIN_URL" -o "/tmp/bitcoin.tar.gz"
  mkdir -p "$BITCOIN_DIR"
  tar -xzf "/tmp/bitcoin.tar.gz" -C "$BITCOIN_DIR" --strip-components=1
  cp "$BITCOIN_DIR/bin/bitcoind" /tmp/bitcoind
  cp "$BITCOIN_DIR/bin/bitcoin-cli" /tmp/bitcoin-cli
  rm -rf "$BITCOIN_DIR" /tmp/bitcoin.tar.gz
fi

# Build debug binaries locally (much faster than release)
echo "📦 Building debug binaries locally..."
cd "$LOCAL_DIR/roles"
cargo build \
  --bin pool_sv2 \
  --bin translator_sv2 \
  --bin mint \
  --bin jd_server \
  --bin jd_client \
  --bin stats_pool \
  --bin stats_proxy

echo "✅ Build complete"

# Copy all files to VPS in one rsync operation
echo "📤 Copying all files to VPS..."

# Create temporary staging directory
STAGING_DIR="/tmp/hashpool-deploy-$$"
mkdir -p "$STAGING_DIR/bin"
mkdir -p "$STAGING_DIR/config"
mkdir -p "$STAGING_DIR/systemd"

# Stage binaries
cp "$LOCAL_DIR/roles/target/debug/pool_sv2" "$STAGING_DIR/bin/"
cp "$LOCAL_DIR/roles/target/debug/translator_sv2" "$STAGING_DIR/bin/"
cp "$LOCAL_DIR/roles/target/debug/mint" "$STAGING_DIR/bin/"
cp "$LOCAL_DIR/roles/target/debug/jd_server" "$STAGING_DIR/bin/"
cp "$LOCAL_DIR/roles/target/debug/jd_client" "$STAGING_DIR/bin/"
cp "$LOCAL_DIR/roles/target/debug/stats_pool" "$STAGING_DIR/bin/"
cp "$LOCAL_DIR/roles/target/debug/stats_proxy" "$STAGING_DIR/bin/"
cp /tmp/bitcoind "$STAGING_DIR/bin/"
cp /tmp/bitcoin-cli "$STAGING_DIR/bin/"

# Stage configs
cp -r "$LOCAL_DIR/config/prod/"* "$STAGING_DIR/config/"

# Stage systemd services and control script
cp "$LOCAL_DIR/scripts/systemd/"*.service "$STAGING_DIR/systemd/"
cp "$LOCAL_DIR/scripts/hashpool-ctl.sh" "$STAGING_DIR/bin/"

# Single rsync to VPS and run installation commands
echo "📤 Deploying to VPS and installing..."
rsync -avz --progress "$STAGING_DIR/" "$VPS_USER@$VPS_HOST:/tmp/hashpool-deploy/" && \
ssh "$VPS_USER@$VPS_HOST" << 'EOF'
  # Stop all services first
  echo "Stopping services..."
  systemctl stop hashpool-proxy hashpool-jd-client hashpool-jd-server hashpool-pool hashpool-mint hashpool-stats-proxy hashpool-stats-pool hashpool-bitcoind 2>/dev/null || true

  # Create necessary directories
  mkdir -p /opt/hashpool/{bin,config}
  mkdir -p /opt/hashpool/.devenv/state/{translator,mint,stats-pool,stats-proxy,bitcoind}

  # Move files from staging to final location
  cp -r /tmp/hashpool-deploy/bin/* /opt/hashpool/bin/
  cp -r /tmp/hashpool-deploy/config/* /opt/hashpool/config/
  cp /tmp/hashpool-deploy/systemd/*.service /etc/systemd/system/

  # Reload systemd
  systemctl daemon-reload

  # Fix permissions
  chown -R hashpool:hashpool /opt/hashpool
  chmod +x /opt/hashpool/bin/*

  # Create symlink for easy access
  ln -sf /opt/hashpool/bin/hashpool-ctl.sh /usr/local/bin/hashpool-ctl

  # Start services back up
  echo "Starting services..."
  systemctl start hashpool-bitcoind
  sleep 2
  systemctl start hashpool-stats-pool hashpool-stats-proxy
  sleep 1
  systemctl start hashpool-mint
  sleep 1
  systemctl start hashpool-pool
  sleep 1
  systemctl start hashpool-jd-server hashpool-jd-client hashpool-proxy

  # Clean up staging
  rm -rf /tmp/hashpool-deploy
EOF

# Cleanup local staging
rm -rf "$STAGING_DIR"

echo "✅ Deployment complete!"
echo ""
echo "Service management (from any directory):"
echo "  sudo hashpool-ctl start     # Start all services"
echo "  sudo hashpool-ctl stop      # Stop all services"
echo "  sudo hashpool-ctl restart   # Restart all services"
echo "  sudo hashpool-ctl status    # Check service status"
echo ""
echo "To enable services at boot:"
echo "  sudo systemctl enable hashpool-{bitcoind,stats-pool,stats-proxy,mint,pool,jd-server,jd-client,proxy}"
echo ""
echo "Individual service management:"
echo "  sudo systemctl start hashpool-pool"
echo "  sudo systemctl status hashpool-pool"
echo "  sudo journalctl -u hashpool-pool -f"
