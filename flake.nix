{
  description = "Flake with Devenv requiring self in inputs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    devenv.url = "github:cachix/devenv";
  };

  outputs = {
    self,
    nixpkgs,
    devenv,
    ...
  }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs {
      inherit system;
      lib = nixpkgs.lib;
    };
  in {
    devShells.${system}.default = devenv.lib.mkShell {
      inherit pkgs;

      inputs = {
        nixpkgs = nixpkgs;
        self = self;
      };

      modules = [
        ./devenv.nix

        {
          devenv.root = let
            envRoot = builtins.getEnv "DEVENV_ROOT";
          in
            if envRoot == ""
            then "/tmp"
            else envRoot;

          packages = with pkgs; [
            just
            coreutils
            netcat
            (import ./bitcoind.nix {
              inherit pkgs;
              lib = pkgs.lib;
            })
          ];

          env.BITCOIND_DATADIR = pkgs.lib.mkForce "$DEVENV_ROOT/.devenv/state/bitcoind";

          tasks."create-logs-dir" = {
            exec = pkgs.lib.mkForce "mkdir -p $DEVENV_ROOT/logs";
            before = ["enterShell"];
          };

          processes = {
            local-pool = {
              exec = pkgs.lib.mkForce ''
                mkdir -p $DEVENV_ROOT/logs
                bash -c 'stdbuf -oL cargo -C roles/pool -Z unstable-options run -- -c $DEVENV_ROOT/roles/pool/config-examples/pool-config-local-tp-example.toml 2>&1 | tee -a $DEVENV_ROOT/logs/local-pool.log'
              '';
            };
            job-server = {
              exec = pkgs.lib.mkForce ''
                mkdir -p $DEVENV_ROOT/logs
                bash -c 'stdbuf -oL cargo -C roles/jd-server -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-server/config-examples/jds-config-local-example.toml 2>&1 | tee -a $DEVENV_ROOT/logs/job-server.log'
              '';
            };
            job-client = {
              exec = pkgs.lib.mkForce ''
                mkdir -p $DEVENV_ROOT/logs
                bash -c 'stdbuf -oL cargo -C roles/jd-client -Z unstable-options run -- -c $DEVENV_ROOT/roles/jd-client/config-examples/jdc-config-local-example.toml 2>&1 | tee -a $DEVENV_ROOT/logs/job-client.log'
              '';
            };
            translator-proxy = {
              exec = pkgs.lib.mkForce ''
                mkdir -p $DEVENV_ROOT/logs
                bash -c 'stdbuf -oL cargo -C roles/translator -Z unstable-options run -- -c $DEVENV_ROOT/roles/translator/config-examples/tproxy-config-local-jdc-example.toml 2>&1 | tee -a $DEVENV_ROOT/logs/translator-proxy.log'
              '';
            };
            bitcoind-testnet = {
              exec = pkgs.lib.mkForce ''
                mkdir -p $DEVENV_ROOT/logs
                bash -c 'stdbuf -oL bitcoind -testnet4 -sv2 -sv2port=8442 -debug=sv2 -conf=$DEVENV_ROOT/bitcoin.conf -datadir=$BITCOIND_DATADIR 2>&1 | tee -a $DEVENV_ROOT/logs/bitcoind-testnet.log'
              '';
            };
            miner = {
              exec = pkgs.lib.mkForce ''
                echo "Waiting for translator proxy on port 34255..."
                while ! nc -z localhost 34255; do sleep 1; done
                echo "Translator proxy is up, starting miner..."
                cd roles/test-utils/mining-device-sv1
                while true; do
                  RUST_LOG=debug cargo run 2>&1 | tee -a $DEVENV_ROOT/logs/miner.log
                  echo "Miner crashed. Restarting..." >> $DEVENV_ROOT/logs/miner.log
                  sleep 5
                done
              '';
            };
          };

          pre-commit.hooks = {
            alejandra.enable = true;
          };

          enterShell = ''
            echo "Available Just Commands:"
            just --list
            echo "Running Processes:"
            for process in $(ls $DEVENV_ROOT/.devenv/processes 2>/dev/null); do
              echo "- $process"
            done
          '';
        }
      ];
    };
  };
}
