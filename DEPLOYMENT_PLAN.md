# Hashpool Production Deployment Plan

**NixOS VPS • Testnet4 • ASIC-Ready • With Monitoring & Backups**

Deploy the hashpool ecash mining pool to a single NixOS VPS for POC demonstration. External ASIC miners will connect to demonstrate real mining with ecash token generation.

---

## Architecture Overview

### Components
- **bitcoind-sv2**: Stratum V2 enabled Bitcoin node (testnet4)
- **hashpool-server**: Pool, Mint, and JD-Server components
- **hashpool-client**: Translator proxy and JD-Client components
- **Monitoring**: Health endpoints and metrics

### Network Topology
```
Internet
    │
    ├─> :34255 (Translator Proxy) <- ASIC Miners
    │
VPS │
    ├── bitcoind-sv2 (:48332 RPC)
    ├── hashpool-server
    │   ├── pool_sv2 (:34254)
    │   ├── mint (:3338 HTTP, :34260 SV2)
    │   └── jd_server (:34256)
    └── hashpool-client
        ├── translator_sv2 (:34255)
        └── jd_client
```

---

## Implementation Strategy

### Binary Consolidation via Wrappers

Create wrapper scripts that orchestrate existing binaries without modifying Rust code:

**hashpool-serverd**:
```bash
#!/usr/bin/env bash
set -e
trap 'kill $(jobs -p) 2>/dev/null' EXIT INT TERM

# Start health endpoint
python3 -m http.server 8081 --bind 127.0.0.1 --directory /var/lib/hashpool/health &

# Start components
${mint}/bin/mint -c /etc/hashpool/mint.config.toml -g /etc/hashpool/shared/pool.toml &
MINT_PID=$!

# Wait for mint to be ready
while ! nc -z localhost 3338; do sleep 1; done

${pool_sv2}/bin/pool_sv2 -c /etc/hashpool/pool.config.toml -g /etc/hashpool/shared/pool.toml &
POOL_PID=$!

while ! nc -z localhost 34254; do sleep 1; done

${jd_server}/bin/jd_server -c /etc/hashpool/jds.config.toml &
JD_PID=$!

# Monitor processes
wait $MINT_PID $POOL_PID $JD_PID
```

**hashpool-clientd**:
```bash
#!/usr/bin/env bash
set -e
trap 'kill $(jobs -p) 2>/dev/null' EXIT INT TERM

# Start health endpoint
python3 -m http.server 8082 --bind 127.0.0.1 --directory /var/lib/hashpool/health &

# Start components
export CDK_WALLET_DB_PATH=/var/lib/hashpool/translator/wallet.sqlite
${translator_sv2}/bin/translator_sv2 -c /etc/hashpool/tproxy.config.toml -g /etc/hashpool/shared/miner.toml &
TRANSLATOR_PID=$!

${jd_client}/bin/jd_client -c /etc/hashpool/jdc.config.toml &
CLIENT_PID=$!

wait $TRANSLATOR_PID $CLIENT_PID
```

---

## File Structure

```
hashpool/
├── flake.nix                    # [EXTEND] Add nixosModules and configurations
├── bitcoind.nix                 # [EXISTING] bitcoind-sv2 package
├── devenv.nix                   # [EXISTING] Development environment
├── config/                      # [EXISTING] Configuration files
│   ├── bitcoin.conf
│   ├── jdc.config.toml
│   ├── jds.config.toml
│   ├── mint.config.toml
│   ├── pool.config.toml
│   ├── tproxy.config.toml
│   └── shared/
│       ├── pool.toml
│       └── miner.toml
├── nix/
│   ├── wrappers/               # [NEW] Service wrappers
│   │   ├── hashpool-serverd.sh
│   │   └── hashpool-clientd.sh
│   ├── modules/                # [NEW] NixOS modules
│   │   └── hashpool.nix       # Main service module
│   └── health/                # [NEW] Health check files
│       └── index.html         # Simple health page
└── hosts/
    └── poc.nix                # [NEW] VPS configuration
```

---

## NixOS Module Configuration

### Main Module (`nix/modules/hashpool.nix`)

