#!/bin/bash
# Simple VPS deployment script for Hashpool PoC
# Based on architect's recommendation: build with cargo on VPS, run via systemd

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}Hashpool VPS Deployment Script${NC}"
echo "================================="

# Check if running as root
if [[ $EUID -eq 0 ]]; then
   echo -e "${RED}This script should not be run as root initially${NC}"
   echo "It will use sudo when needed"
   exit 1
fi

# Configuration
HASHPOOL_USER="hashpool"
HASHPOOL_HOME="/opt/hashpool"
DATA_DIR="/var/lib/hashpool"
CONFIG_DIR="/etc/hashpool"
LOG_DIR="/var/log/hashpool"

# Step 1: Install build dependencies
echo -e "\n${YELLOW}Step 1: Installing build dependencies...${NC}"
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    sqlite3 \
    libsqlite3-dev \
    git \
    curl \
    cmake \
    clang \
    protobuf-compiler \
    wget

# Install Rust if not present
if ! command -v cargo &> /dev/null; then
    echo -e "${YELLOW}Installing Rust...${NC}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

# Step 2: Create user and directories
echo -e "\n${YELLOW}Step 2: Creating hashpool user and directories...${NC}"
sudo useradd -r -s /sbin/nologin $HASHPOOL_USER || echo "User already exists"

# Create directory structure
sudo mkdir -p $HASHPOOL_HOME/bin
sudo mkdir -p $DATA_DIR/{mint,translator,bitcoind}
sudo mkdir -p $CONFIG_DIR/shared
sudo mkdir -p $LOG_DIR

# Set permissions
sudo chown -R $HASHPOOL_USER:$HASHPOOL_USER $DATA_DIR
sudo chown -R $HASHPOOL_USER:$HASHPOOL_USER $LOG_DIR
sudo chmod 750 $DATA_DIR
sudo chmod 755 $CONFIG_DIR

# Step 2.5: Install Bitcoin SV2
echo -e "\n${YELLOW}Step 2.5: Installing Bitcoin SV2...${NC}"

# Platform detection (for Ubuntu VPS)
PLATFORM="x86_64-linux-gnu"
BITCOIN_VERSION="sv2-tp-0.1.17"
BITCOIN_URL="https://github.com/Sjors/bitcoin/releases/download/${BITCOIN_VERSION}/bitcoin-${BITCOIN_VERSION}-${PLATFORM}.tar.gz"

echo "Downloading Bitcoin SV2 from: $BITCOIN_URL"
cd /tmp
wget -q "$BITCOIN_URL" || {
    echo -e "${RED}Failed to download Bitcoin SV2${NC}"
    exit 1
}

echo "Extracting Bitcoin SV2..."
tar -xzf "bitcoin-${BITCOIN_VERSION}-${PLATFORM}.tar.gz"

echo "Installing Bitcoin SV2 binaries..."
sudo install -m 755 "bitcoin-${BITCOIN_VERSION}/bin/bitcoind" $HASHPOOL_HOME/bin/bitcoind-sv2
sudo install -m 755 "bitcoin-${BITCOIN_VERSION}/bin/bitcoin-cli" $HASHPOOL_HOME/bin/bitcoin-cli-sv2

# Cleanup
rm -rf "bitcoin-${BITCOIN_VERSION}"*

echo -e "${GREEN}Bitcoin SV2 installed successfully${NC}"

# Step 3: Build from source
echo -e "\n${YELLOW}Step 3: Building Hashpool from source...${NC}"

# Determine where to build from
BUILD_DIR="$HOME/hashpool-build"
echo "Using build directory: $BUILD_DIR"

# Clean build environment
if [ -d "$BUILD_DIR" ]; then
    echo "Removing existing build directory..."
    rm -rf "$BUILD_DIR"
fi

# Clone fresh copy
echo "Cloning hashpool repository..."
git clone -b deploy-test https://github.com/vnprc/hashpool.git "$BUILD_DIR"
cd "$BUILD_DIR/roles"

# Build all binaries
echo "Building mint..."
cargo build --release -p mint --bin mint

echo "Building pool..."
cargo build --release -p pool_sv2 --bin pool_sv2

echo "Building translator..."
cargo build --release -p translator_sv2 --bin translator_sv2

echo "Building JD server..."
cargo build --release -p jd_server --bin jd_server

echo "Building JD client..."
cargo build --release -p jd_client --bin jd_client

# Step 4: Install binaries
echo -e "\n${YELLOW}Step 4: Installing binaries...${NC}"
sudo install -m 755 target/release/mint $HASHPOOL_HOME/bin/
sudo install -m 755 target/release/pool_sv2 $HASHPOOL_HOME/bin/
sudo install -m 755 target/release/translator_sv2 $HASHPOOL_HOME/bin/
sudo install -m 755 target/release/jd_server $HASHPOOL_HOME/bin/
sudo install -m 755 target/release/jd_client $HASHPOOL_HOME/bin/

