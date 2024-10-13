{ pkgs, lib, config, inputs, ... }:
let
  minerd = import ./cpuminer.nix {inherit pkgs;};
  # override bitcoin's src attribute to get version 28
  bitcoind = pkgs.bitcoind.overrideAttrs (oldAttrs: {
  name = "bitcoind-sv2";
  src = pkgs.fetchFromGitHub {
    owner = "Sjors";
    repo = "bitcoin";
    rev = "sv2";
    hash = "sha256-iPVtR06DdheYRfZ/Edm1hu3JLoXAu5obddTQ38cqljs=";
  };
  # ugly, drops autoconfHook as first list item
  nativeBuildInputs = lib.lists.drop 1 oldAttrs.nativeBuildInputs ++ [pkgs.cmake];
  # doCheck = false;
  postInstall = "";
  cmakeFlags = [
    (lib.cmakeBool "WITH_SV2" true)
    (lib.cmakeBool "BUILD_BENCH" true)
    (lib.cmakeBool "BUILD_TESTS" true)
    (lib.cmakeBool "ENABLE_WALLET" false)
    (lib.cmakeBool "BUILD_GUI" false)
    (lib.cmakeBool "BUILD_GUI_TESTS" false)
  ];
  });
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
