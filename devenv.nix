{ pkgs, lib, config, inputs, ... }:
let
  minerd = import ./cpuminer.nix {inherit pkgs;};
  # override bitcoin's src attribute to get version 28
  bitcoind = pkgs.bitcoind.overrideAttrs (final: rec {
    version = "28.0";
    src = pkgs.fetchurl {
      urls = [
        "https://bitcoincore.org/bin/bitcoin-core-${version}/bitcoin-${version}.tar.gz"
      ];
      sha256 = "sha256-cAri0eIEYC6wfyd5puZmmJO8lsDcopBZP4D/jhAv838=";
  };});
in {
  # https://devenv.sh/packages/
  packages = [ bitcoind minerd ];

  # https://devenv.sh/languages/
  languages.rust.enable = true;

  # https://devenv.sh/processes/
  processes = {
     run-local-pool.exec = "cargo -C roles/pool -Z unstable-options run -- -c roles/pool/config-examples/pool-config-local-tp-example.toml";
     run-job-server.exec = "cargo -C roles/jd-server -Z unstable-options run -- -c roles/jd-server/config-examples/jds-config-local-example.toml";
     run-job-client.exec = "cargo -C roles/jd-client -Z unstable-options run -- -c roles/jd-client/config-examples/jds-config-local-example.toml";
     run-translator-proxy.exec = "cargo -C roles/translator -Z unstable-options run -- -c roles/translator/config-examples/tproxy-config-local-jdc-example.toml";
     bitcoind-testnet.exec = "bitcoind -daemon -testnet4 -sv2 -sv2port=8442 -debug=sv2";
     run-minerd.exec = "minerd -a sha256d -o stratum+tcp://localhost:34255 -q -D -P";
  };

  # https://devenv.sh/basics/
  # https://devenv.sh/services/
  # services.postgres.enable = true;
  # https://devenv.sh/scripts/
  # https://devenv.sh/tasks/
  # https://devenv.sh/tests/
  # https://devenv.sh/pre-commit-hooks/
  # See full reference at https://devenv.sh/reference/options/
}
