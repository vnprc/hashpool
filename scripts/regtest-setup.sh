#!/usr/bin/env bash

# regtest-setup.sh - Initialize regtest environment safely and idempotently
# create wallet 'regtest' if it doesn't exist
# check block height, if <16 generate blocks up to that height

set -euo pipefail

BITCOIN_CONF="${DEVENV_ROOT:-$(pwd)}/bitcoin.conf"
DATADIR="${BITCOIND_DATADIR:-$(pwd)/.devenv/state/bitcoind}"

get_conf_value() {
    local key="$1"
    grep -E "^[[:space:]]*$key[[:space:]]*=" "$BITCOIN_CONF" | tail -n1 | cut -d= -f2- | xargs
}

# Extract credentials from bitcoin.conf (or allow override via env)
RPC_USER="${BITCOIN_RPC_USER:-$(get_conf_value rpcuser)}"
RPC_PASS="${BITCOIN_RPC_PASS:-$(get_conf_value rpcpassword)}"

RPC_ARGS="-datadir=${DATADIR} -conf=${BITCOIN_CONF} -rpcuser=${RPC_USER} -rpcpassword=${RPC_PASS} -regtest"

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
for attempt in $(seq 1 $max_attempts); do
    if bitcoin-cli $RPC_ARGS getblockchaininfo >/dev/null 2>&1; then
        echo "bitcoind is ready!"
        break
    fi
    echo "Attempt $attempt/$max_attempts: bitcoind not ready yet, waiting..."
    sleep 2
done

if ! bitcoin-cli $RPC_ARGS getblockchaininfo >/dev/null 2>&1; then
    echo "ERROR: bitcoind failed to become ready after $max_attempts attempts"
    exit 1
fi

BLOCK_HEIGHT=$(bitcoin-cli $RPC_ARGS getblockcount 2>/dev/null || echo "0")
echo "Current block height: $BLOCK_HEIGHT"

create_and_load_wallet
if [ "$BLOCK_HEIGHT" -lt 16 ]; then
    BLOCKS_NEEDED=$((16 - BLOCK_HEIGHT))
    echo "Generating $BLOCKS_NEEDED blocks to reach height 16..."

    bitcoin-cli $RPC_ARGS -rpcwallet=regtest -generate "$BLOCKS_NEEDED"
    NEW_HEIGHT=$(bitcoin-cli $RPC_ARGS getblockcount)
    echo "✅ Regtest environment initialized! New block height: $NEW_HEIGHT"
fi