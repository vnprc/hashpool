#!/usr/bin/env bash
set -euo pipefail

# Hashpool service control script
# Usage: hashpool-ctl.sh {start|stop|restart|status}

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
  *)
    echo "Usage: $0 {start|stop|restart|status|logs <service>|watch}"
    exit 1
    ;;
esac