```nix
{ config, lib, pkgs, ... }:
{
  options.services.hashpool = {
    server = {
      enable = lib.mkEnableOption "Hashpool server (pool, mint, jd-server)";
      openFirewall = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Open firewall for pool connections";
      };
    };
    
    client = {
      enable = lib.mkEnableOption "Hashpool client (translator, jd-client)";
      openFirewall = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Open firewall for ASIC miner connections";
      };
    };
  };
  
  config = lib.mkMerge [
    (lib.mkIf config.services.hashpool.server.enable {
      # Server configuration
      systemd.services.hashpool-server = {
        description = "Hashpool Server Components";
        after = [ "network.target" "bitcoind.service" ];
        requires = [ "bitcoind.service" ];
        wantedBy = [ "multi-user.target" ];
        
        environment = {
          CDK_MINT_DB_PATH = "/var/lib/hashpool/mint/mint.sqlite";
          RUST_LOG = "info";
        };
        
        serviceConfig = {
          Type = "forking";
          User = "hashpool";
          Group = "hashpool";
          ExecStart = "${pkgs.hashpool-serverd}/bin/hashpool-serverd";
          Restart = "always";
          RestartSec = "10s";
          
          # Hardening
          ProtectSystem = "strict";
          ProtectHome = true;
          NoNewPrivileges = true;
          PrivateTmp = true;
          ReadWritePaths = [ "/var/lib/hashpool" ];
        };
      };
      
      # Firewall rules for server
      networking.firewall.allowedTCPPorts = lib.mkIf config.services.hashpool.server.openFirewall [
        34254  # Pool
        34256  # JD Server
      ];
    })
    
    (lib.mkIf config.services.hashpool.client.enable {
      # Client configuration
      systemd.services.hashpool-client = {
        description = "Hashpool Client Components";
        after = [ "network.target" "hashpool-server.service" ];
        wantedBy = [ "multi-user.target" ];
        
        environment = {
          CDK_WALLET_DB_PATH = "/var/lib/hashpool/translator/wallet.sqlite";
          RUST_LOG = "info";
        };
        
        serviceConfig = {
          Type = "forking";
          User = "hashpool";
          Group = "hashpool";
          ExecStart = "${pkgs.hashpool-clientd}/bin/hashpool-clientd";
          Restart = "always";
          RestartSec = "10s";
          
          # Hardening
          ProtectSystem = "strict";
          ProtectHome = true;
          NoNewPrivileges = true;
          PrivateTmp = true;
          ReadWritePaths = [ "/var/lib/hashpool" ];
        };
      };
      
      # Firewall rule for ASIC miners
      networking.firewall.allowedTCPPorts = lib.mkIf config.services.hashpool.client.openFirewall [
        34255  # Translator proxy for external miners
      ];
    })
  ];
}
```

---

## Host Configuration (`hosts/poc.nix`)

```nix
{ config, pkgs, lib, ... }:
{
  imports = [
    ../nix/modules/hashpool.nix
  ];
  
  # Bitcoind with SV2 support
  services.bitcoind = {
    enable = true;
    package = import ../bitcoind.nix { inherit pkgs lib; stdenv = pkgs.stdenv; };
    testnet = 4;
    rpc = {
      port = 48332;
      users = [{
        name = "hashpool";
        passwordHMAC = "..."; # Generated via bitcoin-cli
      }];
    };
    extraConfig = ''
      sv2port=8442
      sv2interval=20
      debug=sv2
    '';
  };
  
  # Hashpool services
  services.hashpool = {
    server = {
      enable = true;
      openFirewall = false; # Only internal access
    };
    client = {
      enable = true;
      openFirewall = true; # Allow ASIC connections
    };
  };
  
  # User and permissions
  users.users.hashpool = {
    isSystemUser = true;
    group = "hashpool";
    home = "/var/lib/hashpool";
    createHome = true;
  };
  users.groups.hashpool = {};
  
  # Create necessary directories
  systemd.tmpfiles.rules = [
    "d /var/lib/hashpool 0750 hashpool hashpool -"
    "d /var/lib/hashpool/mint 0750 hashpool hashpool -"
    "d /var/lib/hashpool/translator 0750 hashpool hashpool -"
    "d /var/lib/hashpool/health 0755 hashpool hashpool -"
    "d /etc/hashpool 0750 hashpool hashpool -"
    "d /etc/hashpool/shared 0750 hashpool hashpool -"
  ];
  
  # Copy configuration files
  environment.etc = {
    "hashpool/pool.config.toml".source = ../config/pool.config.toml;
    "hashpool/mint.config.toml".source = ../config/mint.config.toml;
    "hashpool/jds.config.toml".source = ../config/jds.config.toml;
    "hashpool/jdc.config.toml".source = ../config/jdc.config.toml;
    "hashpool/tproxy.config.toml".source = ../config/tproxy.config.toml;
    "hashpool/shared/pool.toml".source = ../config/shared/pool.toml;
    "hashpool/shared/miner.toml".source = ../config/shared/miner.toml;
  };
  
  # Health check endpoints
  services.nginx = {
    enable = true;
    virtualHosts = {
      "health.local" = {
        listen = [
          { addr = "127.0.0.1"; port = 8081; }
          { addr = "127.0.0.1"; port = 8082; }
        ];
        locations."/" = {
          return = "200 'OK'";
          extraConfig = "add_header Content-Type text/plain;";
        };
        locations."/metrics" = {
          proxyPass = "http://localhost:9090/metrics";
        };
      };
    };
  };
  
  # Monitoring with Prometheus
  services.prometheus = {
    enable = true;
    port = 9090;
    scrapeConfigs = [
      {
        job_name = "hashpool";
        static_configs = [{
          targets = [ "localhost:8081" "localhost:8082" ];
        }];
      }
    ];
  };
  
  # Firewall
  networking.firewall = {
    enable = true;
    allowedTCPPorts = [
      22     # SSH
      34255  # Translator proxy (for ASIC miners)
      80     # HTTP (optional, for monitoring dashboard)
      443    # HTTPS (optional, for monitoring dashboard)
    ];
  };
}
```

