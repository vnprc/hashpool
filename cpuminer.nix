{ pkgs, ...}:
let
  src = pkgs.fetchFromGitHub {
    owner = "pooler";
    repo = "cpuminer";
    rev = "v2.5.1";
    hash = "sha256-ERBcFKAWsNW2UbqAfEaRsgIMcBfp+ggFA6LFjD6IhDg=";
  };
in pkgs.stdenv.mkDerivation {
  name = "cpuminer";
  inherit src;
  buildInputs = [ pkgs.autoconf pkgs.automake pkgs.curl ];
  configurePhase = ''
    ./autogen.sh
    ./configure CFLAGS="-O3" --disable-assembly
  '';
   buildPhase = ''
      make
   '';
   installPhase = ''
     mkdir -p $out/bin
     cp minerd $out/bin
   '';
}
