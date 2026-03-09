{
  pkgs,
  lib,
  stdenv,
  ...
}: let
  version = "30.2";

  # Detect platform for selecting correct binary tarball
  platform =
    if stdenv.isDarwin
    then
      if stdenv.isAarch64
      then "arm64-apple-darwin-unsigned"
      else "x86_64-apple-darwin-unsigned"
    else if stdenv.isLinux
    then
      if stdenv.isx86_64
      then "x86_64-linux-gnu"
      else "aarch64-linux-gnu"
    else throw "Unsupported platform";

  # Platform-specific hashes for the official Bitcoin Core binary release
  hashes = {
    "x86_64-linux-gnu" = "sha256-aqe7T+tpnExiYt0j5ABBkfbffzc7XVl4tbzdS7cvddg=";
    "aarch64-linux-gnu" = "sha256-TODO";
    "x86_64-apple-darwin-unsigned" = "sha256-TODO";
    "arm64-apple-darwin-unsigned" = "sha256-TODO";
  };

  binaryUrl = "https://bitcoincore.org/bin/bitcoin-core-${version}/bitcoin-${version}-${platform}.tar.gz";

  binary = pkgs.fetchurl {
    url = binaryUrl;
    hash = hashes.${platform};
  };
in
  pkgs.stdenv.mkDerivation {
    name = "bitcoin-node";
    version = version;
    src = binary;

    nativeBuildInputs =
      [pkgs.gnutar pkgs.gzip]
      ++ lib.optionals stdenv.isLinux [
        pkgs.autoPatchelfHook
        pkgs.gcc.cc.lib
      ];

    sourceRoot = "bitcoin-${version}";

    dontBuild = true;
    dontConfigure = true;

    # Install the multiprocess wrapper (bitcoin), the CLI, and the node component.
    # The 'bitcoin' wrapper locates 'bitcoin-node' via ../libexec/ relative to itself,
    # so the layout $out/bin/bitcoin + $out/libexec/bitcoin-node must be preserved.
    installPhase = ''
      mkdir -p $out/bin $out/libexec
      cp bin/bitcoin $out/bin/
      cp bin/bitcoin-cli $out/bin/
      cp libexec/bitcoin-node $out/libexec/
    '' + lib.optionalString stdenv.isDarwin ''
      /usr/bin/codesign -s - $out/bin/bitcoin
      /usr/bin/codesign -s - $out/bin/bitcoin-cli
      /usr/bin/codesign -s - $out/libexec/bitcoin-node
    '';

    meta = {
      description = "Bitcoin Core with multiprocess IPC support (bitcoin-node)";
      homepage = "https://bitcoincore.org";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
    };
  }