# Step 5: Copy configuration files
echo -e "\n${YELLOW}Step 5: Installing configuration files...${NC}"
cd "$BUILD_DIR"
sudo cp config/*.toml $CONFIG_DIR/
sudo cp -r config/shared/* $CONFIG_DIR/shared/

# Update config paths for production
sudo sed -i "s|.devenv/state/mint|$DATA_DIR/mint|g" $CONFIG_DIR/mint.config.toml
sudo sed -i "s|.devenv/state/translator|$DATA_DIR/translator|g" $CONFIG_DIR/tproxy.config.toml

# Step 6: Create systemd service files
echo -e "\n${YELLOW}Step 6: Creating systemd services...${NC}"

# Mint service
sudo tee /etc/systemd/system/hashpool-mint.service > /dev/null <<EOF
[Unit]
Description=Hashpool Mint (Cashu eCash Mint)
After=network.target

[Service]
Type=simple
User=$HASHPOOL_USER
Group=$HASHPOOL_USER
Environment="RUST_LOG=info"
Environment="CDK_MINT_DB_PATH=$DATA_DIR/mint/mint.sqlite"
ExecStart=$HASHPOOL_HOME/bin/mint -c $CONFIG_DIR/mint.config.toml -g $CONFIG_DIR/shared/pool.toml
Restart=on-failure
RestartSec=10

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DATA_DIR/mint $LOG_DIR

[Install]
WantedBy=multi-user.target
EOF

# Bitcoin SV2 service  
sudo tee /etc/systemd/system/bitcoind-sv2.service > /dev/null <<EOF
[Unit]
Description=Bitcoin SV2 Daemon
After=network.target

[Service]
Type=simple
User=$HASHPOOL_USER
Group=$HASHPOOL_USER
Environment="RUST_LOG=info"
ExecStart=$HASHPOOL_HOME/bin/bitcoind-sv2 -testnet4 -server=1 -rpcuser=username -rpcpassword=password -rpcport=48332 -rpcallowip=127.0.0.1 -datadir=$DATA_DIR/bitcoind
Restart=on-failure
RestartSec=10

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DATA_DIR/bitcoind $LOG_DIR

[Install]
WantedBy=multi-user.target
EOF

# Pool service
sudo tee /etc/systemd/system/hashpool-pool.service > /dev/null <<EOF
[Unit]
Description=Hashpool Pool (Stratum V2 Pool)
After=network.target hashpool-mint.service bitcoind-sv2.service
Wants=hashpool-mint.service bitcoind-sv2.service

[Service]
Type=simple
User=$HASHPOOL_USER
Group=$HASHPOOL_USER
Environment="RUST_LOG=info"
ExecStart=$HASHPOOL_HOME/bin/pool_sv2 -c $CONFIG_DIR/pool.config.toml -g $CONFIG_DIR/shared/pool.toml
Restart=on-failure
RestartSec=10

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$LOG_DIR

[Install]
WantedBy=multi-user.target
EOF

# Translator service
sudo tee /etc/systemd/system/hashpool-translator.service > /dev/null <<EOF
[Unit]
Description=Hashpool Translator (SV1 to SV2 Proxy)
After=network.target hashpool-pool.service
Wants=hashpool-pool.service

[Service]
Type=simple
User=$HASHPOOL_USER
Group=$HASHPOOL_USER
Environment="RUST_LOG=info"
Environment="CDK_WALLET_DB_PATH=$DATA_DIR/translator/wallet.sqlite"
ExecStart=$HASHPOOL_HOME/bin/translator_sv2 -c $CONFIG_DIR/tproxy.config.toml -g $CONFIG_DIR/shared/pool.toml
Restart=on-failure
RestartSec=10

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DATA_DIR/translator $LOG_DIR

[Install]
WantedBy=multi-user.target
EOF

# JD Server service
sudo tee /etc/systemd/system/hashpool-jd-server.service > /dev/null <<EOF
[Unit]
Description=Hashpool Job Declaration Server
After=network.target hashpool-pool.service
Wants=hashpool-pool.service

[Service]
Type=simple
User=$HASHPOOL_USER
Group=$HASHPOOL_USER
Environment="RUST_LOG=info"
ExecStart=$HASHPOOL_HOME/bin/jd_server -c $CONFIG_DIR/jds.config.toml
Restart=on-failure
RestartSec=10

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$LOG_DIR

[Install]
WantedBy=multi-user.target
EOF

# JD Client service
sudo tee /etc/systemd/system/hashpool-jd-client.service > /dev/null <<EOF
[Unit]
Description=Hashpool Job Declaration Client
After=network.target hashpool-jd-server.service
Wants=hashpool-jd-server.service

[Service]
Type=simple
User=$HASHPOOL_USER
Group=$HASHPOOL_USER
Environment="RUST_LOG=info"
ExecStart=$HASHPOOL_HOME/bin/jd_client -c $CONFIG_DIR/jdc.config.toml -g $CONFIG_DIR/shared/pool.toml
Restart=on-failure
RestartSec=10

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$LOG_DIR

[Install]
WantedBy=multi-user.target
EOF

# Step 7: Enable and start services
echo -e "\n${YELLOW}Step 7: Enabling systemd services...${NC}"
sudo systemctl daemon-reload
sudo systemctl enable bitcoind-sv2
sudo systemctl enable hashpool-mint
sudo systemctl enable hashpool-pool
sudo systemctl enable hashpool-translator
sudo systemctl enable hashpool-jd-server
sudo systemctl enable hashpool-jd-client

echo -e "\n${GREEN}Deployment complete!${NC}"
echo
echo "To start the services, run:"
echo "  sudo systemctl start bitcoind-sv2"
echo "  sudo systemctl start hashpool-mint"
echo "  sudo systemctl start hashpool-pool"
echo "  sudo systemctl start hashpool-translator"
echo "  sudo systemctl start hashpool-jd-server"
echo "  sudo systemctl start hashpool-jd-client"
echo
echo "To check service status:"
echo "  sudo systemctl status hashpool-mint"
echo "  sudo journalctl -u hashpool-mint -f"
echo
echo "Configuration files are in: $CONFIG_DIR"
echo "Data files are in: $DATA_DIR"
echo "Log files are in: $LOG_DIR"