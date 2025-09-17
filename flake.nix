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

    # Common build configuration
    common = {
      # Include whole repo so protocols/ path deps are available
      src = craneLib.path ./.;
      
      # Point to workspace manifest
      cargoToml = ./roles/Cargo.toml;
      cargoLock = ./roles/Cargo.lock;
      
      # Change to roles directory after unpacking so cargo finds the workspace
      postUnpack = ''
        sourceRoot="$sourceRoot/roles"
      '';
      
      # Will need to replace with actual hash on first build
      cargoVendorHash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
      
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
        protobuf
      ];

      # Set environment variables for builds
      OPENSSL_NO_VENDOR = "1";
      PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig:${pkgs.sqlite.dev}/lib/pkgconfig";
      RUST_BACKTRACE = "1";
    };

    # Build dependency graph once, reused by all packages
    cargoArtifacts = craneLib.buildDepsOnly (common // {
      pname = "hashpool-deps";
      version = "0.1.0";
    });

    # Individual packages - one per binary
    mintPackage = craneLib.buildPackage (common // {
      inherit cargoArtifacts;
      pname = "mint";
      version = "0.1.0";
      cargoExtraArgs = "--package mint --bin mint";
    });

    poolSv2Package = craneLib.buildPackage (common // {
      inherit cargoArtifacts;
      pname = "pool_sv2";
      version = "0.1.0";
      cargoExtraArgs = "--package pool --bin pool";
    });

    translatorPackage = craneLib.buildPackage (common // {
      inherit cargoArtifacts;
      pname = "translator_sv2";
      version = "0.1.0";
      cargoExtraArgs = "--package translator --bin translator_sv2";
    });

    jdServerPackage = craneLib.buildPackage (common // {
      inherit cargoArtifacts;
      pname = "jd_server";
      version = "0.1.0";
      cargoExtraArgs = "--package jd-server --bin jd_server";
    });

    jdClientPackage = craneLib.buildPackage (common // {
      inherit cargoArtifacts;
      pname = "jd_client";
      version = "0.1.0";
      cargoExtraArgs = "--package jd-client --bin jd_client";
    });

    # Wrapper scripts for orchestrating services
    hashpool-serverd = pkgs.writeShellApplication {
      name = "hashpool-serverd";
      runtimeInputs = with pkgs; [ netcat-gnu python3 ];
      text = builtins.replaceStrings 
        ["@mint@" "@pool_sv2@" "@jd_server@"]
        ["${mintPackage}" "${poolSv2Package}" "${jdServerPackage}"]
        (builtins.readFile ./nix/wrappers/hashpool-serverd.sh);
    };
    
    hashpool-clientd = pkgs.writeShellApplication {
      name = "hashpool-clientd";
      runtimeInputs = with pkgs; [ netcat-gnu python3 ];
      text = builtins.replaceStrings
        ["@translator_sv2@" "@jd_client@"]
        ["${translatorPackage}" "${jdClientPackage}"]
        (builtins.readFile ./nix/wrappers/hashpool-clientd.sh);
    };

  in {
    # Packages
    packages = {
      default = hashpool-serverd;
      mint = mintPackage;
      pool_sv2 = poolSv2Package;
      translator_sv2 = translatorPackage;
      jd_server = jdServerPackage;
      jd_client = jdClientPackage;
      hashpool-serverd = hashpool-serverd;
      hashpool-clientd = hashpool-clientd;
      
      # Additional checks
      clippy = craneLib.cargoClippy (common // {
        inherit cargoArtifacts;
        pname = "hashpool-clippy";
        version = "0.1.0";
        cargoClippyExtraArgs = "--all-targets -- --deny warnings";
      });

      # Documentation
      doc = craneLib.cargoDoc (common // {
        inherit cargoArtifacts;
        pname = "hashpool-doc";
        version = "0.1.0";
      });

      # Unit tests
      test = craneLib.cargoTest (common // {
        inherit cargoArtifacts;
        pname = "hashpool-test";
        version = "0.1.0";
      });
    };

    # Development apps
    apps = {
      default = flake-utils.lib.mkApp {
        drv = hashpool-serverd;
        exePath = "/bin/hashpool-serverd";
      };
      
      mint = flake-utils.lib.mkApp {
        drv = mintPackage;
        exePath = "/bin/mint";
      };

      translator = flake-utils.lib.mkApp {
        drv = translatorPackage;
        exePath = "/bin/translator_sv2";
      };
      
      hashpool-serverd = flake-utils.lib.mkApp {
        drv = hashpool-serverd;
        exePath = "/bin/hashpool-serverd";
      };
      
      hashpool-clientd = flake-utils.lib.mkApp {
        drv = hashpool-clientd;
        exePath = "/bin/hashpool-clientd";
      };
    };
  }) // {
    # NixOS modules and configurations
    nixosModules = {
      hashpool = import ./nix/modules/hashpool.nix;
    };

    nixosConfigurations = {
      hashpool-poc = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          ./hosts/poc.nix
          self.nixosModules.hashpool
          ({ pkgs, ... }: {
            nixpkgs.overlays = [
              rust-overlay.overlays.default
              (final: prev: {
                hashpool-serverd = self.packages.x86_64-linux.hashpool-serverd;
                hashpool-clientd = self.packages.x86_64-linux.hashpool-clientd;
              })
            ];
          })
        ];
      };
    };
  };
}