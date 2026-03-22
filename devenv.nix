{
  pkgs,
  lib,
  config,
  inputs ? null,
  ...
}: let
  bitcoinNode = import ./bitcoin-node.nix {
    pkgs = pkgs;
    lib = lib;
    stdenv = pkgs.stdenv;
  };

  sv2tp = import ./sv2-tp.nix {
    pkgs = pkgs;
    lib = lib;
    stdenv = pkgs.stdenv;
  };

  # CDK configuration
  cdkRepo = "https://github.com/vnprc/cdk.git";
  cdkCommit = "0315c1f2";

  bitcoindDataDir = "${config.devenv.root}/.devenv/state/bitcoind";
  translatorWalletDb = "${config.devenv.root}/.devenv/state/translator/wallet.sqlite";
  mintDb = "${config.devenv.root}/.devenv/state/mint/mint.sqlite";
  prometheusPoolDataDir = "${config.devenv.root}/.devenv/state/prometheus-pool";
  prometheusProxyDataDir = "${config.devenv.root}/.devenv/state/prometheus-proxy";
  # Service ports (now loaded from config files)
  # These are kept for compatibility but the actual ports are defined in:
  # - config/web-pool.config.toml
  # - config/web-proxy.config.toml
  webPoolPort = 8081;
  webProxyPort = 3030;

  poolConfig = builtins.fromTOML (builtins.readFile ./config/shared/pool.toml);
  minerConfig = builtins.fromTOML (builtins.readFile ./config/shared/miner.toml);

  # supported values: "regtest", "testnet4"
  bitcoinNetwork = "regtest";
  # Set the default bitcoind RPC port, based on the network
  bitcoindRpcPort =
    if bitcoinNetwork == "regtest"
    then poolConfig.bitcoin.portRegtest
    else if bitcoinNetwork == "testnet4"
    then poolConfig.bitcoin.portTestnet
    else abort "Invalid network {$bitcoinNetwork}";

  # add logging to any command
  withLogging = command: logFile: ''
    mkdir -p ${config.devenv.root}/logs
    sh -c ${lib.escapeShellArg command} 2>&1 | stdbuf -oL tee -a ${config.devenv.root}/logs/${logFile}
  '';

  # wait for a port to open before proceeding
  waitForPort = port: name: ''
    wait_for_port() {
      local port="$1"
      local name="$2"
      [ -z "$name" ] && name="service"
      echo "Waiting for $name on port $port..."
      while ! nc -z localhost "$port"; do
        sleep 1
      done
      echo "$name is up!"
    }
    wait_for_port ${toString port} "${name}"
  '';

  # get all process names dynamically
  processNames = lib.attrNames config.processes;
