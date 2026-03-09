# NixOS module for Hashpool — a Stratum V2 pool with Cashu ecash.
#
# Usage in a NixOS configuration:
#
#   inputs.hashpool.url = "github:vnprc/hashpool";
#
#   nixosModules.hashpool = inputs.hashpool.nixosModules.default;
#
#   services.hashpool = {
#     enable = true;
#     network = "testnet4";
#     dataDir = "/var/lib/hashpool";
#     configDir = "/etc/hashpool/config";
#   };
#
# The configDir must contain: pool.config.toml, jds.config.toml,
# jdc.config.toml, mint.config.toml, tproxy.config.toml, sv2-tp.conf,
# bitcoin.conf, shared/pool.toml, shared/miner.toml
#
# This module is curried over `self` (the hashpool flake) so it can
# default package options to the flake's own built packages.
self: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.hashpool;

  # Map Hashpool network name to Bitcoin Core chain name
  chainName = {
    regtest = "regtest";
    testnet4 = "testnet4";
    mainnet = "main";
  }
  .${cfg.network};

  # Common systemd hardening for all hashpool services
  commonServiceConfig = {
    User = cfg.user;
    Group = cfg.group;
    Restart = "on-failure";
    RestartSec = "5s";
    PrivateTmp = true;
    ProtectSystem = "strict";
    ProtectHome = true;
    ReadWritePaths = [cfg.dataDir];
    # Allow reading config files
    ReadOnlyPaths = [cfg.configDir];
    NoNewPrivileges = true;
    PrivateDevices = true;
  };
