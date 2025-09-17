#!/bin/bash
# Service management script for Hashpool

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

SERVICES=("bitcoind-sv2" "hashpool-mint" "hashpool-pool" "hashpool-translator" "hashpool-jd-server" "hashpool-jd-client")

case "$1" in
    start)
        echo -e "${GREEN}Starting Hashpool services in dependency order...${NC}"
        for service in "${SERVICES[@]}"; do
            echo "Starting $service..."
            sudo systemctl start "$service"
            if systemctl is-active --quiet "$service"; then
                echo -e "✓ $service: ${GREEN}Started${NC}"
            else
                echo -e "✗ $service: ${RED}Failed to start${NC}"
                echo "Check logs with: sudo journalctl -u $service -n 20"
                exit 1
            fi
        done
        echo -e "\n${GREEN}All services started successfully!${NC}"
        ;;
        
    stop)
        echo -e "${YELLOW}Stopping Hashpool services...${NC}"
        # Stop in reverse order
        for ((i=${#SERVICES[@]}-1; i>=0; i--)); do
            service="${SERVICES[$i]}"
            echo "Stopping $service..."
            sudo systemctl stop "$service"
        done
        echo -e "${YELLOW}All services stopped${NC}"
        ;;
        
    restart)
        echo -e "${YELLOW}Restarting Hashpool services...${NC}"
        $0 stop
        sleep 2
        $0 start
        ;;
        
    status)
        echo "Hashpool Service Status:"
        echo "======================="
        for service in "${SERVICES[@]}"; do
            if systemctl is-active --quiet "$service"; then
                echo -e "$service: ${GREEN}Running${NC}"
            else
                echo -e "$service: ${RED}Not Running${NC}"
            fi
        done
        ;;
        
    logs)
        service="${2:-bitcoind-sv2}"
        if [[ " ${SERVICES[*]} " =~ " $service " ]]; then
            echo "Showing logs for $service (Ctrl+C to exit):"
            sudo journalctl -u "$service" -f
        else
            echo "Available services: ${SERVICES[*]}"
            exit 1
        fi
        ;;
        
    enable)
        echo -e "${GREEN}Enabling Hashpool services for auto-start...${NC}"
        for service in "${SERVICES[@]}"; do
            sudo systemctl enable "$service"
            echo "✓ $service enabled"
        done
        ;;
        
    disable)
        echo -e "${YELLOW}Disabling Hashpool services auto-start...${NC}"
        for service in "${SERVICES[@]}"; do
            sudo systemctl disable "$service"
            echo "✓ $service disabled"
        done
        ;;
        
    *)
        echo "Usage: $0 {start|stop|restart|status|logs [service]|enable|disable}"
        echo ""
        echo "Commands:"
        echo "  start    - Start all services in correct order"
        echo "  stop     - Stop all services"
        echo "  restart  - Restart all services"
        echo "  status   - Show status of all services"
        echo "  logs     - Show logs for a service (default: bitcoind-sv2)"
        echo "  enable   - Enable services for auto-start on boot"
        echo "  disable  - Disable auto-start on boot"
        echo ""
        echo "Available services: ${SERVICES[*]}"
        exit 1
        ;;
esac