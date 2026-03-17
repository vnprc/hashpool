#!/usr/bin/env bash
set -euo pipefail

# Hashpool VPS Deployment Script
# Builds binaries locally and deploys to production VPS

VPS_HOST="80.71.235.186"
VPS_USER="root"
VPS_DIR="/opt/hashpool"
LOCAL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SKIP_BUILD=0
BUILD_ONLY=0
BUILD_IN_PLACE=0
DRY_RUN=0
CLEAN_BUILD=0
CONFIG_ONLY=0

usage() {
  cat << 'USAGE'
Usage: ./scripts/deploy.sh [--skip-build] [--build-only] [--build-in-place] [--config-only] [--dry-run] [--clean]

Options:
  --skip-build       Skip local cargo build step and use existing binaries.
  --build-only       Build local binaries and exit without deploying.
  --build-in-place   Build and deploy on the VPS (single entrypoint).
  --config-only      Deploy configs/systemd/nginx only; skip builds/binaries.
                    Note: config-only still restarts services and may take ~15-30s.
  --dry-run          Run preflight checks only; do not deploy.
  --clean            Clean local build artifacts before building.
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --build-only)
      BUILD_ONLY=1
      shift
      ;;
    --build-in-place)
      BUILD_IN_PLACE=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --config-only)
      CONFIG_ONLY=1
      shift
      ;;
    --clean)
      CLEAN_BUILD=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1"
      usage
      exit 1
      ;;
  esac
done

echo "🚀 Starting hashpool deployment..."

if [ "$BUILD_IN_PLACE" -eq 1 ]; then
  if [ "$SKIP_BUILD" -eq 1 ] || [ "$BUILD_ONLY" -eq 1 ]; then
    echo "Invalid flags: --build-in-place cannot be combined with --skip-build or --build-only."
    exit 1
  fi
  if [ "$CONFIG_ONLY" -eq 1 ]; then
    echo "Invalid flags: --build-in-place cannot be combined with --config-only."
    exit 1
  fi
  if [ "$DRY_RUN" -eq 1 ]; then
    if [ "$CLEAN_BUILD" -eq 1 ]; then
      "$LOCAL_DIR/scripts/deploy-build-in-place.sh" --dry-run --clean
    else
      "$LOCAL_DIR/scripts/deploy-build-in-place.sh" --dry-run
    fi
  else
    if [ "$CLEAN_BUILD" -eq 1 ]; then
      "$LOCAL_DIR/scripts/deploy-build-in-place.sh" --clean
    else
      "$LOCAL_DIR/scripts/deploy-build-in-place.sh"
    fi
  fi
  exit 0
fi

if [ "$BUILD_ONLY" -eq 1 ] && [ "$SKIP_BUILD" -eq 1 ]; then
  echo "Invalid flags: --build-only cannot be combined with --skip-build."
  exit 1
fi
if [ "$CONFIG_ONLY" -eq 1 ] && [ "$BUILD_ONLY" -eq 1 ]; then
  echo "Invalid flags: --config-only cannot be combined with --build-only."
  exit 1
fi
if [ "$CONFIG_ONLY" -eq 1 ] && [ "$CLEAN_BUILD" -eq 1 ]; then
  echo "Invalid flags: --config-only cannot be combined with --clean."
  exit 1
fi
if [ "$CLEAN_BUILD" -eq 1 ] && [ "$SKIP_BUILD" -eq 1 ]; then
  echo "Invalid flags: --clean cannot be combined with --skip-build."
  exit 1
fi
if [ "$DRY_RUN" -eq 1 ] && [ "$BUILD_ONLY" -eq 1 ]; then
  echo "Invalid flags: --dry-run cannot be combined with --build-only."
  exit 1
fi
if [ "$DRY_RUN" -eq 1 ] && [ "$CLEAN_BUILD" -eq 1 ]; then
  echo "Invalid flags: --dry-run cannot be combined with --clean."
  exit 1
fi

