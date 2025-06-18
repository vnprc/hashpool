#!/usr/bin/env bash

# bitcoind-setup.sh - Initialize regtest environment
# This script waits for bitcoind to be ready, then checks if we need to initialize
# the regtest environment with a wallet and some initial blocks. This should only
# need to be run once per dev environment setup or if you've reset your state. It's
# idempotent, so you can safely run it multiple times.

set -e

BITCOIN_CONF="${DEVENV_ROOT:-$(pwd)}/bitcoin.conf"
DATADIR="${BITCOIND_DATADIR:-$(pwd)/.devenv/state/bitcoind}"
RPC_ARGS="-datadir=${DATADIR} -conf=${BITCOIN_CONF} -rpcuser=username -rpcpassword=password -regtest"

create_and_load_wallet() {
    echo "Creating/loading regtest wallet..."
    if ! bitcoin-cli $RPC_ARGS createwallet "regtest" 2>/dev/null; then
        echo "Wallet exists, attempting to load..."
        bitcoin-cli $RPC_ARGS loadwallet "regtest" 2>/dev/null || echo "Wallet already loaded"
    fi
}

echo "Waiting for bitcoind to be ready..."

# Wait for bitcoind RPC to be available
max_attempts=30
attempt=0
while [ $attempt -lt $max_attempts ]; do
    if bitcoin-cli $RPC_ARGS getblockchaininfo >/dev/null 2>&1; then
        echo "bitcoind is ready!"
        break
    fi
    attempt=$((attempt + 1))
    echo "Attempt $attempt/$max_attempts: bitcoind not ready yet, waiting..."
    sleep 2
done

if [ $attempt -eq $max_attempts ]; then
    echo "ERROR: bitcoind failed to become ready after $max_attempts attempts"
    exit 1
fi

BLOCK_HEIGHT=$(bitcoin-cli $RPC_ARGS getblockcount 2>/dev/null || echo "0")
echo "Current block height: $BLOCK_HEIGHT"

# Ensure we have at least 16 blocks and a wallet
if [ "$BLOCK_HEIGHT" -lt "16" ]; then
    BLOCKS_NEEDED=$((16 - BLOCK_HEIGHT))
    echo "Block height is $BLOCK_HEIGHT, need to generate $BLOCKS_NEEDED more blocks..."

    create_and_load_wallet

    echo "Generating $BLOCKS_NEEDED blocks to reach height 16..."
    bitcoin-cli $RPC_ARGS -rpcwallet=regtest -generate $BLOCKS_NEEDED

    NEW_HEIGHT=$(bitcoin-cli $RPC_ARGS getblockcount)
    echo "✅ Regtest environment initialized! New block height: $NEW_HEIGHT"
else
    echo "✅ Regtest environment already has sufficient blocks (height: $BLOCK_HEIGHT)"

    create_and_load_wallet
fi

echo "bitcoind setup complete!"
