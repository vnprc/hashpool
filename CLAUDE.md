# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Hashpool is a fork of the Stratum V2 Reference Implementation (SRI) that replaces traditional share accounting with an **ecash mint**. Instead of using an account-based database to track miner shares internally, hashpool issues "ehash" tokens for each accepted share. These tokens can accrue value as the pool finds blocks, allowing miners to either hold them to maturity (accepting luck risk) or sell them early for guaranteed payouts.

## Key Architecture Differences from Standard SRI

### Traditional SRI Share Accounting
- Internal database tracking each miner's proof of work shares
- Account-based system with direct share-to-payout mapping
- Centralized accounting within the pool

### Hashpool's Ecash Approach
- **Cashu mint** integrated into the pool role for issuing ecash tokens
- **Cashu wallet** integrated into the translator/proxy for managing tokens
- Each accepted share generates a blinded message → blinded signature → ecash token
- Tokens represent transferable, tradeable value instead of database entries

## Core Components

### Modified SRI Components

1. **Pool** (`roles/pool/`)
   - Extended with internal Cashu mint functionality
   - Issues ecash tokens for accepted shares via blinded signatures
   - Direct TCP communication with mint role via SV2 protocol
   - Configuration: `config/pool.config.toml`, `config/shared/pool.toml`
   - Sends MintQuoteRequest messages to mint when shares are submitted

2. **Translator/Proxy** (`roles/translator/`)
   - Manages Cashu wallet for miners
   - Bundles blinded messages with shares sent to pool
   - Stores unblinded ecash tokens (message + signature pairs)
   - SQLite database at `.devenv/state/translator/wallet.sqlite`
   - **⚠️ ISSUE**: Currently not receiving keyset information from mint

### New Components

3. **Mint** (`roles/mint/`)
   - Standalone CDK Cashu mint for token operations
   - Generates and redeems ehash/ecash tokens
   - Receives quote requests via TCP from pool using SV2 protocol
   - SQLite database at `.devenv/state/mint/mint.sqlite`
   - Configuration: `config/mint.config.toml`
   - **⚠️ ISSUE**: Not communicating keyset to translator/wallet

## Current Architecture Status

### Working
- Pool successfully sends MintQuoteRequest to mint via TCP
- Mint receives requests and creates quotes
- Mint sends MintQuoteResponse back to pool
- Direct TCP communication between pool and mint roles

### Broken
- **Wallet/Translator cannot mint tokens** - Shows error: "No keysets available in wallet"
- **Root cause**: No mechanism for mint to share its keyset ID with translator
- Previously used Redis for keyset distribution (now removed)
- Need to implement keyset sharing mechanism

## Development Commands

### Build Commands
```bash
# Build specific workspace
cd protocols && cargo build
cd roles && cargo build

# Build specific components
cd roles/pool && cargo build
cd roles/mint && cargo build
cd roles/translator && cargo build

# Full workspace build
cargo build --workspace
```

### Testing
```bash
# Run all tests
cargo test

# Run specific component tests
cd roles/pool && cargo test
cd roles/mint && cargo test

# Integration tests
cd roles/tests-integration && cargo test
```

### Code Quality
```bash
# Format code
cargo fmt

# Run linter
cargo clippy

# Check without building
cargo check
```

### Development Environment
```bash
# Start all services with devenv
devenv shell
devenv up

# With backtrace for debugging
just up backtrace

# Database access
just db wallet  # Access wallet SQLite
just db mint    # Access mint SQLite

# Clean data
just clean cashu     # Delete all SQLite data
just clean regtest   # Delete regtest blockchain data
just clean testnet4  # Delete testnet4 blockchain data

# Generate blocks (regtest only)
just generate-blocks 10
```

### CDK Dependency Management
```bash
# Point to local CDK repo for development
just local-cdk

# Update CDK commit hash
just update-cdk OLD_REV NEW_REV

# Restore original dependencies
just restore-deps
```

## Current Development Task: Fix Keyset Communication

### Problem
The translator/wallet cannot mint ecash tokens because it doesn't know the mint's keyset ID. Error: "No keysets available in wallet - skipping mint attempt"

### Previous Solution (Removed)
- Redis was used to share keyset information
- Mint published active keyset to Redis
- Pool/Translator read keyset from Redis on startup

### Proposed Solutions
1. **Add keyset to MintQuoteResponse** - Include keyset_id in the response message
2. **Separate keyset broadcast message** - Create new SV2 message type for keyset updates
3. **HTTP endpoint** - Translator queries mint's HTTP API for keysets
4. **Configuration** - Static keyset in config (less flexible)

### Key Files to Modify
- `protocols/v2/subprotocols/mint-quote/src/mint_quote_response.rs` - Add keyset field
- `roles/mint/src/lib/sv2_connection/quote_processing.rs` - Include keyset in response
- `roles/pool/src/lib/mining_pool/message_handler.rs` - Forward keyset to translator
- `roles/translator/src/lib/upstream_sv2/upstream.rs` - Receive and store keyset

## Configuration

All SV2 messaging configuration in `config/shared/pool.toml`:
```toml
[sv2_messaging]
enabled = true
mpsc_buffer_size = 100
broadcast_buffer_size = 100
max_retries = 3
timeout_seconds = 30
```

## Important Notes

1. **Direct TCP communication**: Pool and mint communicate directly via TCP, no message hub
2. **CDK dependencies**: Using forked CDK from `github.com/vnprc/cdk.git`
3. **Database paths**: Set via environment variables (e.g., `CDK_MINT_DB_PATH`)
4. **Redis removed**: All Redis-based communication has been deleted
5. **Keyset issue**: Critical blocker - wallet cannot create tokens without keyset

## Testing Approach

1. Monitor TCP messages between pool and mint
2. Verify quote creation and response flow
3. Debug keyset communication issue
4. Test token minting once keyset is available