require_cmd() {
  local cmd="$1"
  local hint="$2"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "Missing required tool: $cmd"
    if [ -n "$hint" ]; then
      echo "Install hint: $hint"
    fi
    exit 1
  fi
}

REQUIRED_BINS=(
  "$LOCAL_DIR/roles/target/debug/pool_sv2"
  "$LOCAL_DIR/roles/target/debug/translator_sv2"
  "$LOCAL_DIR/roles/target/debug/mint"
  "$LOCAL_DIR/roles/target/debug/jd_server"
  "$LOCAL_DIR/roles/target/debug/jd_client_sv2"
  "$LOCAL_DIR/roles/target/debug/web_pool"
  "$LOCAL_DIR/roles/target/debug/web_proxy"
)

if [ "$DRY_RUN" -eq 1 ]; then
  if [ "$SKIP_BUILD" -eq 1 ]; then
    MISSING=0
    for bin in "${REQUIRED_BINS[@]}"; do
      if [ ! -f "$bin" ]; then
        echo "Missing binary: $bin"
        MISSING=1
      fi
    done
    if [ "$MISSING" -ne 0 ]; then
      echo "One or more binaries are missing. Build first or disable --skip-build."
      exit 1
    fi
  elif [ "$CONFIG_ONLY" -eq 0 ]; then
    echo "🔎 Checking required tools locally..."
    require_cmd cargo "Install Rust (https://rustup.rs) or: apt-get install -y cargo"
    require_cmd rustc "Install Rust (https://rustup.rs) or: apt-get install -y rustc"
    require_cmd gcc "apt-get install -y build-essential"
    require_cmd make "apt-get install -y build-essential"
    require_cmd pkg-config "apt-get install -y pkg-config"
    if ! command -v readelf >/dev/null 2>&1 && ! command -v strings >/dev/null 2>&1; then
      echo "Missing required tool: readelf or strings (for ABI check)"
      echo "Install hint: apt-get install -y binutils"
      exit 1
    fi
  fi
  echo "✅ Local preflight checks passed (dry run)."
  exit 0
fi

if [ "$CONFIG_ONLY" -eq 0 ]; then
  # Download bitcoin-core (multiprocess) if not present
  echo "📥 Downloading bitcoin-core and sv2-tp..."
  BITCOIN_VERSION="30.2"
  BITCOIN_URL="https://bitcoincore.org/bin/bitcoin-core-${BITCOIN_VERSION}/bitcoin-${BITCOIN_VERSION}-x86_64-linux-gnu.tar.gz"
  BITCOIN_DIR="/tmp/bitcoin-${BITCOIN_VERSION}"

  SV2_TP_VERSION="1.0.6"
  SV2_TP_URL="https://github.com/stratum-mining/sv2-tp/releases/download/v${SV2_TP_VERSION}/sv2-tp-${SV2_TP_VERSION}-x86_64-linux-gnu.tar.gz"
  SV2_TP_DIR="/tmp/sv2-tp-${SV2_TP_VERSION}"

  if [ ! -f "/tmp/bitcoin" ] || [ ! -f "/tmp/bitcoin-cli" ] || [ ! -f "/tmp/bitcoin-node" ]; then
    curl -L "$BITCOIN_URL" -o "/tmp/bitcoin.tar.gz"
    mkdir -p "$BITCOIN_DIR"
    tar -xzf "/tmp/bitcoin.tar.gz" -C "$BITCOIN_DIR" --strip-components=1
    cp "$BITCOIN_DIR/bin/bitcoin" /tmp/bitcoin
    cp "$BITCOIN_DIR/bin/bitcoin-cli" /tmp/bitcoin-cli
    cp "$BITCOIN_DIR/libexec/bitcoin-node" /tmp/bitcoin-node
    rm -rf "$BITCOIN_DIR" /tmp/bitcoin.tar.gz
  fi

  if [ ! -f "/tmp/sv2-tp" ]; then
    curl -L "$SV2_TP_URL" -o "/tmp/sv2-tp.tar.gz"
    mkdir -p "$SV2_TP_DIR"
    tar -xzf "/tmp/sv2-tp.tar.gz" -C "$SV2_TP_DIR" --strip-components=1
    cp "$SV2_TP_DIR/bin/sv2-tp" /tmp/sv2-tp
    rm -rf "$SV2_TP_DIR" /tmp/sv2-tp.tar.gz
  fi