in {
  env.BITCOIND_NETWORK = bitcoinNetwork;
  env.BITCOIND_RPC_PORT = bitcoindRpcPort;
  # TODO split bitcoind configs into poolside and minerside
  env.BITCOIND_DATADIR = config.devenv.root + "/.devenv/state/bitcoind";
  env.IN_DEVENV = "1";
  env.TRANSLATOR_WALLET_DB = translatorWalletDb;
  env.MINT_DB = mintDb;
  env.RUST_LOG = "info";

  # Ensure log and db directories exists before processes run
  tasks."create:dirs" = {
    exec = ''
      echo "Creating persistent directories..."
      mkdir -p ${config.devenv.root}/logs
      mkdir -p $(dirname ${translatorWalletDb})
      mkdir -p $(dirname ${mintDb})
      mkdir -p ${prometheusPoolDataDir}
      mkdir -p ${prometheusProxyDataDir}
    '';
    before = [
      "devenv:processes:proxy"
      "devenv:processes:pool"
      "devenv:processes:prometheus_pool"
      "devenv:processes:prometheus_proxy"
      "devenv:processes:web_pool"
      "devenv:processes:web_proxy"
    ];
  };

  # Build CDK CLI from remote repo using same CDK version as hashpool
  tasks."build:cdk:cli" = {
    exec = ''
      echo "Building CDK CLI from remote repo..."

      # Create temporary build directory
      CDK_BUILD_DIR=$(mktemp -d)
      cd "$CDK_BUILD_DIR"

      # Clone and build
      git clone https://github.com/vnprc/cdk.git .
      git checkout 77df2ae4
      cargo build --release --bin cdk-cli

      # Copy to hashpool bin directory
      mkdir -p ${config.devenv.root}/bin
      cp target/release/cdk-cli ${config.devenv.root}/bin/cdk-cli

      # Cleanup
      rm -rf "$CDK_BUILD_DIR"

      echo "✅ CDK CLI ready"
    '';
    before = ["devenv:processes:proxy" "devenv:processes:pool"];
  };

  # https://devenv.sh/packages/
  packages =
    [
      pkgs.netcat
      bitcoinNode
      sv2tp
      pkgs.just
      pkgs.coreutils # Provides stdbuf for disabling output buffering
      pkgs.openssl
      pkgs.pkg-config
      pkgs.sqlite # Add SQLite3 for database operations
      pkgs.protobuf # Required by cdk-signatory (gRPC/protobuf support)
      pkgs.prometheus
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [pkgs.darwin.apple_sdk.frameworks.Security];

  # https://devenv.sh/languages/
  languages.rust = {
    enable = true;
    # Use nightly for development (flake will use pinned stable for builds)
    # Note: removed 'channel' attribute - newer devenv uses inputs from Nix
  };

  # https://devenv.sh/processes/
  processes = {
    pool = {
      exec = withLogging ''
        ${waitForPort 18447 "sv2-tp"}
        cd ${config.devenv.root} && cargo -C roles/pool -Z unstable-options run -- \
          -c ${config.devenv.root}/config/pool.config.toml \
          -g ${config.devenv.root}/config/shared/pool.toml
      '' "pool.log";
    };

    jd-server = {
      exec = withLogging ''
        # Prepare config file
        sed -i -E "s/(core_rpc_port\s*=\s*)[0-9]+/\1${config.env.BITCOIND_RPC_PORT}/" ${config.devenv.root}/config/jds.config.toml
        if [ "$BITCOIND_NETWORK" = "regtest" ]; then
          DEVENV_ROOT=${config.devenv.root} BITCOIND_DATADIR=${bitcoindDataDir} ${config.devenv.root}/scripts/regtest-setup.sh
        fi
        ${waitForPort poolConfig.pool.port "Pool"}
        cd ${config.devenv.root} && cargo -C roles/jd-server -Z unstable-options run -- -c ${config.devenv.root}/config/jds.config.toml
      '' "jd-server.log";
    };

    jd-client = {
      exec = withLogging ''
        ${waitForPort 18447 "sv2-tp"}
        ${waitForPort poolConfig.pool.port "Pool"}
        ${waitForPort 34264 "JD-Server"}
        cd ${config.devenv.root} && cargo -C roles/jd-client -Z unstable-options run -- -c ${config.devenv.root}/config/jdc.config.toml
      '' "job-client.log";
    };

    mint = {
      exec = withLogging ''
        export CDK_MINT_DB_PATH=${mintDb}
        cd ${config.devenv.root} && cargo -C roles/mint -Z unstable-options run -- -c ${config.devenv.root}/config/mint.config.toml -g ${config.devenv.root}/config/shared/pool.toml
      '' "mint.log";
    };

    proxy = {
      exec = withLogging ''
        export CDK_WALLET_DB_PATH=${config.env.TRANSLATOR_WALLET_DB}
        ${waitForPort minerConfig.pool.port "Pool"}
        cd ${config.devenv.root} && cargo -C roles/translator -Z unstable-options run -- \
          -c ${config.devenv.root}/config/tproxy.config.toml
      '' "proxy.log";
    };

    prometheus_pool = {
      exec = withLogging ''
        ${waitForPort 9108 "Pool monitoring"}
        prometheus \
          --config.file=${config.devenv.root}/config/prometheus-pool.yml \
          --storage.tsdb.path=${prometheusPoolDataDir} \
          --web.listen-address=127.0.0.1:9090
      '' "prometheus-pool.log";
    };

    prometheus_proxy = {
      exec = withLogging ''
        ${waitForPort 9109 "Translator monitoring"}
        prometheus \
          --config.file=${config.devenv.root}/config/prometheus-proxy.yml \
          --storage.tsdb.path=${prometheusProxyDataDir} \
          --web.listen-address=127.0.0.1:9091
      '' "prometheus-proxy.log";
    };

    bitcoin_node = {
      exec = withLogging ''
        mkdir -p ${bitcoindDataDir}
        bitcoin -m node \
          -datadir=${bitcoindDataDir} \
          -chain=${config.env.BITCOIND_NETWORK} \
          -conf=${config.devenv.root}/config/bitcoin.conf \
          -ipcbind=unix
      '' "bitcoin-node-${config.env.BITCOIND_NETWORK}.log";
    };

    sv2_tp = {
      exec = withLogging ''
        # In regtest: run setup (creates wallet + ensures ≥16 blocks) then mine one
        # fresh block so bitcoin-node's chain tip is recent.  sv2-tp v1.0.6 waits for
        # IsInitialBlockDownload() to return false; a stale tip (>24 h old) keeps the
        # node in IBD indefinitely even though the chain is complete.
        if [ "${config.env.BITCOIND_NETWORK}" = "regtest" ]; then
          DEVENV_ROOT=${config.devenv.root} BITCOIND_DATADIR=${bitcoindDataDir} \
            ${config.devenv.root}/scripts/regtest-setup.sh
          FRESH_ADDR=$(bitcoin-cli \
            -datadir=${bitcoindDataDir} \
            -conf=${config.devenv.root}/config/bitcoin.conf \
            -regtest -rpcwallet=regtest getnewaddress 2>/dev/null)
          [ -n "$FRESH_ADDR" ] && bitcoin-cli \
            -datadir=${bitcoindDataDir} \
            -conf=${config.devenv.root}/config/bitcoin.conf \
            -regtest generatetoaddress 1 "$FRESH_ADDR" 2>/dev/null || true
          echo "Regtest IBD refresh: mined fresh block to update chain tip timestamp"
          echo "Waiting for bitcoin-node to exit IBD..."
          while bitcoin-cli \
              -datadir=${bitcoindDataDir} \
              -conf=${config.devenv.root}/config/bitcoin.conf \
              -regtest getblockchaininfo 2>/dev/null | grep -q '"initialblockdownload": true'; do
            sleep 1
          done
          echo "IBD complete, starting sv2-tp..."
        else
          ${waitForPort bitcoindRpcPort "Bitcoin Core RPC"}
        fi
        sv2-tp \
          -datadir=${bitcoindDataDir} \
          -chain=${config.env.BITCOIND_NETWORK} \
          -conf=${config.devenv.root}/config/sv2-tp.conf
      '' "sv2-tp.log";
    };

    miner = {
      exec = withLogging ''
        ${waitForPort minerConfig.proxy.port "Proxy"}
        cd roles/test-utils/mining-device-sv1
        while true; do
          stdbuf -oL cargo run 2>&1 | tee -a ${config.devenv.root}/logs/miner.log
          echo "Miner crashed. Restarting..." >> ${config.devenv.root}/logs/miner.log
          sleep 5
        done
      '' "miner.log";
    };

    web_pool = {
      exec = withLogging ''
        ${waitForPort 9090 "Prometheus (pool-side)"}
        cd ${config.devenv.root} && cargo -C roles/web-pool -Z unstable-options run -- \
          --web-pool-config ${config.devenv.root}/config/web-pool.config.toml \
          --shared-config ${config.devenv.root}/config/shared/pool.toml
      '' "web_pool.log";
    };

    web_proxy = {
      exec = withLogging ''
        ${waitForPort 9091 "Prometheus (proxy-side)"}
        cd ${config.devenv.root} && cargo -C roles/web-proxy -Z unstable-options run -- \
          --web-proxy-config ${config.devenv.root}/config/web-proxy.config.toml \
          --config ${config.devenv.root}/config/shared/miner.toml \
          --shared-config ${config.devenv.root}/config/shared/miner.toml
      '' "web_proxy.log";
    };
  };

  git-hooks.hooks = {
    alejandra.enable = true;
  };

  enterShell = ''
    echo Just
    echo ====
    just --list
    echo
    echo Running Processes
    echo =================
    ${lib.concatStringsSep "\n" (map (name: "echo \"${name}\"") processNames)}
    echo

    # Warn if ~/.bitcoin/bitcoin.conf exists
    if [ -f "$HOME/.bitcoin/bitcoin.conf" ]; then
      echo
      echo "⚠️  WARNING: ~/.bitcoin/bitcoin.conf exists and may interfere with this environment." >&2
      echo "⚠️  Please rename or remove it if you encounter network conflicts." >&2
      echo
    fi
  '';
}
