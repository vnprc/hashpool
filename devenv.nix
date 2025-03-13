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
  withLogging = command: logFile: "mkdir -p ${config.devenv.root}/logs && ${command} 2>&1 | tee -a ${config.devenv.root}/logs/${logFile}";
in {
  env.BITCOIND_DATADIR = config.devenv.root + "/.devenv/state/bitcoind";

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
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [pkgs.darwin.apple_sdk.frameworks.Security];

  # https://devenv.sh/languages/
  languages.rust.enable = true;

  # https://devenv.sh/processes/
  processes = {
    run-local-pool = {exec = withLogging "run-local-pool" "run-local-pool.log";};
    run-job-server = {exec = withLogging "run-job-server" "run-job-server.log";};
    run-job-client = {exec = withLogging "run-job-client" "run-job-client.log";};
    run-translator-proxy = {exec = withLogging "run-translator-proxy" "run-translator-proxy.log";};
    bitcoind-testnet = {exec = withLogging "bitcoind-testnet" "bitcoind-testnet.log";};
    run-miner = {
      exec = withLogging ''
        echo "Waiting for translator proxy on port 34255..."
        while ! nc -z localhost 34255; do
          sleep 1
        done
        echo "Translator proxy is up, starting miner..."
        cd roles/test-utils/mining-device-sv1
        cargo run
      '' "run-miner.log";
    };
  };

  # https://devenv.sh/scripts/
  scripts = {
    run-local-pool.exec = withLogging "cargo -C roles/pool -Z unstable-options run -- -c $DEVENV_ROOT/roles/pool/config-examples/pool-config-local-tp-example.toml" "run-local-pool.log";
    run-job-server.exec = withLogging "cargo -C roles/jd-server -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-server/config-examples/jds-config-local-example.toml" "run-job-server.log";
    run-job-client.exec = withLogging "cargo -C roles/jd-client -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-client/config-examples/jdc-config-local-example.toml" "run-job-client.log";
    run-translator-proxy.exec = withLogging "cargo -C roles/translator -Z unstable-options run -- -c $DEVENV_ROOT/roles/translator/config-examples/tproxy-config-local-jdc-example.toml" "run-translator-proxy.log";
    bitcoind-testnet.exec = withLogging "bitcoind -testnet4 -sv2 -sv2port=8442 -debug=sv2 -conf=$DEVENV_ROOT/bitcoin.conf -datadir=$BITCOIND_DATADIR" "bitcoind-testnet.log";
  };

  pre-commit.hooks = {
    alejandra.enable = true;
  };

  enterShell = ''
    echo Just
    echo ====
    just --list
    echo
    echo Helper scripts
    echo ==============
    echo
    ${pkgs.gnused}/bin/sed -e 's| |••|g' -e 's|=| |' <<EOF | ${pkgs.util-linuxMinimal}/bin/column -t | ${pkgs.gnused}/bin/sed -e 's|^| |' -e 's|••| |g'
    ${lib.generators.toKeyValue {} (lib.mapAttrs (name: value: value.description) config.scripts)}
    EOF
    echo
  '';
}