fi

if [ "$CONFIG_ONLY" -eq 0 ] && [ "$SKIP_BUILD" -eq 0 ]; then
  echo "🔎 Checking required tools locally..."
  require_cmd cargo "Install Rust (https://rustup.rs) or: apt-get install -y cargo"
  require_cmd rustc "Install Rust (https://rustup.rs) or: apt-get install -y rustc"
  require_cmd gcc "apt-get install -y build-essential"
  require_cmd make "apt-get install -y build-essential"
  require_cmd pkg-config "apt-get install -y pkg-config"
  if ! command -v readelf >/dev/null 2>&1 && ! command -v strings >/dev/null 2>&1; then
    echo "Missing required tool: readelf or strings (for ABI check)"
    echo "Install hint: apt-get install -y binutils"
    exit 1
  fi
  # Build debug binaries locally (much faster than release)
  echo "📦 Building debug binaries locally..."
  cd "$LOCAL_DIR/roles"
  if [ "$CLEAN_BUILD" -eq 1 ]; then
    echo "🧹 Cleaning local build artifacts..."
    cargo clean
  fi
  cargo build \
    --bin pool_sv2 \
    --bin translator_sv2 \
    --bin mint \
    --bin jd_server \
    --bin jd_client_sv2 \
    --bin web_pool \
    --bin web_proxy

  echo "✅ Build complete"
  if [ "$BUILD_ONLY" -eq 1 ]; then
    echo "✅ Build-only complete; skipping deploy."
    exit 0
  fi
elif [ "$CONFIG_ONLY" -eq 0 ]; then
  echo "⏭️  Skipping local build (ship-only deploy)..."
  MISSING=0
  for bin in "${REQUIRED_BINS[@]}"; do
    if [ ! -f "$bin" ]; then
      echo "Missing binary: $bin"
      MISSING=1
    fi
  done
  if [ "$MISSING" -ne 0 ]; then
    echo "One or more binaries are missing. Build first or disable --skip-build."
    exit 1
  fi
fi

# Copy all files to VPS in one rsync operation
echo "📤 Copying all files to VPS..."

# Create temporary staging directory
STAGING_DIR="/tmp/hashpool-deploy-$$"
mkdir -p "$STAGING_DIR/bin"
mkdir -p "$STAGING_DIR/libexec"
mkdir -p "$STAGING_DIR/config"
mkdir -p "$STAGING_DIR/systemd"
mkdir -p "$STAGING_DIR/nginx"

if [ "$CONFIG_ONLY" -eq 0 ]; then
  # Stage binaries
  cp "$LOCAL_DIR/roles/target/debug/pool_sv2" "$STAGING_DIR/bin/"
  cp "$LOCAL_DIR/roles/target/debug/translator_sv2" "$STAGING_DIR/bin/"
  cp "$LOCAL_DIR/roles/target/debug/mint" "$STAGING_DIR/bin/"
  cp "$LOCAL_DIR/roles/target/debug/jd_server" "$STAGING_DIR/bin/"
  cp "$LOCAL_DIR/roles/target/debug/jd_client_sv2" "$STAGING_DIR/bin/"
  cp "$LOCAL_DIR/roles/target/debug/web_pool" "$STAGING_DIR/bin/"
  cp "$LOCAL_DIR/roles/target/debug/web_proxy" "$STAGING_DIR/bin/"
  cp /tmp/bitcoin "$STAGING_DIR/bin/"
  cp /tmp/bitcoin-cli "$STAGING_DIR/bin/"
  cp /tmp/bitcoin-node "$STAGING_DIR/libexec/"
  cp /tmp/sv2-tp "$STAGING_DIR/bin/"
fi

