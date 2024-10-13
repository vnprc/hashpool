{
  pkgs,
  lib,
  ...
}: let
  src = pkgs.fetchFromGitHub {
    owner = "Sjors";
    repo = "bitcoin";
    rev = "sv2";
    hash = "sha256-iPVtR06DdheYRfZ/Edm1hu3JLoXAu5obddTQ38cqljs=";
  };
in
  pkgs.bitcoind.overrideAttrs (oldAttrs: {
    name = "bitcoind-sv2";
    src = src;
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
  })
