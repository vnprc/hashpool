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

  outputs = inputs @ {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    crane,
    ...
  }:
    # Per-system packages and apps
    (flake-utils.lib.eachDefaultSystem (system: let
      # Pin Rust version to ensure reproducible builds.
      # Note: home@0.5.12 and time@0.3.47 require Rust >=1.88. Since this pins 1.87,
      # roles/Cargo.lock must keep home=0.5.11 and time=0.3.41 (pinned via `cargo update
      # --precise`). Bump rustVersion to "1.88.0" and re-run the cargo updates to remove
      # those workarounds once the nixpkgs Rust version also reaches 1.88.
      rustVersion = "1.87.0";

      pkgs = import nixpkgs {
        inherit system;
        overlays = [
          rust-overlay.overlays.default
        ];
      };

      lib = pkgs.lib;

      craneLib = crane.mkLib pkgs;

      # Custom Rust toolchain with specific version
      rust-toolchain = pkgs.rust-bin.stable.${rustVersion}.default.override {
        extensions = ["rust-src" "rustfmt" "clippy" "rust-analyzer"];
      };

      # Source filter: include cargo sources from all workspace-relevant directories.
      # The roles/ workspace has path deps pointing to protocols/, common/, utils/, and test/,
      # so all must be present in the build sandbox. postUnpack sets sourceRoot to roles/
      # so cargo sees the correct workspace root while resolving relative path deps.
      workspaceSrc = lib.cleanSourceWith {
        src = craneLib.path ./.;
        filter = path: type:
          (craneLib.filterCargoSources path type)
          || (lib.hasInfix "/protocols/" path)
          || (lib.hasInfix "/roles/" path)
          || (lib.hasInfix "/common/" path)
          || (lib.hasInfix "/utils/" path)
          || (lib.hasInfix "/test/" path);
      };

      # Common arguments for all Rust builds
      commonArgs = {
        src = workspaceSrc;

        # The workspace Cargo.toml is at roles/, not at the repo root.
        # cargoToml lets crane read pname/version at eval time.
        # postUnpack sets the build-time working directory to roles/ so cargo
        # finds Cargo.toml/Cargo.lock and path deps like ../../protocols/ehash resolve correctly.
        cargoToml = ./roles/Cargo.toml;
        postUnpack = ''sourceRoot="$sourceRoot/roles"'';

        strictDeps = true;

        buildInputs = with pkgs;
          [
            openssl
            sqlite
            pkg-config
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.libiconv
            pkgs.darwin.apple_sdk.frameworks.Security
          ];

        nativeBuildInputs = with pkgs; [
          pkg-config
          rust-toolchain
        ];

        OPENSSL_NO_VENDOR = "1";
        PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig:${pkgs.sqlite.dev}/lib/pkgconfig";
        RUST_BACKTRACE = "1";
      };

      # Build workspace dependencies (shared across all binary builds)
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      # --- Hashpool role packages ---

      poolPackage = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "pool";
          cargoExtraArgs = "--bin pool";
        });

      mintPackage = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "mint";
          cargoExtraArgs = "--bin mint";
        });

      translatorPackage = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "translator_sv2";
          cargoExtraArgs = "--bin translator_sv2";
        });

      jdServerPackage = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "jd_server";
          cargoExtraArgs = "--bin jd_server";
        });

      jdClientPackage = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          pname = "jd_client_sv2";
          cargoExtraArgs = "--bin jd_client_sv2";
        });

      # --- Infrastructure packages (pre-built binaries) ---

      bitcoinNodePackage = import ./bitcoin-node.nix {
        inherit pkgs lib;
        stdenv = pkgs.stdenv;
      };

      sv2TpPackage = import ./sv2-tp.nix {
        inherit pkgs lib;
        stdenv = pkgs.stdenv;
      };
    in {
      # Packages
      packages = {
        default = poolPackage;

        # Hashpool roles
        pool = poolPackage;
        mint = mintPackage;
        translator = translatorPackage;
        jd-server = jdServerPackage;
        jd-client = jdClientPackage;

        # Infrastructure binaries
        bitcoin-node = bitcoinNodePackage;
        sv2-tp = sv2TpPackage;

        # CI / quality targets
        clippy = craneLib.cargoClippy (commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

        doc = craneLib.cargoDoc (commonArgs
          // {
            inherit cargoArtifacts;
          });

        test = craneLib.cargoTest (commonArgs
          // {
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

        jd-server = flake-utils.lib.mkApp {
          drv = jdServerPackage;
          exePath = "/bin/jd_server";
        };

        jd-client = flake-utils.lib.mkApp {
          drv = jdClientPackage;
          exePath = "/bin/jd_client_sv2";
        };
      };
    }))
    # System-independent outputs
    // {
      # NixOS module — curried over `self` so package options default to this flake's packages.
      # Usage: nixosModules.default = inputs.hashpool.nixosModules.default;
      nixosModules.hashpool = import ./nix/hashpool-module.nix self;
      nixosModules.default = self.nixosModules.hashpool;
    };
}