# Stage configs - copy prod configs directly to config/ directory
cp -r "$LOCAL_DIR/config/prod"/* "$STAGING_DIR/config/"
cp "$LOCAL_DIR/config/sv2-tp.conf" "$STAGING_DIR/config/"
cp "$LOCAL_DIR/config/prometheus-pool.yml" "$STAGING_DIR/config/"
cp "$LOCAL_DIR/config/prometheus-proxy.yml" "$STAGING_DIR/config/"

# Stage systemd services and control script
cp "$LOCAL_DIR/scripts/systemd/"*.service "$STAGING_DIR/systemd/"
cp "$LOCAL_DIR/scripts/hashpool-ctl.sh" "$STAGING_DIR/bin/"

# Stage nginx configs
cp -r "$LOCAL_DIR/scripts/nginx/sites-available" "$STAGING_DIR/nginx/"

check_nix_abi() {
  local bin_path="$1"
  if command -v readelf >/dev/null 2>&1; then
    local interp
    interp="$(readelf -l "$bin_path" 2>/dev/null | awk '/interpreter/ {print $NF}' || true)"
    if echo "$interp" | grep -q "/nix/store"; then
      echo "Refusing to deploy Nix ABI binary: $bin_path (interpreter: $interp)"
      exit 1
    fi
  elif command -v strings >/dev/null 2>&1; then
    if strings -a "$bin_path" | grep -q "/nix/store/.*/ld-linux"; then
      echo "Refusing to deploy Nix ABI binary: $bin_path (strings check)"
      exit 1
    fi
  else
    echo "Cannot verify ABI for $bin_path (missing readelf/strings)."
    exit 1
  fi
}

if [ "$CONFIG_ONLY" -eq 0 ]; then
  # Safety guard against Nix ABI binaries
  while IFS= read -r -d '' bin; do
    check_nix_abi "$bin"
  done < <(find "$STAGING_DIR" -type f -perm -111 -print0)
fi

