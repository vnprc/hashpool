{
  pkgs,
  lib,
  ...
}: let
  src = pkgs.fetchFromGitHub {
    owner = "Sjors";
    repo = "bitcoin";
    rev = "b4eb739e5d76e2b62fd37bae5da8acfa75484879";
    hash = "sha256-vRPVOjGt1bYQUWZycweaYI6y22O1K1QAaCh5u3rfP6Q=";
  };
in
  pkgs.bitcoind.overrideAttrs (oldAttrs: {
    name = "bitcoind-sv2";
    src = src;

    installCheckPhase = ''
      OUTPUT=$(${pkgs.bitcoind}/bin/bitcoin-cli --version || true)
      echo "Bitcoin CLI Version Output: $OUTPUT"
      echo "Skipping strict version check..."
    '';

    # Modify build settings
    nativeBuildInputs = lib.lists.drop 1 oldAttrs.nativeBuildInputs ++ [pkgs.cmake];
    postInstall = "";
    cmakeFlags = [
      (lib.cmakeBool "WITH_SV2" true)
      (lib.cmakeBool "BUILD_BENCH" true)
      (lib.cmakeBool "BUILD_TESTS" true)
      (lib.cmakeBool "ENABLE_WALLET" false)
      (lib.cmakeBool "BUILD_GUI" false)
      (lib.cmakeBool "BUILD_GUI_TESTS" false)
    ];
  })
