{
  description = "Hashpool - Stratum V2 with Cashu Ecash Mining Pool";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
  };

  outputs = inputs@{
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    crane,
    ...
  }: let
    # Pin Rust version to ensure reproducible builds
    rustVersion = "1.87.0";
    
    # Pin key dependency versions to eliminate conflicts
    pinnedVersions = {
      bitcoin = "0.32.6";
      bitcoin_hashes = "0.14.0";
      secp256k1 = "0.29.1";
      bip39 = "2.2.0";
      tokio = "1.42.1";
      serde = "1.0.219";
      anyhow = "1.0.98";
      tracing = "0.1.41";
    };

  in flake-utils.lib.eachDefaultSystem (system: let
    pkgs = import nixpkgs {
      inherit system;
      overlays = [
        rust-overlay.overlays.default
      ];
    };

    craneLib = crane.mkLib pkgs;

    # Custom Rust toolchain with specific version
    rust-toolchain = pkgs.rust-bin.stable.${rustVersion}.default.override {
      extensions = ["rust-src" "rustfmt" "clippy" "rust-analyzer"];
    };

    # Common arguments for all builds
    commonArgs = {
      src = craneLib.cleanCargoSource (craneLib.path ./roles);
      strictDeps = true;
      
      buildInputs = with pkgs; [
        openssl
        sqlite
        pkg-config
      ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
        pkgs.libiconv
        pkgs.darwin.apple_sdk.frameworks.Security
      ];

      nativeBuildInputs = with pkgs; [
        pkg-config
        rust-toolchain
      ];

      # Set environment variables for builds
      OPENSSL_NO_VENDOR = "1";
      PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig:${pkgs.sqlite.dev}/lib/pkgconfig";
      RUST_BACKTRACE = "1";
    };

    # Build workspace dependencies
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;

    # Individual packages
    poolPackage = craneLib.buildPackage (commonArgs // {
      inherit cargoArtifacts;
      pname = "pool";
      cargoExtraArgs = "--bin pool";
    });

    mintPackage = craneLib.buildPackage (commonArgs // {
      inherit cargoArtifacts;
      pname = "mint";
      cargoExtraArgs = "--bin mint";
    });

    translatorPackage = craneLib.buildPackage (commonArgs // {
      inherit cargoArtifacts;
      pname = "translator_sv2";
      cargoExtraArgs = "--bin translator_sv2";
    });

  in {
    # Packages
    packages = {
      default = poolPackage;
      pool = poolPackage;
      mint = mintPackage;
      translator = translatorPackage;
      
      # Additional checks
      clippy = craneLib.cargoClippy (commonArgs // {
        inherit cargoArtifacts;
        cargoClippyExtraArgs = "--all-targets -- --deny warnings";
      });

      # Documentation
      doc = craneLib.cargoDoc (commonArgs // {
        inherit cargoArtifacts;
      });

      # Unit tests
      test = craneLib.cargoTest (commonArgs // {
        inherit cargoArtifacts;
      });
    };

    # Development apps
    apps = {
      pool = flake-utils.lib.mkApp {
        drv = poolPackage;
        exePath = "/bin/pool";
      };
      
      mint = flake-utils.lib.mkApp {
        drv = mintPackage;
        exePath = "/bin/mint";
      };

      translator = flake-utils.lib.mkApp {
        drv = translatorPackage;
        exePath = "/bin/translator_sv2";
      };
    };
  });
}