---

## Deployment Commands

### Build and Deploy

```bash
# Build locally
nix build .#nixosConfigurations.hashpool-poc.config.system.build.toplevel

# Deploy to VPS
nixos-rebuild switch --flake .#hashpool-poc --target-host root@<vps-ip>

# Or build on VPS directly
ssh root@<vps-ip>
cd /etc/nixos
git clone <repo>
nixos-rebuild switch --flake .#hashpool-poc
```

### Monitoring and Testing

```bash
# Check services
systemctl status bitcoind hashpool-server hashpool-client

# View logs
journalctl -u hashpool-server -f
journalctl -u hashpool-client -f

# Test health endpoints
curl http://localhost:8081/
curl http://localhost:8082/

# Test mint API
curl http://localhost:3338/v1/info

# Manual backup (if needed)
sqlite3 /var/lib/hashpool/mint/mint.sqlite ".backup '/tmp/mint-backup.sqlite'"
sqlite3 /var/lib/hashpool/translator/wallet.sqlite ".backup '/tmp/wallet-backup.sqlite'"
```

### ASIC Miner Configuration

Point ASIC miners to:
- **Server**: `<vps-ip>:34255`
- **Username**: Any (will be assigned by JD)
- **Password**: Any

---

## Port Summary

| Service | Port | Access | Purpose |
|---------|------|--------|---------|
| SSH | 22 | Public | Administration |
| Translator Proxy | 34255 | **Public** | ASIC miner connections |
| Pool | 34254 | Internal | Share submission |
| JD Server | 34256 | Internal | Job distribution |
| Mint HTTP | 3338 | Internal | Cashu API |
| Mint SV2 | 34260 | Internal | Pool communication |
| Bitcoin RPC | 48332 | Internal | Blockchain interaction |
| Health (Server) | 8081 | Internal | Monitoring |
| Health (Client) | 8082 | Internal | Monitoring |
| Prometheus | 9090 | Internal | Metrics collection |

---

## Security Considerations

1. **Firewall**: Only translator proxy (34255) exposed publicly
2. **User isolation**: Dedicated `hashpool` system user
3. **systemd hardening**: ProtectSystem, NoNewPrivileges, etc.
4. **Process monitoring**: Auto-restart on failure
5. **Health checks**: HTTP endpoints for monitoring

---

## Timeline

**Day 1**:
- Create wrapper scripts
- Write NixOS modules
- Test locally with `nixos-rebuild build-vm`

**Day 2**:
- Set up VPS with NixOS
- Deploy configuration
- Test with local mining software

**Day 3**:
- Connect ASIC miner
- Monitor operation
- Final documentation

---

## Success Criteria

✅ All services start and stay running  
✅ Health endpoints return 200 OK  
✅ ASIC miner connects and submits shares  
✅ Mint issues ecash tokens for shares  
✅ Logs show normal operation  

---

## Troubleshooting

### Service won't start
```bash
journalctl -xe -u hashpool-server
# Check for port conflicts
ss -tlnp | grep 34254
```

### ASIC can't connect
```bash
# Check firewall
iptables -L -n | grep 34255
# Test locally
nc -zv localhost 34255
```

### Database issues
```bash
# Check integrity
sqlite3 /var/lib/hashpool/mint/mint.sqlite "PRAGMA integrity_check;"
# Create manual backup if needed
sqlite3 /var/lib/hashpool/mint/mint.sqlite ".backup '/tmp/mint-emergency.sqlite'"
```

### No shares being accepted
```bash
# Check pool logs
tail -f /var/log/hashpool/pool.log
# Verify bitcoind connection
bitcoin-cli -rpcport=48332 -rpcuser=hashpool getblockchaininfo
```