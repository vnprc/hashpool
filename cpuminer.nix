{pkgs, ...}: let
  src = pkgs.fetchFromGitHub {
    owner = "pooler";
    repo = "cpuminer";
    rev = "v2.5.1";
    hash = "sha256-ERBcFKAWsNW2UbqAfEaRsgIMcBfp+ggFA6LFjD6IhDg=";
  };
  configurePhase =
    if pkgs.stdenv.isDarwin
    then ''
      ./autogen.sh
      ./configure CFLAGS="-O3" --disable-assembly
    ''
    else ''
      ./autogen.sh
      ./configure CFLAGS="-O3"
    '';
in
  pkgs.stdenv.mkDerivation {
    name = "cpuminer";
    src = src;
    buildInputs = [pkgs.autoconf pkgs.automake pkgs.curl];
    configurePhase = configurePhase;
    buildPhase = ''
      make
    '';
    installPhase = ''
      mkdir -p $out/bin
      cp minerd $out/bin
    '';
  }
