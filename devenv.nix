{
  pkgs,
  lib,
  config,
  inputs ? null,
  ...
}: let
  bitcoind = import ./bitcoind.nix {
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

  # Service ports (now loaded from config files)
  # These are kept for compatibility but the actual ports are defined in:
  # - config/stats-pool.config.toml
  # - config/stats-proxy.config.toml
  # - config/web-pool.config.toml
  # - config/web-proxy.config.toml
  statsPoolTcpPort = 9083;
  statsPoolHttpPort = 9084;
  statsProxyTcpPort = 8082;
  statsProxyHttpPort = 8084;
  webPoolPort = 8081;
  webProxyPort = 3030;

  poolConfig = builtins.fromTOML (builtins.readFile ./config/shared/pool.toml);
  minerConfig = builtins.fromTOML (builtins.readFile ./config/shared/miner.toml);

  # supported values: "regtest", "testnet4"
  bitcoinNetwork = "testnet4";
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
    '';
    before = ["devenv:processes:proxy" "devenv:processes:pool" "devenv:processes:stats_pool" "devenv:processes:stats_proxy" "devenv:processes:web_pool" "devenv:processes:web_proxy"];
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
      git checkout 62114cf6
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
      bitcoind
      pkgs.just
      pkgs.coreutils # Provides stdbuf for disabling output buffering
      pkgs.openssl
      pkgs.pkg-config
      pkgs.sqlite # Add SQLite3 for database operations
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
        ${waitForPort 9083 "Stats-Pool"}
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
        ${waitForPort 8082 "Stats-Proxy"}
        ${waitForPort minerConfig.pool.port "Pool"}
        cd ${config.devenv.root} && cargo -C roles/translator -Z unstable-options run -- \
          -c ${config.devenv.root}/config/tproxy.config.toml \
          -g ${config.devenv.root}/config/shared/miner.toml
      '' "proxy.log";
    };

    bitcoind = {
      exec = withLogging ''
        mkdir -p ${bitcoindDataDir}
        bitcoind -datadir=${bitcoindDataDir} -chain=${config.env.BITCOIND_NETWORK} -conf=${config.devenv.root}/config/bitcoin.conf
      '' "bitcoind-${config.env.BITCOIND_NETWORK}.log";
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

    # mint process removed - Phase 3 deferred until SRI spec provides PoolMessages and plain_connection_tokio APIs

    stats_pool = {
      exec = withLogging ''
        cd ${config.devenv.root} && cargo -C roles/stats-pool -Z unstable-options run -- \
          --config ${config.devenv.root}/config/stats-pool.config.toml
      '' "stats_pool.log";
    };

    stats_proxy = {
      exec = withLogging ''
        cd ${config.devenv.root} && cargo -C roles/stats-proxy -Z unstable-options run -- \
          --config ${config.devenv.root}/config/stats-proxy.config.toml \
          --tproxy-config ${config.devenv.root}/config/tproxy.config.toml \
          --shared-config ${config.devenv.root}/config/shared/miner.toml
      '' "stats_proxy.log";
    };

    web_pool = {
      exec = withLogging ''
        ${waitForPort statsPoolTcpPort "Stats-Pool"}
        cd ${config.devenv.root} && cargo -C roles/web-pool -Z unstable-options run -- \
          --web-pool-config ${config.devenv.root}/config/web-pool.config.toml \
          --shared-config ${config.devenv.root}/config/shared/pool.toml
      '' "web_pool.log";
    };

    web_proxy = {
      exec = withLogging ''
        ${waitForPort statsProxyTcpPort "Stats-Proxy"}
        cd ${config.devenv.root} && cargo -C roles/web-proxy -Z unstable-options run -- \
          --web-proxy-config ${config.devenv.root}/config/web-proxy.config.toml \
          --config ${config.devenv.root}/config/tproxy.config.toml \
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
