#!/usr/bin/env bash
set -euo pipefail

# Hashpool build-in-place deployment script
# Ships source to VPS and builds there

VPS_HOST="80.71.235.186"
VPS_USER="root"
VPS_DIR="/opt/hashpool"
REMOTE_SRC="/tmp/hashpool-src"
LOCAL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SUBCOMMAND=""
NO_RESTART=0
CLEAN_BUILD=0
DRY_RUN=0

usage() {
  cat << 'USAGE'
Usage: ./scripts/build.sh [config] [flags]

Subcommands:
  (default)  Sync source to VPS, build there, and install
  config     Deploy configs only (no source sync, no build)

Flags:
  --no-restart     Skip service stop/start cycle
  --clean          Cargo clean on VPS before building (default subcommand only)
  --dry-run        VPS preflight checks only (default subcommand only)
USAGE
}

# Parse optional subcommand first
if [ $# -ge 1 ]; then
  case "$1" in
    config)
      SUBCOMMAND="config"
      shift
      ;;
    --dry-run|--no-restart|--clean|-h|--help)
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

echo "Starting hashpool build-in-place deployment..."

case "${SUBCOMMAND:-}" in
  config)
    # Stage configs locally, rsync to VPS, install
    STAGING_DIR="/tmp/hashpool-deploy-$$"
    mkdir -p "$STAGING_DIR/config"
    mkdir -p "$STAGING_DIR/systemd"
    mkdir -p "$STAGING_DIR/nginx"

    cp -r "$LOCAL_DIR/config/prod"/* "$STAGING_DIR/config/"
    cp "$LOCAL_DIR/config/sv2-tp.conf" "$STAGING_DIR/config/"
    cp "$LOCAL_DIR/config/prometheus-pool.yml" "$STAGING_DIR/config/"
    cp "$LOCAL_DIR/config/prometheus-proxy.yml" "$STAGING_DIR/config/"
    cp "$LOCAL_DIR/scripts/systemd/"*.service "$STAGING_DIR/systemd/"
    cp -r "$LOCAL_DIR/scripts/nginx/sites-available" "$STAGING_DIR/nginx/"

    echo "Syncing configs to VPS..."
    rsync -avz --progress --partial --bwlimit=5000 --compress-level=9 --timeout=300 \
      "$STAGING_DIR/" "$VPS_USER@$VPS_HOST:/tmp/hashpool-deploy/"
    rm -rf "$STAGING_DIR"

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

  "")
    # Default: sync source, build on VPS, install
    if [ "$DRY_RUN" -eq 0 ]; then
      echo "Syncing source to VPS..."
      rsync -avz --progress --partial --bwlimit=5000 --compress-level=9 --timeout=300 \
        --exclude .git \
        --exclude .devenv \
        --exclude .direnv \
        --exclude .github \
        --exclude .idea \
        --exclude .vscode \
        --include 'test/' \
        --include 'test/integration-tests/' \
        --include 'test/integration-tests/***' \
        --exclude 'test/**' \
        --exclude target \
        --exclude result \
        --exclude logs \
        --exclude docs \
        --exclude examples \
        --exclude benches \
        --exclude nix \
        --exclude '**/node_modules' \
        --exclude '**/.pytest_cache' \
        --exclude '**/.mypy_cache' \
        "$LOCAL_DIR/" "$VPS_USER@$VPS_HOST:$REMOTE_SRC/"
    else
      echo "Dry run: skipping rsync and build/install steps."
    fi

    ssh "$VPS_USER@$VPS_HOST" "DRY_RUN=$DRY_RUN CLEAN_BUILD=$CLEAN_BUILD NO_RESTART=$NO_RESTART bash -s" << 'REMOTE'
      set -euo pipefail

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

      echo "Checking required tools on VPS..."
      require_cmd cargo "Install Rust (https://rustup.rs) or: apt-get install -y cargo"
      require_cmd rustc "Install Rust (https://rustup.rs) or: apt-get install -y rustc"
      require_cmd gcc "apt-get install -y build-essential"
      require_cmd make "apt-get install -y build-essential"
      require_cmd pkg-config "apt-get install -y pkg-config"
      require_cmd curl "apt-get install -y curl"
      require_cmd tar "apt-get install -y tar"
      require_cmd openssl "apt-get install -y openssl"
      require_cmd protoc "apt-get install -y protobuf-compiler"
      if ! command -v readelf >/dev/null 2>&1 && ! command -v strings >/dev/null 2>&1; then
        echo "Missing required tool: readelf or strings (for ABI check)"
        echo "Install hint: apt-get install -y binutils"
        exit 1
      fi

      if [ "${DRY_RUN:-0}" -eq 1 ]; then
        echo "VPS preflight checks passed (dry run)."
        exit 0
      fi

      echo "Downloading bitcoin-core and sv2-tp on VPS..."
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

      echo "Building debug binaries on VPS..."
      cd /tmp/hashpool-src/roles
      if [ "${CLEAN_BUILD:-0}" -eq 1 ]; then
        echo "Cleaning build artifacts on VPS..."
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

      # Stage
      rm -rf /tmp/hashpool-deploy
      mkdir -p /tmp/hashpool-deploy/{bin,libexec,config,systemd,nginx}

      cp /tmp/hashpool-src/roles/target/debug/pool_sv2 /tmp/hashpool-deploy/bin/
      cp /tmp/hashpool-src/roles/target/debug/translator_sv2 /tmp/hashpool-deploy/bin/
      cp /tmp/hashpool-src/roles/target/debug/mint /tmp/hashpool-deploy/bin/
      cp /tmp/hashpool-src/roles/target/debug/jd_server /tmp/hashpool-deploy/bin/
      cp /tmp/hashpool-src/roles/target/debug/jd_client_sv2 /tmp/hashpool-deploy/bin/
      cp /tmp/hashpool-src/roles/target/debug/web_pool /tmp/hashpool-deploy/bin/
      cp /tmp/hashpool-src/roles/target/debug/web_proxy /tmp/hashpool-deploy/bin/
      cp /tmp/bitcoin /tmp/hashpool-deploy/bin/
      cp /tmp/bitcoin-cli /tmp/hashpool-deploy/bin/
      cp /tmp/bitcoin-node /tmp/hashpool-deploy/libexec/
      cp /tmp/sv2-tp /tmp/hashpool-deploy/bin/

      cp -r /tmp/hashpool-src/config/prod/* /tmp/hashpool-deploy/config/
      cp /tmp/hashpool-src/config/sv2-tp.conf /tmp/hashpool-deploy/config/
      cp /tmp/hashpool-src/config/prometheus-pool.yml /tmp/hashpool-deploy/config/
      cp /tmp/hashpool-src/config/prometheus-proxy.yml /tmp/hashpool-deploy/config/
      cp /tmp/hashpool-src/scripts/systemd/*.service /tmp/hashpool-deploy/systemd/
      cp /tmp/hashpool-src/scripts/hashpool-ctl.sh /tmp/hashpool-deploy/bin/
      cp -r /tmp/hashpool-src/scripts/nginx/sites-available /tmp/hashpool-deploy/nginx/

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

      while IFS= read -r -d '' bin; do
        check_nix_abi "$bin"
      done < <(find /tmp/hashpool-deploy -type f -perm -111 -print0)

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
esac

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
