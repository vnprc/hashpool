# Hashpool - Stratum V2 Reference Implementation Fork

This project is a fork of the Stratum V2 Reference Implementation (SRI) that replaces traditional share accounting with an ecash mint. For each mining share accepted by the pool, Hashpool generates "ehash", an ecash token backed by proof of work. Miners can hold choose to hold these tokens to maturity or trade them.

You can access the original SRI README [here](https://github.com/stratum-mining/stratum/blob/main/README.md).

## Getting Started

To run Hashpool, first **clone the repository** and follow the instructions to **[install nix and devenv](https://devenv.sh/getting-started/)**.

Once set up, cd into the hashpool directory and run:

```
devenv shell
devenv up
```

## Development Environment Setup

The development environment initializes a containerized system with the following components:

### Components Overview

1. **SV2 Mining Pool**
   - Coordinates mining tasks, distributes workloads among miners, and aggregates results for potential block creation.

2. **SV2 Translator Proxy**
   - Translates between Stratum V1 and Stratum V2 protocols, allowing miners with different protocol versions to work with the pool.

3. **SV2 Job Declarator Client**
   - Initiates job negotiations with the Job Negotiator Server, enabling mining jobs to follow the Stratum V2 protocol.

4. **SV2 Job Declarator Server**
   - Manages incoming job negotiation requests from clients and assigns appropriate mining jobs.

5. **Bitcoind (Sjors' SV2 Fork)**
   - A modified Bitcoin node supporting Stratum V2, serving as the blockchain backend for the mining pool.

6. **CPUMiner**
   - A CPU-based miner connecting to the SV2 Mining Pool to perform simulated mining operations.

---

## Contribution

This project is very early. PRs and bug reports are very welcome!

---

