# Hashpool

This project is a fork of the Stratum V2 Reference Implementation (SRI) that replaces traditional share accounting with an ecash mint. Instead of internally accounting for each miner's proof of work shares, hashpool issues an "ehash" token for each share accepted by the pool. For a limited time after issuance, ehash tokens accrue value in bitcoin as the pool finds blocks. Miners can choose to accept the 'luck risk' of finding blocks and hold these tokens to maturity or sell them early for a guaranteed payout.

You can find the original SRI README [here](https://github.com/stratum-mining/stratum/blob/main/README.md).

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

5. `bitcoind` - **Bitcoin Daemon (Sjors' SV2 Fork)**
   - modified bitcoind supporting stratum v2
   - check the [PR](https://github.com/bitcoin/bitcoin/pull/29432) for more information

6. `miner` - **CPU Miner**
   - find shares to submit upstream to the proxy

7. `mint` - **CDK Cashu Mint**
   - generate ehash and ecash tokens
   - redeem ehash and ecash tokens

8. `stats-pool` - **Stats Service (Pool Side)**
   - collects and serves pool-side mining statistics
   - TCP interface to collect stats from Sv2 services
   - HTTP APIs to serve stats to the web service

9. `stats-proxy` - **Stats Service (Proxy Side)**
   - collects and serves proxy-side mining statistics
   - TCP interface to collect stats from Sv2 services
   - HTTP APIs to serve stats to the web service

10. `web-pool` - **Web Dashboard (Pool Side)**
    - web interface for pool statistics and monitoring
    - displays pool hashrate, services, and connected proxies
    - deployed at [pool.hashpool.dev](https://pool.hashpool.dev/)

11. `web-proxy` - **Web Dashboard (Proxy Side)**
    - web interface for proxy statistics and monitoring
    - wallet page displays ehash balance and an ehash faucet
    - miners page displays miner connection info and connected miners
    - pool page displays upstream pool and blockchain stats
    - deployed at [proxy.hashpool.dev](https://proxy.hashpool.dev/)

---

## Contribution

This project is very early. PRs and bug reports are very welcome!

---

