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

  configFile = ./devenv.toml;
  appConfig = builtins.fromTOML (builtins.readFile configFile);

  redis = appConfig.redis;
  mintd = appConfig.mintd;
  pool = appConfig.pool;
  proxy = appConfig.proxy;
  
  REDIS_PORT = toString redis.port;
  MINTD_PORT = toString mintd.port;
  POOL_PORT = toString pool.port;
  PROXY_PORT = toString proxy.port;

  # Function to add logging logic to any command
  withLogging = command: logFile: ''
    mkdir -p ${config.devenv.root}/logs
    sh -c ${lib.escapeShellArg command} 2>&1 | stdbuf -oL tee -a ${config.devenv.root}/logs/${logFile}
  '';

  # Get all process names dynamically
  processNames = lib.attrNames config.processes;
in {
  env.REDIS_URL = redis.url;
  env.REDIS_HOST = redis.host;
  env.REDIS_PORT = builtins.toString redis.port;

  env.MINTD_PORT = builtins.toString mintd.port;
  env.POOL_PORT = builtins.toString pool.port;
  env.PROXY_PORT = builtins.toString proxy.port;

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
    redis = {exec = withLogging "mkdir -p ${config.devenv.root}/.devenv/state/redis && redis-server --dir ${config.devenv.root}/.devenv/state/redis --port $REDIS_PORT" "redis.log";};
    pool = {
      exec = withLogging ''
        echo "Waiting for Mintd..."
        while ! nc -z localhost ${MINTD_PORT}; do
          sleep 1
        done
        echo "Mintd is up. Starting Local Pool..."
        cargo -C roles/pool -Z unstable-options run -- -c $DEVENV_ROOT/roles/pool/config-examples/pool-config-local-tp-example.toml
      '' "pool.log";
    };
    jd-server = {
      exec = withLogging ''
        echo "Waiting for Pool..."
        while ! nc -z localhost ${POOL_PORT}; do
          sleep 1
        done
        echo "Pool is up. Starting Job Server..."
        cargo -C roles/jd-server -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-server/config-examples/jds-config-local-example.toml
      '' "jd-server.log";
    };
    jd-client = {exec = withLogging "cargo -C roles/jd-client -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-client/config-examples/jdc-config-local-example.toml" "job-client.log";};
    proxy = {
      exec = withLogging ''
        echo "Waiting for Pool..."
        while ! nc -z localhost ${POOL_PORT}; do
          sleep 1
        done
        echo "Pool is up. Starting Proxy..."
        cargo -C roles/translator -Z unstable-options run -- -c $DEVENV_ROOT/roles/translator/config-examples/tproxy-config-local-jdc-example.toml
      '' "proxy.log";
    };
    bitcoind = {
      exec = withLogging ''
        mkdir -p $BITCOIND_DATADIR
        bitcoind -datadir=$BITCOIND_DATADIR -conf=$DEVENV_ROOT/bitcoin.conf
      '' "bitcoind-testnet.log";
    };
    miner = {
      exec = withLogging ''
        echo "Waiting for proxy..."
        while ! nc -z localhost ${PROXY_PORT}; do
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
        echo "Waiting for Redis on port ${REDIS_PORT}..."
        while ! nc -z localhost ${REDIS_PORT}; do
          sleep 1
        done
        echo "Redis is up. Starting Mintd..."
        cargo -C roles/mint -Z unstable-options run -- -c $DEVENV_ROOT/roles/mint/config/mint.config.toml
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
