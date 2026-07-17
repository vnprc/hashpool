# Pin-free adapter over the bitcoind-gunix flake.
#
# `gunix` = bitcoind-gunix.packages.<system> from the CALLER's own lock
# (flake.nix passes its flake input; devenv.nix passes inputs.bitcoind-gunix.…).
# This file carries NO version string, NO hash, and NO URL: every artifact is
# derived from gunix's `tarball` derivation, which is the one output that runs
# the byte-for-byte parity gate against the official GUIX release. On the v31.1
# branch there is no per-binary gate, so `release` and `patched` MUST take
# `gunix.tarball` (not `gunix.bitcoind`) as their src — that makes every
# `nix build` of a node attr force the parity gate transitively.
#
# The gate-verified tarball unpacks to a single top-level `bitcoin-<version>/`
# dir, which stdenv auto-detects as sourceRoot — so no version string is needed
# to find the files inside.
{
  pkgs,
  lib,
  gunix,
}:
if gunix == null
then
  throw ''
    bitcoin-node-gunix: `gunix` is null — the bitcoind-gunix flake input is not
    available. Add the input and pin its lock:

      * devenv.nix path: add to devenv.yaml
            inputs:
              bitcoind-gunix:
                url: github:0xB10C/bitcoind-gunix/v31.1
        then run `devenv update`.
      * flake.nix path: this should be unreachable — the caller guards with
        `bitcoind-gunix.packages ? <system>`, so a null here means an
        unsupported system.
  ''
else let
  # The parity-gated archive; re-exported verbatim ($out is the .tar.gz file,
  # whose name embeds the version, e.g. bitcoin-31.1-x86_64-linux-gnu.tar.gz).
  tarball = gunix.tarball;

  # The three multiprocess files the node needs at runtime. The `bitcoin`
  # wrapper resolves `bitcoin-node` via ../libexec/ relative to itself, so
  # bin/ and libexec/ must stay siblings in the output.
  installNode = ''
    mkdir -p $out/bin $out/libexec
    cp bin/bitcoin $out/bin/
    cp bin/bitcoin-cli $out/bin/
    cp libexec/bitcoin-node $out/libexec/
  '';

  nodeMeta = {
    description = "Bitcoin Core with multiprocess IPC support (bitcoin-node), byte-verified via the bitcoind-gunix flake";
    homepage = "https://github.com/0xB10C/bitcoind-gunix";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };

  # Deploy variant: the official release bytes, untouched. `dontFixup` disables
  # strip/patchelf/shebang rewrites, so the binaries stay byte-identical to the
  # tarball and keep the FHS interpreter /lib64/ld-linux-x86-64.so.2 — which is
  # exactly what the deploy scripts' check_nix_abi gate requires (and which a
  # patched /nix/store interpreter would fail).
  release = pkgs.stdenv.mkDerivation {
    name = "bitcoin-node-release";
    src = tarball;
    nativeBuildInputs = [pkgs.gnutar pkgs.gzip];
    dontConfigure = true;
    dontBuild = true;
    dontFixup = true;
    installPhase = installNode;
    meta = nodeMeta;
  };

  # Dev / NixOS-runnable variant: the same three files made to run on NixOS via
  # autoPatchelfHook (rewrites the interpreter into /nix/store and links
  # libstdc++ from gcc.cc.lib). This mirrors the treatment the retired
  # fetchurl-based node package applied to the identical official tarball.
  patched = pkgs.stdenv.mkDerivation {
    name = "bitcoin-node";
    src = tarball;
    nativeBuildInputs = [
      pkgs.gnutar
      pkgs.gzip
      pkgs.autoPatchelfHook
      pkgs.gcc.cc.lib
    ];
    dontConfigure = true;
    dontBuild = true;
    installPhase = installNode;
    meta = nodeMeta;
  };
in {
  inherit tarball release patched;
}
