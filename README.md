# Hashpool

This project is a fork of the Stratum V2 Reference Implementation (SRI) that replaces traditional share accounting with an ecash mint. Instead of internally accounting for each miner's proof of work shares, hashpool issues an "ehash" token for each share accepted by the pool. For a limited time after issuance, ehash tokens accrue value in bitcoin as the pool finds blocks. Miners can choose to accept the 'luck risk' of finding blocks and hold these tokens to maturity or sell them early for a guaranteed payout.

You can find the original SRI README [here](./SRI_README.md).

## Getting Started

To run Hashpool, first **clone the repository** and follow the instructions to **[install nix and devenv](https://devenv.sh/getting-started/)**.

Once set up, cd into the hashpool directory and run:

```
devenv shell
devenv up
```

<img width="1705" height="1294" alt="Screenshot from 2025-10-11 08-37-22" src="https://github.com/user-attachments/assets/1a1cd855-be1a-419c-a517-f5ed8b0c265c" />

## Development Environment Setup

The development environment initializes a containerized system with the following components:

### Components Overview

1. `pool` - **SV2 Mining Pool**
   - coordinates mining tasks and distributes workloads to miners
   - issues an ecash token for each share accepted
   - manages an internal cashu mint
      - receives a blinded message for each mining share
      - signs it and returns a blinded signature to the proxy/wallet

2. `proxy` - **SV2 Translator Proxy**
   - talks stratum v1 to downstream miners and stratum v2 to the upstream pool
   - manages the cashu wallet
      - bundles a blinded message with each share sent upstream to the pool
      - receives the blinded signature for each blinded message
      - stores each unblinded message with it's unblinded signature (this is an ecash token)

3. `jd-client` - **SV2 Job Declarator Client**
   - talks to bitcoind miner side
   - retrieves block templates
   - negotiates work with upstream pool

4. `jd-server` - **SV2 Job Declarator Server**
   - talks to bitcoind pool side
   - negotiates work with downstream proxy

5. `bitcoin-node` - **Bitcoin Core 30.2 (multiprocess)**
   - official Bitcoin Core node binary with IPC support
   - serves block data to sv2-tp via unix socket

6. `sv2-tp` - **SV2 Template Provider (v1.0.6)**
   - connects to bitcoin-node via IPC socket
   - serves Stratum V2 block templates to the pool and jd-client

7. `miner` - **CPU Miner**
   - find shares to submit upstream to the proxy

8. `mint` - **CDK Cashu Mint**
   - generate ehash and ecash tokens
   - redeem ehash and ecash tokens

9. `stats-pool` - **Stats Service (Pool Side)**
   - collects and serves pool-side mining statistics
   - TCP interface to collect stats from Sv2 services
   - HTTP APIs to serve stats to the web service

10. `stats-proxy` - **Stats Service (Proxy Side)**
    - collects and serves proxy-side mining statistics
    - TCP interface to collect stats from Sv2 services
    - HTTP APIs to serve stats to the web service

11. `web-pool` - **Web Dashboard (Pool Side)**
    - web interface for pool statistics and monitoring
    - displays pool hashrate, services, and connected proxies
    - deployed at [pool.hashpool.dev](https://pool.hashpool.dev/)

12. `web-proxy` - **Web Dashboard (Proxy Side)**
    - web interface for proxy statistics and monitoring
    - wallet page displays ehash balance and an ehash faucet
    - miners page displays miner connection info and connected miners
    - pool page displays upstream pool and blockchain stats
    - deployed at [proxy.hashpool.dev](https://proxy.hashpool.dev/)

---

## Production Deployment

For deploying Hashpool on Debian 12, see the
[Deployment Guide](./docs/deployment.md).

---

## What's New in v0.2

- **SRI 1.7.0 migration complete**: all protocols and utilities now import from
  crates.io (roles_logic_sv2 deprecated), vendored crates limited to hashpool-specific
  modifications
- **New Template Provider**: Bitcoin Core 30.2 (`bitcoin-node`) + sv2-tp v1.0.6
  replaces the Sjors SV2 fork; connects via unix IPC socket
- **Ehash mint flow redesign**: end-to-end minting and share accounting were
  reworked across pool, translator, and mint services
- **New roles**: stats + web services for pool/proxy monitoring and wallet UX
- **Testnet staging**: dedicated testnet deployment configs and workflow
- **Fixed**: share difficulty formula in the SV1 translator now uses the SV2
  formula (`2^256 / target`) matching miner expectations
- **Fixed**: CoinbaseOutputConstraints 6-byte encoding (SRI 1.7.0 pool↔TP protocol)
- **Added**: Debian 12 deployment workflow (build-in-place + ship-only)

---

## Contribution

This project is very early. PRs and bug reports are very welcome!

---
