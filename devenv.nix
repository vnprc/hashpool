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
  };

  # Function to add logging logic to any command
  withLogging = command: logFile: "mkdir -p ${config.devenv.root}/logs && bash -c 'stdbuf -oL ${command} 2>&1 | tee -a ${config.devenv.root}/logs/${logFile}'";

  # Get all process names dynamically
  processNames = lib.attrNames config.processes;
in {
  env.BITCOIND_DATADIR = config.devenv.root + "/.devenv/state/bitcoind";

  # Ensure logs directory exists before processes run
  tasks.createlogsdir = {
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
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [pkgs.darwin.apple_sdk.frameworks.Security];

  # https://devenv.sh/languages/
  languages.rust.enable = true;

  # https://devenv.sh/processes/
  processes = {
    local-pool = {exec = withLogging "cargo -C roles/pool -Z unstable-options run -- -c $DEVENV_ROOT/roles/pool/config-examples/pool-config-local-tp-example.toml" "local-pool.log";};
    job-server = {exec = withLogging "cargo -C roles/jd-server -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-server/config-examples/jds-config-local-example.toml" "job-server.log";};
    job-client = {exec = withLogging "cargo -C roles/jd-client -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-client/config-examples/jdc-config-local-example.toml" "job-client.log";};
    translator-proxy = {exec = withLogging "cargo -C roles/translator -Z unstable-options run -- -c $DEVENV_ROOT/roles/translator/config-examples/tproxy-config-local-jdc-example.toml" "translator-proxy.log";};
    bitcoind-testnet = {exec = withLogging "bitcoind -testnet4 -sv2 -sv2port=8442 -debug=sv2 -conf=$DEVENV_ROOT/bitcoin.conf -datadir=$BITCOIND_DATADIR" "bitcoind-testnet.log";};
    miner = {
      exec = withLogging ''
        echo "Waiting for translator proxy on port 34255..."
        while ! nc -z localhost 34255; do
          sleep 1
        done
        echo "Translator proxy is up, starting miner..."
        cd roles/test-utils/mining-device-sv1
        while true; do
          RUST_LOG=debug stdbuf -oL cargo run 2>&1 | tee -a ${config.devenv.root}/logs/miner.log
          echo "Miner crashed. Restarting..." >> ${config.devenv.root}/logs/miner.log
          sleep 5
        done
      '' "miner.log";
    };
  };

  pre-commit.hooks = {
    alejandra.enable = true;
  };

  enterShell = ''
    just --list || true
  '';
}
