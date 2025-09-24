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

### New Components

3. **Mint** (`roles/mint/`)
   - Standalone CDK Cashu mint for token operations
   - Generates and redeems ehash/ecash tokens
   - Receives quote requests via TCP from pool using SV2 protocol
   - SQLite database at `.devenv/state/mint/mint.sqlite`
   - Configuration: `config/mint.config.toml`

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

## Testing Approach

1. Monitor TCP messages between pool and mint
2. Verify quote creation and response flow
3. Debug keyset communication issue
4. Test token minting once keyset is available