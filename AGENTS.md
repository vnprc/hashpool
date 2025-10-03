# AGENTS.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Hashpool is a fork of the Stratum V2 Reference Implementation (SRI) that replaces traditional share accounting with an **ecash mint**. Instead of using an account-based database to track miner shares internally, hashpool issues "ehash" tokens for each accepted share. These tokens can accrue value as the pool finds blocks, allowing miners to either hold them to maturity (accepting luck risk) or sell them early for guaranteed payouts.

## Key Architecture Differences from Standard SRI

### Traditional SRI Share Accounting
- Internal database tracking each miner's proof of work shares
- Account-based system with direct share-to-payout mapping
- Centralized accounting within the pool

### Hashpool's Ecash Approach
- **Separate Cashu mint service** running independently from the pool
- **Cashu wallet** integrated into the translator/proxy for managing tokens
- Each accepted share generates a blinded message → blinded signature → ecash token
- Tokens represent transferable, tradeable value instead of database entries

## System Architecture

Hashpool consists of two distinct deployments that communicate only through standard SV2 mining protocol:

### Pool Side Deployment
Components that run together (typically on pool operator's infrastructure):
1. **Pool** - SV2 mining pool coordinator
2. **Mint** - Standalone CDK Cashu mint service
3. **JD-Server** - Job Declarator Server
4. **Bitcoind** - Bitcoin node (pool side)

Configuration files:
- `config/pool.config.toml` - Pool-specific settings
- `config/mint.config.toml` - Mint service settings
- `config/jds.config.toml` - Job Declarator Server settings
- `config/shared/pool.toml` - Shared settings (pool-mint communication, ports, etc.)

### Miner Side Deployment
Components that run together (typically on miner's infrastructure):
1. **Translator** - SV2/SV1 proxy with integrated wallet
2. **JD-Client** - Job Declarator Client
3. **Bitcoind** - Bitcoin node (miner side)

Configuration files:
- `config/tproxy.config.toml` - Translator/proxy settings
- `config/jdc.config.toml` - Job Declarator Client settings
- `config/shared/miner.toml` - Shared miner-side settings

### Inter-Deployment Communication
**IMPORTANT**: The pool side and miner side deployments have **NO direct communication** except:
- Standard SV2 messages between Pool and Translator (share submissions, job assignments)
- Both sides operate their own Bitcoin nodes independently

There is **NO** direct communication between:
- Pool ↔ JD-Client
- Mint ↔ Translator
- JD-Server ↔ Translator
- Pool ↔ Bitcoind (miner side)

## Core Components Detail

### Pool Side Components

1. **Pool** (`roles/pool/`)
   - SV2 mining pool that coordinates work
   - Communicates with separate Mint service via TCP (SV2 MintQuote protocol)
   - Sends MintQuoteRequest messages to mint when shares are accepted
   - Web dashboard on port 8081 showing pool status and connections
   - Configuration: `config/pool.config.toml`, `config/shared/pool.toml`

2. **Mint** (`roles/mint/`)
   - **Standalone service** running independently from pool
   - CDK Cashu mint for ehash/ecash token operations
   - Receives quote requests via TCP from pool using SV2 MintQuote subprotocol
   - Generates blinded signatures for accepted shares
   - SQLite database at `.devenv/state/mint/mint.sqlite`
   - HTTP API on port 3338 for wallet operations
   - Configuration: `config/mint.config.toml`

3. **JD-Server** (`roles/jd-server/`)
   - Job Declarator Server for custom job negotiation
   - Talks to bitcoind (pool side) for block templates
   - Configuration: `config/jds.config.toml`

### Miner Side Components

4. **Translator** (`roles/translator/`)
   - Proxy that translates SV1 (downstream) ↔ SV2 (upstream)
   - Integrated Cashu wallet for managing ehash tokens
   - Bundles blinded messages with shares sent upstream to pool
   - Receives blinded signatures from pool and stores complete tokens
   - Web dashboard on port 3030 showing miner stats and wallet balance
   - SQLite database at `.devenv/state/translator/wallet.sqlite`
   - Configuration: `config/tproxy.config.toml`, `config/shared/miner.toml`

5. **JD-Client** (`roles/jd-client/`)
   - Job Declarator Client for custom job selection
   - Talks to bitcoind (miner side) for block template construction
   - Configuration: `config/jdc.config.toml`

### Web Dashboards

Both the Pool and Translator have embedded web dashboards:
- **Pool Dashboard** (port 8081): Shows pool status, connected miners, share statistics
- **Translator Dashboard** (port 3030): Shows wallet balance, miner stats, ehash redemption interface

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

## Configuration Layout

The configuration system uses a split structure to separate concerns between the two deployments:

### Shared Configuration Files
Located in `config/shared/`:
- **`pool.toml`** - Shared settings for pool-side deployment (pool-mint communication, ports, ehash difficulty)
- **`miner.toml`** - Shared settings for miner-side deployment (ports, ehash settings)

### Component-Specific Configuration Files
Located in `config/`:
- **Pool side**: `pool.config.toml`, `mint.config.toml`, `jds.config.toml`
- **Miner side**: `tproxy.config.toml`, `jdc.config.toml`

Example of pool-mint SV2 messaging configuration in `config/shared/pool.toml`:
```toml
[sv2_messaging]
enabled = true
mint_listen_address = "127.0.0.1:34260"
mpsc_buffer_size = 100
broadcast_buffer_size = 1000
max_retries = 3
timeout_ms = 5000
```

## Development Environment

The `devenv` stack runs all components together as a **smoke test** to ensure ehash token creation works end-to-end:
- Starts both pool-side and miner-side components locally
- Uses CPU miner to generate shares
- Primary purpose: verify the complete ehash issuance flow
- Not intended to represent production deployment topology

Run with:
```bash
devenv shell
devenv up
```

## Important Notes

1. **Deployment isolation**: Pool and miner sides are separate deployments with no direct inter-component communication
2. **Mint is standalone**: The mint runs as its own service, not embedded in the pool
3. **Two web dashboards**: Pool (8081) and Translator (3030) each have their own dashboard
4. **Shared Bitcoin nodes**: In devenv, both sides may share a Bitcoin node for convenience, but this is not required in production
5. **CDK dependencies**: Using forked CDK from `github.com/vnprc/cdk.git`
6. **Database paths**: Set via environment variables (e.g., `CDK_MINT_DB_PATH`)

## Testing Approach

The devenv stack serves as an integration test to verify:
1. Pool accepts shares from translator
2. Pool sends MintQuoteRequest to mint service
3. Mint generates blinded signatures
4. Translator receives and stores complete ehash tokens
5. Web dashboards display correct state