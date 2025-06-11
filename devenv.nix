{
  pkgs,
  lib,
  config,
  inputs,
  ...
}: let
  bitcoind = import ./bitcoind.nix {
    pkgs = pkgs;
    lib = lib;
    stdenv = pkgs.stdenv;
  };

  bitcoindDataDir = "${config.devenv.root}/.devenv/state/bitcoind";

  poolConfig = builtins.fromTOML (builtins.readFile ./config/pool.toml);
  minerConfig = builtins.fromTOML (builtins.readFile ./config/miner.toml);

  # Function to add logging logic to any command
  withLogging = command: logFile: ''
    mkdir -p ${config.devenv.root}/logs
    sh -c ${lib.escapeShellArg command} 2>&1 | stdbuf -oL tee -a ${config.devenv.root}/logs/${logFile}
  '';

  # Get all process names dynamically
  processNames = lib.attrNames config.processes;
in {
  # TODO split bitcoind configs into poolside and minerside
  env.BITCOIND_DATADIR = config.devenv.root + "/.devenv/state/bitcoind";
  env.IN_DEVENV = "1";

  # Ensure logs directory exists before processes run
  tasks.create-logs-dir = {
    exec = "mkdir -p ${config.devenv.root}/logs";
    before = ["devenv:enterShell"];
  };

  # https://devenv.sh/packages/
  packages =
    [
      pkgs.netcat
      bitcoind
      pkgs.just
      pkgs.coreutils # Provides stdbuf for disabling output buffering
      pkgs.redis
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [pkgs.darwin.apple_sdk.frameworks.Security];

  # https://devenv.sh/languages/
  languages.rust = {
    enable = true;
    channel = "nightly";
  };

  # https://devenv.sh/processes/
  processes = {
    redis = {exec = withLogging "mkdir -p ${config.devenv.root}/.devenv/state/redis && redis-server --dir ${config.devenv.root}/.devenv/state/redis --port ${toString poolConfig.redis.port}" "redis.log";};
    pool = {
      exec = withLogging ''
        DEVENV_ROOT=${config.devenv.root} BITCOIND_DATADIR=${bitcoindDataDir} ${config.devenv.root}/scripts/bitcoind-setup.sh
        echo "Waiting for Mint..."
        while ! nc -z localhost ${toString poolConfig.mint.port}; do
          sleep 1
        done
        echo "Mint is up. Starting Local Pool..."
        cargo -C roles/pool -Z unstable-options run -- \
          -c ${config.devenv.root}/roles/pool/config-examples/pool-config-local-tp-example.toml \
          -g ${config.devenv.root}/config/pool.toml
      '' "pool.log";
    };
    jd-server = {
      exec = withLogging ''
        echo "Waiting for Pool..."
        while ! nc -z localhost ${toString poolConfig.pool.port}; do
          sleep 1
        done
        echo "Pool is up. Starting Job Server..."
        cargo -C roles/jd-server -Z unstable-options run -- -c ${config.devenv.root}/roles/jd-server/config-examples/jds-config-local-example.toml
      '' "jd-server.log";
    };
    jd-client = {exec = withLogging "cargo -C roles/jd-client -Z unstable-options run -- -c ${config.devenv.root}/roles/jd-client/config-examples/jdc-config-local-example.toml" "job-client.log";};
    # TODO switch to miner config
    proxy = {
      exec = withLogging ''
        echo "Waiting for Pool..."
        while ! nc -z localhost ${toString minerConfig.pool.port}; do
          sleep 1
        done
        echo "Pool is up. Starting Proxy..."
        cargo -C roles/translator -Z unstable-options run -- \
          -c ${config.devenv.root}/roles/translator/config-examples/tproxy-config-local-jdc-example.toml \
          -g ${config.devenv.root}/config/pool.toml
      '' "proxy.log";
    };
    bitcoind = {
      exec = withLogging ''
        mkdir -p ${bitcoindDataDir}
        bitcoind -datadir=${bitcoindDataDir} -conf=${config.devenv.root}/bitcoin.conf
      '' "bitcoind-regtest.log";
    };
    miner = {
      exec = withLogging ''
        echo "Waiting for proxy..."
        while ! nc -z localhost ${toString minerConfig.proxy.port}; do
          sleep 1
        done
        echo "Proxy is up, starting miner..."
        cd roles/test-utils/mining-device-sv1
        while true; do
          RUST_LOG=debug stdbuf -oL cargo run 2>&1 | tee -a ${config.devenv.root}/logs/miner.log
          echo "Miner crashed. Restarting..." >> ${config.devenv.root}/logs/miner.log
          sleep 5
        done
      '' "miner.log";
    };
    mint = {
      exec = withLogging ''
        echo "Waiting for Redis on port ${toString poolConfig.redis.port}..."
        while ! nc -z localhost ${toString poolConfig.redis.port}; do
          sleep 1
        done
        echo "Redis is up. Starting Mint..."
        cargo -C roles/mint -Z unstable-options run -- \
          -c ${config.devenv.root}/roles/mint/config/mint.config.toml \
          -g ${config.devenv.root}/config/pool.toml
      '' "mint.log";
    };
  };

  pre-commit.hooks = {
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
  '';
}
