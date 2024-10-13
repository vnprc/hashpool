{ pkgs, lib, config, inputs, ... }:
let
  minerd = import ./cpuminer.nix {inherit pkgs;};
  bitcoind = import ./bitcoind.nix {pkgs=pkgs; lib=lib;};
in {

  env.BITCOIND_DATADIR = config.devenv.root + "/.devenv/state/bitcoind";

  # https://devenv.sh/packages/
  packages = [ bitcoind minerd ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.darwin.apple_sdk.frameworks.Security ];

  # https://devenv.sh/languages/
  languages.rust.enable = true;

  # https://devenv.sh/processes/
  processes = {
     run-local-pool.exec = "run-local-pool";
     run-job-server.exec = "run-job-server";
     run-job-client.exec = "run-job-client";
     run-translator-proxy.exec = "run-translator-proxy";
     bitcoind-testnet.exec = "bitcoind-testnet";
     run-minerd.exec = "run-minerd";
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
     run-minerd.exec = "minerd -a sha256d -o stratum+tcp://localhost:34255 -q -D -P";
  };

  # https://devenv.sh/tasks/
  # https://devenv.sh/tests/
  # https://devenv.sh/pre-commit-hooks/
  # See full reference at https://devenv.sh/reference/options/

  tasks."bitcoind:make_datadir" = {
    exec = ''mkdir -p $BITCOIND_DATADIR'';
    before = [ "devenv:enterShell" ];
  };
}