in {
  options.services.hashpool = {
    enable = lib.mkEnableOption "Hashpool SV2 mining pool with Cashu ecash";

    network = lib.mkOption {
      type = lib.types.enum ["regtest" "testnet4" "mainnet"];
      default = "testnet4";
      description = ''
        Bitcoin network to connect to.
        Determines the chain flag passed to bitcoin-node and sv2-tp.
      '';
    };

    dataDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/hashpool";
      description = ''
        State directory for bitcoin-node data and hashpool databases
        (mint SQLite, translator wallet). Created automatically with
        ownership set to services.hashpool.user.
      '';
    };

    configDir = lib.mkOption {
      type = lib.types.path;
      description = ''
        Path to a directory containing TOML config files for all hashpool
        services. Expected layout:

          <configDir>/
            bitcoin.conf
            sv2-tp.conf
            pool.config.toml
            jds.config.toml
            jdc.config.toml
            mint.config.toml        (required if enableMint = true)
            tproxy.config.toml      (required if enableTranslator = true)
            shared/
              pool.toml
              miner.toml

        See config/ in the hashpool repository for reference examples.
        The operator MUST set fresh authority_public_key/authority_secret_key
        and a real coinbase_reward_script in pool.config.toml before deploying.
      '';
    };

    enableMint = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Whether to run the Cashu mint service.";
    };

    enableTranslator = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether to run the SV1 translator/proxy (miner-facing).
        Miners connect to this service using the Stratum V1 protocol.
      '';
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "hashpool";
      description = "System user to run all hashpool services as.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "hashpool";
      description = "System group for all hashpool services.";
    };

    # --- Package options (default to this flake's built packages) ---

    bitcoinNodePackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.bitcoin-node;
      defaultText = lib.literalExpression "hashpool.packages.\${system}.bitcoin-node";
      description = "Bitcoin Core 30.2 package providing bitcoin-node and bitcoin-cli.";
    };

    sv2TpPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.sv2-tp;
      defaultText = lib.literalExpression "hashpool.packages.\${system}.sv2-tp";
      description = "sv2-tp Template Provider package (v1.0.6).";
    };

    poolPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.pool;
      defaultText = lib.literalExpression "hashpool.packages.\${system}.pool";
      description = "Hashpool pool role binary.";
    };

    jdServerPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.jd-server;
      defaultText = lib.literalExpression "hashpool.packages.\${system}.jd-server";
      description = "Hashpool Job Declarator Server binary.";
    };

    jdClientPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.jd-client;
      defaultText = lib.literalExpression "hashpool.packages.\${system}.jd-client";
      description = "Hashpool Job Declarator Client binary.";
    };

    mintPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.mint;
      defaultText = lib.literalExpression "hashpool.packages.\${system}.mint";
      description = "Hashpool Cashu mint binary.";
    };

    translatorPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.translator;
      defaultText = lib.literalExpression "hashpool.packages.\${system}.translator";
      description = "Hashpool SV1 translator/proxy binary.";
    };
  };

  config = lib.mkIf cfg.enable {
    # Create the hashpool system user and group
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      home = cfg.dataDir;
      description = "Hashpool service user";
    };

    users.groups.${cfg.group} = {};

    # Ensure the state directory exists with correct ownership
    systemd.tmpfiles.rules = [
      "d '${cfg.dataDir}' 0750 ${cfg.user} ${cfg.group} - -"
    ];

    systemd.services =
      {
        # ── bitcoin-node ──────────────────────────────────────────────────────
        # Runs Bitcoin Core with multiprocess IPC. sv2-tp connects via unix socket.
        hashpool-bitcoin-node = {
          description = "Bitcoin Core node (hashpool)";
          wantedBy = ["multi-user.target"];
          after = ["network.target"];

          serviceConfig =
            commonServiceConfig
            // {
              ExecStart = lib.escapeShellArgs [
                "${cfg.bitcoinNodePackage}/bin/bitcoin"
                "-m"
                "node"
                "-datadir=${cfg.dataDir}"
                "-chain=${chainName}"
                "-conf=${cfg.configDir}/bitcoin.conf"
                "-ipcbind=unix"
              ];
            };
        };

        # ── sv2-tp ────────────────────────────────────────────────────────────
        # Template Provider — connects to bitcoin-node via unix IPC socket and
        # serves block templates to pool and jd-client over the SV2 protocol.
        hashpool-sv2-tp = {
          description = "sv2-tp Template Provider (hashpool)";
          wantedBy = ["multi-user.target"];
          after = ["network.target" "hashpool-bitcoin-node.service"];
          wants = ["hashpool-bitcoin-node.service"];

          serviceConfig =
            commonServiceConfig
            // {
              ExecStart = lib.escapeShellArgs [
                "${cfg.sv2TpPackage}/bin/sv2-tp"
                "-datadir=${cfg.dataDir}"
                "-chain=${chainName}"
                "-conf=${cfg.configDir}/sv2-tp.conf"
              ];
            };
        };

        # ── pool ──────────────────────────────────────────────────────────────
        # Core pool role: receives templates from sv2-tp, manages mining channels,
        # validates shares, and coordinates with jd-server/jd-client.
        hashpool-pool = {
          description = "Hashpool SV2 Pool";
          wantedBy = ["multi-user.target"];
          after = ["network.target" "hashpool-sv2-tp.service"];
          wants = ["hashpool-sv2-tp.service"];

          serviceConfig =
            commonServiceConfig
            // {
              ExecStart =
                "${cfg.poolPackage}/bin/pool"
                + " -c ${cfg.configDir}/pool.config.toml"
                + " -g ${cfg.configDir}/shared/pool.toml";
            };
        };

        # ── jd-server ─────────────────────────────────────────────────────────
        # Job Declarator Server: validates custom jobs from jd-client and
        # provides Bitcoin RPC access for the job declaration flow.
        hashpool-jd-server = {
          description = "Hashpool Job Declarator Server";
          wantedBy = ["multi-user.target"];
          after = ["network.target" "hashpool-pool.service"];
          wants = ["hashpool-pool.service"];

          serviceConfig =
            commonServiceConfig
            // {
              ExecStart = "${cfg.jdServerPackage}/bin/jd_server -c ${cfg.configDir}/jds.config.toml";
            };
        };

        # ── jd-client ─────────────────────────────────────────────────────────
        # Job Declarator Client: fetches templates from sv2-tp, declares custom
        # jobs to jd-server, and connects downstream miners to the pool.
        hashpool-jd-client = {
          description = "Hashpool Job Declarator Client";
          wantedBy = ["multi-user.target"];
          after = [
            "network.target"
            "hashpool-sv2-tp.service"
            "hashpool-pool.service"
            "hashpool-jd-server.service"
          ];
          wants = [
            "hashpool-sv2-tp.service"
            "hashpool-pool.service"
            "hashpool-jd-server.service"
          ];

          serviceConfig =
            commonServiceConfig
            // {
              ExecStart = "${cfg.jdClientPackage}/bin/jd_client_sv2 -c ${cfg.configDir}/jdc.config.toml";
            };
        };
      }
      # ── mint (optional) ────────────────────────────────────────────────────
      # Cashu mint: issues ecash tokens (HASH units) when pool validates shares.
      # Starts independently — pool and translator connect to it via HTTP.
      // lib.optionalAttrs cfg.enableMint {
        hashpool-mint = {
          description = "Hashpool Cashu Mint";
          wantedBy = ["multi-user.target"];
          after = ["network.target"];

          environment = {
            # Override the db_path from mint.config.toml to use dataDir
            CDK_MINT_DB_PATH = "${cfg.dataDir}/mint.sqlite";
          };

          serviceConfig =
            commonServiceConfig
            // {
              ExecStart =
                "${cfg.mintPackage}/bin/mint"
                + " -c ${cfg.configDir}/mint.config.toml"
                + " -g ${cfg.configDir}/shared/pool.toml";
            };
        };
      }
      # ── translator (optional) ──────────────────────────────────────────────
      # SV1 translator/proxy: accepts SV1 miner connections and translates them
      # to SV2 for the upstream pool. Miners point their ASICs here.
      // lib.optionalAttrs cfg.enableTranslator {
        hashpool-translator = {
          description = "Hashpool SV1 Translator/Proxy";
          wantedBy = ["multi-user.target"];
          after = ["network.target" "hashpool-pool.service"];
          wants = ["hashpool-pool.service"];

          environment = {
            # Override wallet db_path from tproxy.config.toml to use dataDir
            CDK_WALLET_DB_PATH = "${cfg.dataDir}/translator-wallet.sqlite";
          };

          serviceConfig =
            commonServiceConfig
            // {
              ExecStart =
                "${cfg.translatorPackage}/bin/translator_sv2"
                + " -c ${cfg.configDir}/tproxy.config.toml"
                + " -g ${cfg.configDir}/shared/miner.toml";
            };
        };
      };
  };
}
