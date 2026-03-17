#!/usr/bin/env bash
set -euo pipefail

# Hashpool ship deployment script
# Builds locally and ships prebuilt artifacts to VPS

VPS_HOST="80.71.235.186"
VPS_USER="root"
VPS_DIR="/opt/hashpool"
LOCAL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SUBCOMMAND=""
SKIP_BUILD=0
NO_RESTART=0
CLEAN_BUILD=0
DRY_RUN=0

usage() {
  cat << 'USAGE'
Usage: ./scripts/ship.sh <subcommand> [flags]

Subcommands:
  scripts    Rsync hashpool-ctl.sh only, no restart
  config     Deploy configs + systemd + nginx, restart services
  binaries   Ship prebuilt binaries only, restart services
  all        Full deploy: build + ship everything

Flags (where applicable):
  --no-restart     Skip service stop/start cycle
  --skip-build     Use existing binaries in roles/target/debug (binaries/all only)
  --clean          Cargo clean before building (binaries/all only)
  --dry-run        Preflight checks only, no deploy
USAGE
}

# Parse optional subcommand first
if [ $# -ge 1 ]; then
  case "$1" in
    scripts|config|binaries|all)
      SUBCOMMAND="$1"
      shift
      ;;
    --dry-run|--no-restart|--skip-build|--clean|-h|--help)
      # These are flags, not a subcommand — handled below
      ;;
    *)
      echo "Unknown subcommand or flag: $1"
      usage
      exit 1
      ;;
  esac
fi

while [ $# -gt 0 ]; do
  case "$1" in
    --no-restart)
      NO_RESTART=1
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --clean)
      CLEAN_BUILD=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
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

# Validate flag combinations
if [ "$SKIP_BUILD" -eq 1 ] && [ "$CLEAN_BUILD" -eq 1 ]; then
  echo "Invalid flags: --skip-build cannot be combined with --clean."
  exit 1
fi

if [ "$SUBCOMMAND" = "scripts" ] || [ "$SUBCOMMAND" = "config" ]; then
  if [ "$SKIP_BUILD" -eq 1 ]; then
    echo "Invalid flags: --skip-build is only valid for binaries/all subcommands."
    exit 1
  fi
  if [ "$CLEAN_BUILD" -eq 1 ]; then
    echo "Invalid flags: --clean is only valid for binaries/all subcommands."
    exit 1
  fi
fi

# --- Shared helpers ---

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

REQUIRED_BINS=(
  "$LOCAL_DIR/roles/target/debug/pool_sv2"
  "$LOCAL_DIR/roles/target/debug/translator_sv2"
  "$LOCAL_DIR/roles/target/debug/mint"
  "$LOCAL_DIR/roles/target/debug/jd_server"
  "$LOCAL_DIR/roles/target/debug/jd_client_sv2"
  "$LOCAL_DIR/roles/target/debug/web_pool"
  "$LOCAL_DIR/roles/target/debug/web_proxy"
)

check_build_tools() {
  echo "Checking required tools locally..."
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
}

# --- dry-run: preflight checks only ---
if [ "$DRY_RUN" -eq 1 ]; then
  if [ "$SUBCOMMAND" = "binaries" ] || [ "$SUBCOMMAND" = "all" ] || [ -z "$SUBCOMMAND" ]; then
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
    else
      check_build_tools
    fi
  fi
  echo "Local preflight checks passed (dry run)."
  exit 0
fi

# --- Stage / build helpers ---

STAGING_DIR="/tmp/hashpool-deploy-$$"
mkdir -p "$STAGING_DIR"

