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
in {
  env.BITCOIND_DATADIR = config.devenv.root + "/.devenv/state/bitcoind";

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
    run-local-pool.exec = "run-local-pool";
    run-job-server.exec = "run-job-server";
    run-job-client.exec = "run-job-client";
    run-translator-proxy.exec = "run-translator-proxy";
    bitcoind-testnet.exec = "bitcoind-testnet";
    run-miner.exec = ''
      echo "Waiting for translator proxy on port 34255..."
      while ! nc -z localhost 34255; do
        sleep 1
      done
      echo "Translator proxy is up, starting miner..."
      cd roles/test-utils/mining-device-sv1 && cargo run
    '';
  };

  # https://devenv.sh/basics/
  # https://devenv.sh/services/
  # services.postgres.enable = true;
  # https://devenv.sh/scripts/
  scripts = {
    run-local-pool.exec = "cargo -C roles/pool -Z unstable-options run -- -c $DEVENV_ROOT/roles/pool/config-examples/pool-config-local-tp-example.toml";
    run-job-server.exec = "cargo -C roles/jd-server -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-server/config-examples/jds-config-local-example.toml";
    run-job-client.exec = "cargo -C roles/jd-client -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-client/config-examples/jdc-config-local-example.toml";
    run-translator-proxy.exec = "cargo -C roles/translator -Z unstable-options run -- -c $DEVENV_ROOT/roles/translator/config-examples/tproxy-config-local-jdc-example.toml";
    bitcoind-testnet.exec = "bitcoind -testnet4 -sv2 -sv2port=8442 -debug=sv2 -conf=$DEVENV_ROOT/bitcoin.conf -datadir=$BITCOIND_DATADIR";
  };

  tasks."bitcoind:make_datadir" = {
    exec = ''mkdir -p $BITCOIND_DATADIR'';
    before = ["devenv:enterShell"];
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