# Single rsync to VPS and run installation commands
echo "📤 Deploying to VPS and installing..."
# Use --partial to resume interrupted transfers, --bwlimit to avoid throttling (KB/s)
# Adjust --bwlimit value if needed: 5000 = 5MB/s, 10000 = 10MB/s
rsync -avz --progress --partial --bwlimit=5000 --compress-level=9 --timeout=300 "$STAGING_DIR/" "$VPS_USER@$VPS_HOST:/tmp/hashpool-deploy/" && \
ssh "$VPS_USER@$VPS_HOST" "CONFIG_ONLY=$CONFIG_ONLY bash -s" << 'EOF'
  # Stop all services first
  echo "Stopping services..."
  systemctl stop hashpool-web-proxy hashpool-web-pool hashpool-proxy hashpool-jd-client hashpool-jd-server hashpool-pool hashpool-mint hashpool-sv2-tp hashpool-bitcoin-node hashpool-prometheus-pool hashpool-prometheus-proxy 2>/dev/null || true

  # Wait for services to fully terminate
  echo "Waiting for services to fully terminate..."
  sleep 3

  # Kill any remaining processes if they didn't stop
  pkill -f pool_sv2 || true
  pkill -f translator_sv2 || true
  pkill -f mint || true
  pkill -f jd_server || true
  pkill -f jd_client_sv2 || true
  pkill -f web_pool || true
  pkill -f web_proxy || true
  pkill -f "prometheus.*prometheus-pool.yml" || true
  pkill -f "prometheus.*prometheus-proxy.yml" || true
  pkill -f "bitcoin -m node" || true
  pkill -f sv2-tp || true

  sleep 1

  # Create necessary directories
  mkdir -p /opt/hashpool/{bin,libexec,config,config/shared}
  mkdir -p /var/lib/hashpool/{translator,mint,pool}
  mkdir -p /var/lib/hashpool/{prometheus-pool,prometheus-proxy}

  if ! command -v prometheus >/dev/null 2>&1; then
    apt-get update
    apt-get install -y prometheus
  fi

  # Move files from staging to final location
  cp /tmp/hashpool-deploy/bin/hashpool-ctl.sh /opt/hashpool/bin/
  if [ "${CONFIG_ONLY:-0}" -eq 0 ]; then
    cp -r /tmp/hashpool-deploy/bin/* /opt/hashpool/bin/
    cp -r /tmp/hashpool-deploy/libexec/* /opt/hashpool/libexec/
  fi
  cp -r /tmp/hashpool-deploy/config/* /opt/hashpool/config/
  cp /tmp/hashpool-deploy/systemd/*.service /etc/systemd/system/

  # Deploy nginx configs
  echo "Deploying nginx configs..."
  cp -r /tmp/hashpool-deploy/nginx/sites-available/* /etc/nginx/sites-available/

  # Ensure certbot nginx SSL options exist (certonly doesn't always create them)
  if [ ! -f /etc/letsencrypt/options-ssl-nginx.conf ]; then
    mkdir -p /etc/letsencrypt
    if [ -f /usr/lib/python3/dist-packages/certbot_nginx/_internal/tls_configs/options-ssl-nginx.conf ]; then
      cp /usr/lib/python3/dist-packages/certbot_nginx/_internal/tls_configs/options-ssl-nginx.conf /etc/letsencrypt/options-ssl-nginx.conf
    fi
  fi
  if [ ! -f /etc/letsencrypt/ssl-dhparams.pem ]; then
    mkdir -p /etc/letsencrypt
    if [ -f /usr/lib/python3/dist-packages/certbot_nginx/_internal/tls_configs/ssl-dhparams.pem ]; then
      cp /usr/lib/python3/dist-packages/certbot_nginx/_internal/tls_configs/ssl-dhparams.pem /etc/letsencrypt/ssl-dhparams.pem
    else
      # Fallback: generate dhparams if certbot nginx assets are unavailable.
      openssl dhparam -out /etc/letsencrypt/ssl-dhparams.pem 2048
    fi
  fi

  # Ensure symlinks exist in sites-enabled
  ln -sf /etc/nginx/sites-available/pool.hashpool.dev /etc/nginx/sites-enabled/pool.hashpool.dev
  ln -sf /etc/nginx/sites-available/proxy.hashpool.dev /etc/nginx/sites-enabled/proxy.hashpool.dev
  ln -sf /etc/nginx/sites-available/mint.hashpool.dev /etc/nginx/sites-enabled/mint.hashpool.dev
  ln -sf /etc/nginx/sites-available/wallet.hashpool.dev /etc/nginx/sites-enabled/wallet.hashpool.dev

  # Test and reload nginx
  nginx -t && systemctl reload nginx

  # Reload systemd
  systemctl daemon-reload

  # Fix permissions
  chown -R hashpool:hashpool /opt/hashpool
  chmod +x /opt/hashpool/bin/hashpool-ctl.sh
  ln -sf /opt/hashpool/bin/hashpool-ctl.sh /usr/local/bin/hashpool-ctl
  if [ "${CONFIG_ONLY:-0}" -eq 0 ]; then
    chmod +x /opt/hashpool/bin/*
  fi

  # Start services back up
  echo "Starting services..."
  systemctl start hashpool-bitcoin-node
  sleep 2
  systemctl start hashpool-sv2-tp
  sleep 2
  systemctl start hashpool-prometheus-pool hashpool-prometheus-proxy
  sleep 1
  systemctl start hashpool-mint
  sleep 1
  systemctl start hashpool-pool
  sleep 1
  systemctl start hashpool-jd-server hashpool-jd-client hashpool-proxy
  sleep 1
  systemctl start hashpool-web-pool hashpool-web-proxy

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
echo "  sudo systemctl enable hashpool-{bitcoin-node,sv2-tp,prometheus-pool,prometheus-proxy,mint,pool,jd-server,jd-client,proxy,web-pool,web-proxy}"
echo ""
echo "Individual service management:"
echo "  sudo systemctl start hashpool-pool"
echo "  sudo systemctl status hashpool-pool"
echo "  sudo journalctl -u hashpool-pool -f"