stage_configs() {
  mkdir -p "$STAGING_DIR/config"
  mkdir -p "$STAGING_DIR/systemd"
  mkdir -p "$STAGING_DIR/nginx"
  mkdir -p "$STAGING_DIR/logrotate"
  cp -r "$LOCAL_DIR/config/prod"/* "$STAGING_DIR/config/"
  cp "$LOCAL_DIR/config/sv2-tp.conf" "$STAGING_DIR/config/"
  cp "$LOCAL_DIR/config/prometheus-pool.yml" "$STAGING_DIR/config/"
  cp "$LOCAL_DIR/config/prometheus-proxy.yml" "$STAGING_DIR/config/"
  cp "$LOCAL_DIR/scripts/systemd/"*.service "$STAGING_DIR/systemd/"
  cp -r "$LOCAL_DIR/scripts/nginx/sites-available" "$STAGING_DIR/nginx/"
  cp "$LOCAL_DIR/scripts/logrotate/hashpool" "$STAGING_DIR/logrotate/"
}

stage_binaries() {
  mkdir -p "$STAGING_DIR/bin"
  mkdir -p "$STAGING_DIR/libexec"
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
}

download_deps() {
  echo "Downloading bitcoin-core and sv2-tp..."
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
}

build_binaries() {
  check_build_tools
  echo "Building debug binaries locally..."
  cd "$LOCAL_DIR/roles"
  if [ "$CLEAN_BUILD" -eq 1 ]; then
    echo "Cleaning local build artifacts..."
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
  echo "Build complete"
}

verify_bins_exist() {
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
}

abi_check_staging() {
  while IFS= read -r -d '' bin; do
    check_nix_abi "$bin"
  done < <(find "$STAGING_DIR" -type f -perm -111 -print0)
}

rsync_to_vps() {
  echo "Syncing to VPS..."
  rsync -avz --progress --partial --bwlimit=5000 --compress-level=9 --timeout=300 \
    "$STAGING_DIR/" "$VPS_USER@$VPS_HOST:/tmp/hashpool-deploy/"
}

# --- Main dispatch ---

echo "Starting hashpool ship deployment..."

case "$SUBCOMMAND" in
  scripts)
    echo "Staging control script..."
    mkdir -p "$STAGING_DIR/bin"
    cp "$LOCAL_DIR/scripts/hashpool-ctl.sh" "$STAGING_DIR/bin/"
    rsync_to_vps
    ssh "$VPS_USER@$VPS_HOST" bash << 'REMOTE'
      mkdir -p /opt/hashpool/bin
      cp /tmp/hashpool-deploy/bin/hashpool-ctl.sh /opt/hashpool/bin/
      chown hashpool:hashpool /opt/hashpool/bin/hashpool-ctl.sh
      chmod +x /opt/hashpool/bin/hashpool-ctl.sh
      ln -sf /opt/hashpool/bin/hashpool-ctl.sh /usr/local/bin/hashpool-ctl
      rm -rf /tmp/hashpool-deploy
REMOTE
    ;;

  config)
    stage_configs
    rsync_to_vps
    ssh "$VPS_USER@$VPS_HOST" "NO_RESTART=$NO_RESTART bash -s" << 'REMOTE'
      set -euo pipefail

      if [ "${NO_RESTART:-0}" -eq 0 ]; then
        echo "Stopping services..."
        systemctl stop hashpool-web-proxy hashpool-web-pool hashpool-proxy hashpool-jd-client hashpool-jd-server hashpool-pool hashpool-mint hashpool-sv2-tp hashpool-bitcoin-node hashpool-prometheus-pool hashpool-prometheus-proxy 2>/dev/null || true
        sleep 3
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
      fi

      mkdir -p /opt/hashpool/{bin,libexec,config,config/shared}
      mkdir -p /var/lib/hashpool/{translator,mint,pool}
      mkdir -p /var/lib/hashpool/{prometheus-pool,prometheus-proxy}

      if ! command -v prometheus >/dev/null 2>&1; then
        apt-get update
        apt-get install -y prometheus
      fi

      cp -r /tmp/hashpool-deploy/config/* /opt/hashpool/config/
      cp /tmp/hashpool-deploy/systemd/*.service /etc/systemd/system/
      cp /tmp/hashpool-deploy/logrotate/hashpool /etc/logrotate.d/hashpool

      echo "Deploying nginx configs..."
      cp -r /tmp/hashpool-deploy/nginx/sites-available/* /etc/nginx/sites-available/

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
          openssl dhparam -out /etc/letsencrypt/ssl-dhparams.pem 2048
        fi
      fi

      ln -sf /etc/nginx/sites-available/pool.hashpool.dev /etc/nginx/sites-enabled/pool.hashpool.dev
      ln -sf /etc/nginx/sites-available/proxy.hashpool.dev /etc/nginx/sites-enabled/proxy.hashpool.dev
      ln -sf /etc/nginx/sites-available/mint.hashpool.dev /etc/nginx/sites-enabled/mint.hashpool.dev
      ln -sf /etc/nginx/sites-available/wallet.hashpool.dev /etc/nginx/sites-enabled/wallet.hashpool.dev

      nginx -t && systemctl reload nginx
      systemctl daemon-reload
      chown -R hashpool:hashpool /opt/hashpool

      if [ "${NO_RESTART:-0}" -eq 0 ]; then
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
      fi

      rm -rf /tmp/hashpool-deploy
REMOTE
    ;;

  binaries)
    download_deps
    if [ "$SKIP_BUILD" -eq 1 ]; then
      echo "Skipping local build (--skip-build)..."
      verify_bins_exist
    else
      build_binaries
    fi
    stage_binaries
    abi_check_staging
    rsync_to_vps
    ssh "$VPS_USER@$VPS_HOST" "NO_RESTART=$NO_RESTART bash -s" << 'REMOTE'
      set -euo pipefail

      if [ "${NO_RESTART:-0}" -eq 0 ]; then
        echo "Stopping services..."
        systemctl stop hashpool-web-proxy hashpool-web-pool hashpool-proxy hashpool-jd-client hashpool-jd-server hashpool-pool hashpool-mint hashpool-sv2-tp hashpool-bitcoin-node hashpool-prometheus-pool hashpool-prometheus-proxy 2>/dev/null || true
        sleep 3
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
      fi

      mkdir -p /opt/hashpool/{bin,libexec,config,config/shared}
      mkdir -p /var/lib/hashpool/{translator,mint,pool}
      mkdir -p /var/lib/hashpool/{prometheus-pool,prometheus-proxy}

      if ! command -v prometheus >/dev/null 2>&1; then
        apt-get update
        apt-get install -y prometheus
      fi

      cp -r /tmp/hashpool-deploy/bin/* /opt/hashpool/bin/
      cp -r /tmp/hashpool-deploy/libexec/* /opt/hashpool/libexec/

      systemctl daemon-reload
      chown -R hashpool:hashpool /opt/hashpool
      chmod +x /opt/hashpool/bin/*

      if [ "${NO_RESTART:-0}" -eq 0 ]; then
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
      fi

      rm -rf /tmp/hashpool-deploy
REMOTE
    ;;

  all)
    download_deps
    if [ "$SKIP_BUILD" -eq 1 ]; then
      echo "Skipping local build (--skip-build)..."
      verify_bins_exist
    else
      build_binaries
    fi
    stage_binaries
    stage_configs
    cp "$LOCAL_DIR/scripts/hashpool-ctl.sh" "$STAGING_DIR/bin/"
    abi_check_staging
    rsync_to_vps
    ssh "$VPS_USER@$VPS_HOST" "NO_RESTART=$NO_RESTART bash -s" << 'REMOTE'
      set -euo pipefail

      if [ "${NO_RESTART:-0}" -eq 0 ]; then
        echo "Stopping services..."
        systemctl stop hashpool-web-proxy hashpool-web-pool hashpool-proxy hashpool-jd-client hashpool-jd-server hashpool-pool hashpool-mint hashpool-sv2-tp hashpool-bitcoin-node hashpool-prometheus-pool hashpool-prometheus-proxy 2>/dev/null || true
        sleep 3
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
      fi

      mkdir -p /opt/hashpool/{bin,libexec,config,config/shared}
      mkdir -p /var/lib/hashpool/{translator,mint,pool}
      mkdir -p /var/lib/hashpool/{prometheus-pool,prometheus-proxy}

      if ! command -v prometheus >/dev/null 2>&1; then
        apt-get update
        apt-get install -y prometheus
      fi

      cp -r /tmp/hashpool-deploy/bin/* /opt/hashpool/bin/
      cp -r /tmp/hashpool-deploy/libexec/* /opt/hashpool/libexec/
      cp -r /tmp/hashpool-deploy/config/* /opt/hashpool/config/
      cp /tmp/hashpool-deploy/systemd/*.service /etc/systemd/system/
      cp /tmp/hashpool-deploy/logrotate/hashpool /etc/logrotate.d/hashpool

      echo "Deploying nginx configs..."
      cp -r /tmp/hashpool-deploy/nginx/sites-available/* /etc/nginx/sites-available/

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
          openssl dhparam -out /etc/letsencrypt/ssl-dhparams.pem 2048
        fi
      fi

      ln -sf /etc/nginx/sites-available/pool.hashpool.dev /etc/nginx/sites-enabled/pool.hashpool.dev
      ln -sf /etc/nginx/sites-available/proxy.hashpool.dev /etc/nginx/sites-enabled/proxy.hashpool.dev
      ln -sf /etc/nginx/sites-available/mint.hashpool.dev /etc/nginx/sites-enabled/mint.hashpool.dev
      ln -sf /etc/nginx/sites-available/wallet.hashpool.dev /etc/nginx/sites-enabled/wallet.hashpool.dev

      nginx -t && systemctl reload nginx
      systemctl daemon-reload
      chown -R hashpool:hashpool /opt/hashpool
      chmod +x /opt/hashpool/bin/*
      ln -sf /opt/hashpool/bin/hashpool-ctl.sh /usr/local/bin/hashpool-ctl

      if [ "${NO_RESTART:-0}" -eq 0 ]; then
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
      fi

      rm -rf /tmp/hashpool-deploy
REMOTE
    ;;

  "")
    echo "Error: subcommand required."
    usage
    exit 1
    ;;
esac

# Cleanup local staging
rm -rf "$STAGING_DIR"

echo "Deployment complete!"
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
