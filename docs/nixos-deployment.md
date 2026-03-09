Hashpool NixOS Deployment Guide

This guide covers deploying Hashpool on a NixOS server. The NixOS module manages
all services (bitcoin-node, sv2-tp, pool, jd-server, jd-client, mint, translator)
as systemd units with proper dependency ordering.

---

1. Prerequisites

- NixOS server with flakes enabled
- x86_64-linux architecture (only platform with verified binary hashes)
- Minimum hardware for testnet4: 2 vCPU, 4 GB RAM, 50 GB SSD
- A Bitcoin wallet address for coinbase rewards (your payout address)

Enable flakes in your NixOS configuration:

  nix.settings.experimental-features = [ "nix-command" "flakes" ];

---

2. Adding Hashpool as a flake input

In your system flake (e.g. /etc/nixos/flake.nix):

  {
    inputs = {
      nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
      hashpool.url = "github:vnprc/hashpool";
      hashpool.inputs.nixpkgs.follows = "nixpkgs";
    };

    outputs = { self, nixpkgs, hashpool, ... }: {
      nixosConfigurations.my-pool = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          ./configuration.nix
          hashpool.nixosModules.default
        ];
      };
    };
  }

---

3. Minimal configuration.nix snippet

  services.hashpool = {
    enable = true;
    network = "testnet4";        # or "mainnet" for production
    dataDir = "/var/lib/hashpool";
    configDir = "/etc/hashpool/config";
  };

This starts all seven services (bitcoin-node, sv2-tp, pool, jd-server, jd-client,
mint, translator) as the `hashpool` system user.

To disable the optional services:

  services.hashpool = {
    enable = true;
    network = "testnet4";
    dataDir = "/var/lib/hashpool";
    configDir = "/etc/hashpool/config";
    enableMint = false;         # disable Cashu mint
    enableTranslator = false;   # disable SV1 translator/proxy
  };

---

4. Config file layout

The configDir must contain the following files before starting services:

  /etc/hashpool/config/
    bitcoin.conf               # Bitcoin Core options (rpcuser, rpcpassword, etc.)
    sv2-tp.conf                # sv2-tp options (sv2bind, ipcconnect, debug)
    pool.config.toml           # Pool auth keys, coinbase script, tp_address
    jds.config.toml            # JD Server config (Bitcoin RPC, listen port)
    jdc.config.toml            # JD Client config (tp_address, upstreams)
    mint.config.toml           # Cashu mint config (listen port, mnemonic)
    tproxy.config.toml         # Translator/proxy config (downstream port, upstreams)
    shared/
      pool.toml                # Shared pool-side config (ports, bitcoin RPC)
      miner.toml               # Shared miner-side config (ports)

Copy the reference configs from the hashpool repository:

  git clone https://github.com/vnprc/hashpool
  cp -r hashpool/config /etc/hashpool/config

Then edit the files for your deployment (see Section 5).

---

5. Operator-specific settings

Before deploying, update these values in your config files:

pool.config.toml — generate fresh authority keypair:

  # Generate with: cdk-cli generate-keys (or any SV2 key generation tool)
  authority_public_key = "<your-fresh-public-key>"
  authority_secret_key = "<your-fresh-secret-key>"

  # Set to a Bitcoin address you control (receives coinbase rewards)
  coinbase_reward_script = "addr(<your-bitcoin-address>)"

  # Set tp_address for your network:
  # testnet4: tp_address = "127.0.0.1:8443"
  # mainnet:  tp_address = "127.0.0.1:8442"
  tp_address = "127.0.0.1:8443"

  pool_signature = "Your Pool Name"

jdc.config.toml — match authority keys with pool.config.toml:

  authority_public_key = "<same-public-key-as-pool>"
  authority_secret_key = "<same-secret-key-as-pool>"
  tp_address = "127.0.0.1:8443"

  [[upstreams]]
  authority_pubkey = "<same-public-key-as-pool>"
  pool_address = "127.0.0.1"
  pool_port = 34254
  jds_address = "127.0.0.1"
  jds_port = 34264

bitcoin.conf — set RPC credentials:

  rpcuser = hashpool
  rpcpassword = <strong-random-password>

sv2-tp.conf — configure for testnet4:

  sv2bind=127.0.0.1
  ipcconnect=unix
  debug=sv2

For mainnet, set the listen port to match jdc/pool tp_address above.

---

6. First-run steps

After nixos-rebuild switch:

a) Check that bitcoin-node is syncing:

  bitcoin-cli -datadir=/var/lib/hashpool -chain=testnet4 getblockchaininfo

   Look for "initialblockdownload": false once IBD completes.

b) Check sv2-tp is serving templates:

  journalctl -u hashpool-sv2-tp -f

   Look for "Template Provider started" and "New template" log lines.

c) Check pool is connected:

  journalctl -u hashpool-pool -f

   Look for "Connected to Template Provider" and "Pool listening on 0.0.0.0:34254".

d) Check JD services:

  journalctl -u hashpool-jd-server -f
  journalctl -u hashpool-jd-client -f

e) Check mint (if enabled):

  journalctl -u hashpool-mint -f

   Look for "Mint listening on localhost:3338".

f) Test miner connection (SV1):

  Connect a miner to <server-ip>:34255 using stratum+tcp://
  Check translator logs: journalctl -u hashpool-translator -f

---

7. Service dependency ordering

The systemd units are ordered as follows:

  bitcoin-node
       |
     sv2-tp
       |     \
     pool   jd-client
       |
  jd-server

  mint           (independent, starts immediately)
  translator     (starts after pool)

If any service fails to connect (e.g. pool can't reach sv2-tp on startup),
systemd will restart it with a 5-second delay (Restart=on-failure, RestartSec=5s).

---

8. Monitoring

View logs for any service:

  journalctl -u hashpool-<service-name> -f

All services:

  journalctl -u "hashpool-*" -f

Check systemd service status:

  systemctl status hashpool-bitcoin-node
  systemctl status hashpool-sv2-tp
  systemctl status hashpool-pool
  systemctl status hashpool-jd-server
  systemctl status hashpool-jd-client
  systemctl status hashpool-mint
  systemctl status hashpool-translator

Bitcoin node health:

  bitcoin-cli -datadir=/var/lib/hashpool -chain=testnet4 getnetworkinfo
  bitcoin-cli -datadir=/var/lib/hashpool -chain=testnet4 getmininginfo

---

9. Overriding packages

To use a custom-built version of any package, override the package options:

  services.hashpool = {
    enable = true;
    network = "testnet4";
    dataDir = "/var/lib/hashpool";
    configDir = "/etc/hashpool/config";
    poolPackage = pkgs.callPackage ./my-pool.nix {};
    bitcoinNodePackage = pkgs.bitcoin;  # use nixpkgs Bitcoin Core instead
  };

---

10. Testnet4 vs mainnet port differences

  Service      | regtest | testnet4 | mainnet
  -------------|---------|----------|--------
  sv2-tp       | 18447   | 8443     | 8442
  pool SV2     | 34254   | 34254    | 34254
  translator   | 34255   | 34255    | 34255
  jd-server    | 34264   | 34264    | 34264
  jd-client    | 34265   | 34265    | 34265
  mint HTTP    | 3338    | 3338     | 3338

The sv2-tp port is the most important to get right — it must match
tp_address in pool.config.toml and jdc.config.toml.
