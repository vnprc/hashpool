{
  pkgs,
  lib,
  stdenv,
  ...
}: let
  version = "1.0.6";

  # Detect platform for selecting correct binary tarball
  platform =
    if stdenv.isLinux
    then
      if stdenv.isx86_64
      then "x86_64-linux-gnu"
      else "aarch64-linux-gnu"
    else if stdenv.isDarwin
    then
      if stdenv.isAarch64
      then "arm64-apple-darwin"
      else "x86_64-apple-darwin"
    else throw "Unsupported platform";

  # Platform-specific hashes for the sv2-tp binary release
  hashes = {
    "x86_64-linux-gnu" = "sha256-qTGAaWER2MlIQ27D3PdfDFQbeQAnAs6DqjZcOmR7Uk0=";
    "aarch64-linux-gnu" = "sha256-TODO";
    "arm64-apple-darwin" = "sha256-TODO";
    "x86_64-apple-darwin" = "sha256-TODO";
  };

  binaryUrl = "https://github.com/stratum-mining/sv2-tp/releases/download/v${version}/sv2-tp-${version}-${platform}.tar.gz";

  binary = pkgs.fetchurl {
    url = binaryUrl;
    hash = hashes.${platform};
  };
in
  pkgs.stdenv.mkDerivation {
    name = "sv2-tp";
    version = version;
    src = binary;

    nativeBuildInputs =
      [pkgs.gnutar pkgs.gzip]
      ++ lib.optionals stdenv.isLinux [
        pkgs.autoPatchelfHook
        pkgs.gcc.cc.lib
      ]
      ++ lib.optionals stdenv.isDarwin [
        pkgs.libiconv
      ];

    sourceRoot = "sv2-tp-${version}";

    dontBuild = true;
    dontConfigure = true;

    installPhase = ''
      mkdir -p $out/bin
      cp bin/sv2-tp $out/bin/
    '' + lib.optionalString stdenv.isDarwin ''
      /usr/bin/codesign -s - $out/bin/sv2-tp
    '';

    meta = {
      description = "Stratum V2 Template Provider (connects to Bitcoin Core via IPC)";
      homepage = "https://github.com/stratum-mining/sv2-tp";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
    };
  }
