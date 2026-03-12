#!/usr/bin/env bash
set -euo pipefail

# Hashpool service control script
# Usage: hashpool-ctl.sh {start|stop|restart|status}

SERVICES=(
  "hashpool-bitcoin-node"
  "hashpool-sv2-tp"
  "hashpool-prometheus-pool"
  "hashpool-prometheus-proxy"
  "hashpool-mint"
  "hashpool-pool"
  "hashpool-jd-server"
  "hashpool-jd-client"
  "hashpool-proxy"
  "hashpool-web-pool"
  "hashpool-web-proxy"
)

start_services() {
  echo "🚀 Starting hashpool services..."
  for service in "${SERVICES[@]}"; do
    echo "  Starting $service..."
    systemctl start "$service"
  done
  echo "✅ All services started"
}

stop_services() {
  echo "🛑 Stopping hashpool services..."
  # Stop in reverse order
  for ((i=${#SERVICES[@]}-1; i>=0; i--)); do
    service="${SERVICES[$i]}"
    echo "  Stopping $service..."
    systemctl stop "$service"
  done
  echo "✅ All services stopped"
}

restart_services() {
  stop_services
  sleep 2
  start_services
}

status_services() {
  echo "📊 Hashpool service status:"
  echo ""
  for service in "${SERVICES[@]}"; do
    if systemctl is-active --quiet "$service"; then
      status="✅ running"
    else
      status="❌ stopped"
    fi
    printf "  %-30s %s\n" "$service" "$status"
  done
  echo ""
  echo "For detailed logs: journalctl -u <service-name> -f"
}

logs_service() {
  if [ -z "${2:-}" ]; then
    echo "Usage: $0 logs <service-name|all>"
    echo ""
    echo "Available services:"
    for service in "${SERVICES[@]}"; do
      echo "  ${service#hashpool-}"
    done
    echo "  all"
    exit 1
  fi

  if [ "$2" = "all" ]; then
    echo "📋 Tailing all hashpool logs (Ctrl+C to exit)..."
    journalctl -u 'hashpool-*' -f
  else
    service_name="hashpool-$2"
    echo "📋 Tailing logs for $service_name (Ctrl+C to exit)..."
    journalctl -u "$service_name" -f
  fi
}

watch_logs() {
  # Check if tmux is installed
  if ! command -v tmux &> /dev/null; then
    echo "❌ tmux is not installed. Install with: apt install tmux"
    exit 1
  fi

  # Create new tmux session with first service
  tmux new-session -d -s hashpool-logs "journalctl -u ${SERVICES[0]} -f"
  tmux rename-window -t hashpool-logs:0 "${SERVICES[0]#hashpool-}"

  # Create a new window (tab) for each remaining service
  for ((i=1; i<${#SERVICES[@]}; i++)); do
    service="${SERVICES[$i]}"
    tmux new-window -t hashpool-logs -n "${service#hashpool-}" "journalctl -u $service -f"
  done

  # Select first window and attach
  tmux select-window -t hashpool-logs:0
  tmux attach-session -t hashpool-logs
}

clean_data() {
  if [ -z "${2:-}" ]; then
    echo "Usage: $0 clean <type>"
    echo ""
    echo "Available clean types:"
    echo "  cashu    - Delete all SQLite data (wallet and mint)"
    echo "  regtest  - Delete regtest blockchain data"
    echo "  testnet4 - Delete testnet4 blockchain data"
    echo "  stats    - Delete stats databases"
    echo "  logs     - Delete all service logs"
    exit 1
  fi

  DATADIR="/var/lib/hashpool"

  case "$2" in
    cashu)
      echo "🗑️  Deleting all SQLite data (wallet and mint)..."
      rm -f "$DATADIR/translator/wallet.sqlite" \
            "$DATADIR/translator/wallet.sqlite-shm" \
            "$DATADIR/translator/wallet.sqlite-wal" \
            "$DATADIR/mint/mint.sqlite" \
            "$DATADIR/mint/mint.sqlite-shm" \
            "$DATADIR/mint/mint.sqlite-wal"
      echo "✅ All SQLite data deleted"
      ;;
    regtest)
      echo "🗑️  Deleting regtest blockchain data..."
      rm -rf "$DATADIR/regtest"
      echo "✅ Regtest data deleted"
      ;;
    testnet4)
      echo "🗑️  Deleting testnet4 blockchain data..."
      rm -rf "$DATADIR/testnet4"
      echo "✅ Testnet4 data deleted"
      ;;
    logs)
      echo "🗑️  Deleting service logs..."
      rm -f /var/log/hashpool/*.log
      echo "✅ Service logs cleared"
      ;;
    *)
      echo "Error: Unknown clean type '$2'"
      echo "Valid types: cashu, regtest, testnet4, logs"
      exit 1
      ;;
  esac
}

case "${1:-}" in
  start)
    start_services
    ;;
  stop)
    stop_services
    ;;
  restart)
    restart_services
    ;;
  status)
    status_services
    ;;
  logs)
    logs_service "$@"
    ;;
  watch)
    watch_logs
    ;;
  clean)
    clean_data "$@"
    ;;
  *)
    echo "Usage: $0 {start|stop|restart|status|logs <service>|watch|clean <type>}"
    echo ""
    echo "Clean types: cashu, regtest, testnet4, logs"
    exit 1
    ;;
esac
