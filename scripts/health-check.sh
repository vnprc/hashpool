#!/bin/bash
# Simple health check script for Hashpool services

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "Hashpool Health Check"
echo "===================="

# Check systemd services
services=("bitcoind-sv2" "hashpool-mint" "hashpool-pool" "hashpool-translator" "hashpool-jd-server" "hashpool-jd-client")

for service in "${services[@]}"; do
    if systemctl is-active --quiet "$service"; then
        echo -e "$service: ${GREEN}Running${NC}"
    else
        echo -e "$service: ${RED}Not Running${NC}"
    fi
done

echo
echo "Port Check:"
echo "-----------"

# Check ports
ports=(
    "48332:Bitcoin SV2 RPC"
    "3338:Mint HTTP"
    "34254:Pool SV2"
    "34255:Translator SV1"
    "34260:Pool-Mint Internal"
    "34264:JD Server"
    "34265:JD Client"
)

for port_desc in "${ports[@]}"; do
    port="${port_desc%%:*}"
    desc="${port_desc##*:}"
    
    if nc -z localhost "$port" 2>/dev/null; then
        echo -e "Port $port ($desc): ${GREEN}Open${NC}"
    else
        echo -e "Port $port ($desc): ${YELLOW}Closed${NC}"
    fi
done

echo
echo "Database Check:"
echo "--------------"

# Check SQLite databases
if [ -f "/var/lib/hashpool/mint/mint.sqlite" ]; then
    echo -e "Mint DB: ${GREEN}Exists${NC}"
    size=$(du -h "/var/lib/hashpool/mint/mint.sqlite" | cut -f1)
    echo "  Size: $size"
else
    echo -e "Mint DB: ${YELLOW}Not Found${NC}"
fi

if [ -f "/var/lib/hashpool/translator/wallet.sqlite" ]; then
    echo -e "Wallet DB: ${GREEN}Exists${NC}"
    size=$(du -h "/var/lib/hashpool/translator/wallet.sqlite" | cut -f1)
    echo "  Size: $size"
else
    echo -e "Wallet DB: ${YELLOW}Not Found${NC}"
fi

echo
echo "Bitcoin RPC Check:"
echo "-----------------"
if command -v /opt/hashpool/bin/bitcoin-cli-sv2 >/dev/null 2>&1; then
    if /opt/hashpool/bin/bitcoin-cli-sv2 -testnet4 -rpcuser=username -rpcpassword=password -rpcport=48332 getblockchaininfo >/dev/null 2>&1; then
        echo -e "Bitcoin RPC: ${GREEN}Connected${NC}"
        BLOCK_COUNT=$(/opt/hashpool/bin/bitcoin-cli-sv2 -testnet4 -rpcuser=username -rpcpassword=password -rpcport=48332 getblockcount 2>/dev/null)
        echo "  Block height: ${BLOCK_COUNT:-Unknown}"
    else
        echo -e "Bitcoin RPC: ${RED}Not responding${NC}"
    fi
else
    echo -e "Bitcoin CLI: ${YELLOW}Not installed${NC}"
fi

echo
echo "Recent Logs:"
echo "-----------"
echo "Use these commands to view logs:"
echo "  sudo journalctl -u bitcoind-sv2 -n 10"
echo "  sudo journalctl -u hashpool-mint -n 10"
echo "  sudo journalctl -u hashpool-pool -n 10"
echo "  sudo journalctl -u hashpool-translator -n 10"