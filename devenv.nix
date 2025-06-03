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

  poolConfigFile = ./pool.toml;
  minerConfigFile = ./miner.toml;
  poolConfig = builtins.fromTOML (builtins.readFile poolConfigFile);
  minerConfig = builtins.fromTOML (builtins.readFile minerConfigFile);

  # poolSide: mining pool and related service configs
  poolSideRedisConfig = poolConfig.redis;
  poolSideMintConfig = poolConfig.mint;
  poolSidePoolConfig = poolConfig.pool;
  poolSideProxyConfig = poolConfig.proxy;

  POOLSIDE_REDIS_PORT = toString poolSideRedisConfig.port;
  POOLSIDE_MINT_PORT = toString poolSideMintConfig.port;
  POOLSIDE_POOL_PORT = toString poolSidePoolConfig.port;
  POOLSIDE_PROXY_PORT = toString poolSideProxyConfig.port;

  # minerSide: translator proxy and related service configs
  minerSideMintConfig = minerConfig.mint;
  minerSidePoolConfig = minerConfig.pool;
  minerSideProxyConfig = minerConfig.proxy;
  
  MINERSIDE_MINT_PORT = toString minerSideMintConfig.port;
  MINERSIDE_POOL_PORT = toString minerSidePoolConfig.port;
  MINERSIDE_PROXY_PORT = toString minerSideProxyConfig.port;

  # Function to add logging logic to any command
  withLogging = command: logFile: ''
    mkdir -p ${config.devenv.root}/logs
    sh -c ${lib.escapeShellArg command} 2>&1 | stdbuf -oL tee -a ${config.devenv.root}/logs/${logFile}
  '';

  # Get all process names dynamically
  processNames = lib.attrNames config.processes;
in {
  env.POOLSIDE_REDIS_URL = poolSideRedisConfig.url;
  env.POOLSIDE_REDIS_HOST = poolSideRedisConfig.host;
  env.POOLSIDE_REDIS_PORT = builtins.toString poolSideRedisConfig.port;

  env.POOLSIDE_MINT_PORT = builtins.toString poolSideMintConfig.port;
  env.POOLSIDE_POOL_PORT = builtins.toString poolSidePoolConfig.port;
  env.POOLSIDE_PROXY_PORT = builtins.toString poolSideProxyConfig.port;

  env.MINERSIDE_MINT_PORT = builtins.toString minerSideMintConfig.port;
  env.MINERSIDE_POOL_PORT = builtins.toString minerSidePoolConfig.port;
  env.MINERSIDE_PROXY_PORT = builtins.toString minerSideProxyConfig.port;

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
    redis = {exec = withLogging "mkdir -p ${config.devenv.root}/.devenv/state/redis && redis-server --dir ${config.devenv.root}/.devenv/state/redis --port ${POOLSIDE_REDIS_PORT}" "redis.log";};
    pool = {
      exec = withLogging ''
        echo "Waiting for Mint..."
        while ! nc -z localhost ${POOLSIDE_MINT_PORT}; do
          sleep 1
        done
        echo "Mint is up. Starting Local Pool..."
        cargo -C roles/pool -Z unstable-options run -- \
          -c $DEVENV_ROOT/roles/pool/config-examples/pool-config-local-tp-example.toml \
          -g $DEVENV_ROOT/pool.toml
      '' "pool.log";
    };
    jd-server = {
      exec = withLogging ''
        echo "Waiting for Pool..."
        while ! nc -z localhost ${POOLSIDE_POOL_PORT}; do
          sleep 1
        done
        echo "Pool is up. Starting Job Server..."
        cargo -C roles/jd-server -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-server/config-examples/jds-config-local-example.toml
      '' "jd-server.log";
    };
    jd-client = {exec = withLogging "cargo -C roles/jd-client -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-client/config-examples/jdc-config-local-example.toml" "job-client.log";};
    # TODO switch to miner config
    proxy = {
      exec = withLogging ''
        echo "Waiting for Pool..."
        while ! nc -z localhost ${MINERSIDE_POOL_PORT}; do
          sleep 1
        done
        echo "Pool is up. Starting Proxy..."
        cargo -C roles/translator -Z unstable-options run -- \
          -c $DEVENV_ROOT/roles/translator/config-examples/tproxy-config-local-jdc-example.toml \
          -g $DEVENV_ROOT/pool.toml
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
        while ! nc -z localhost ${MINERSIDE_PROXY_PORT}; do
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
        echo "Waiting for Redis on port ${POOLSIDE_REDIS_PORT}..."
        while ! nc -z localhost ${POOLSIDE_REDIS_PORT}; do
          sleep 1
        done
        echo "Redis is up. Starting Mint..."
        cargo -C roles/mint -Z unstable-options run -- \
          -c $DEVENV_ROOT/roles/mint/config/mint.config.toml \
          -g $DEVENV_ROOT/pool.toml
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
