# Stats Service Configuration Refactoring Plan

## Executive Summary

This plan consolidates all stats service configuration into dedicated TOML files and shared configuration. The stats architecture is **hybrid push-pull**:

- **PUSH:** Translator/Pool actively send snapshots to stats services (polling interval = **configurable**)
- **PULL:** Web services poll stats services via HTTP (polling intervals = **configurable**)

Currently, many timing and address configurations are hardcoded in source code. This refactor makes them all configurable through TOML files, enabling operators to tune performance without recompiling.

---

## Part 1: Understanding the Stats Architecture

### Data Flow (Push-Pull Hybrid)

```
═══════════════════════════════════════════════════════════════════
PUSH PHASE: Translator/Pool → Stats Services (Core Mining Logic)
═══════════════════════════════════════════════════════════════════

Translator Service
  ├─ Every N seconds (configurable via miner.toml [stats] snapshot_poll_interval_secs)
  ├─ Generate ProxySnapshot via StatsSnapshotProvider::get_snapshot()
  └─ PUSH via: StatsClient::send_snapshot()
      └─ TCP connection → Stats-Proxy (persistent, auto-reconnect)
      └─ Newline-delimited JSON format
      └─ No response expected (fire-and-forget)

Pool Service
  ├─ Every N seconds (configurable via pool.toml [stats] snapshot_poll_interval_secs)
  ├─ Generate PoolSnapshot via StatsSnapshotProvider::get_snapshot()
  └─ PUSH via: StatsClient::send_snapshot()
      └─ TCP connection → Stats-Pool (persistent, auto-reconnect)
      └─ Newline-delimited JSON format
      └─ No response expected (fire-and-forget)

Stats-Proxy Service (RECEIVER)
  ├─ TCP server listening on address from stats-proxy.config.toml
  ├─ Receives newline-delimited JSON from Translator
  ├─ Stores latest snapshot in memory
  └─ HTTP server exposes snapshots to web-proxy

Stats-Pool Service (RECEIVER)
  ├─ TCP server listening on address from stats-pool.config.toml
  ├─ Receives newline-delimited JSON from Pool
  ├─ Stores latest snapshot in memory
  └─ HTTP server exposes snapshots to web-pool

═══════════════════════════════════════════════════════════════════
PULL PHASE: Web Services → Stats Services → Browser (UI Refresh)
═══════════════════════════════════════════════════════════════════

Web-Proxy Service (PULLER)
  ├─ Every N seconds (configurable via miner.toml [web_proxy] stats_poll_interval_secs)
  ├─ HTTP GET request → Stats-Proxy /api/stats
  ├─ Stores snapshot in memory
  ├─ HTTP server on address from web-proxy.config.toml
  └─ Browser connects here

Web-Pool Service (PULLER)
  ├─ Every N seconds (configurable via pool.toml [web_pool] stats_poll_interval_secs)
  ├─ HTTP GET request → Stats-Pool /api/stats
  ├─ Stores snapshot in memory
  ├─ HTTP server on address from web-pool.config.toml
  └─ Browser connects here

Browser (JavaScript, PULLER)
  ├─ Web-Proxy: Every N seconds (configurable via miner.toml [web_proxy] client_poll_interval_secs)
  │  └─ JavaScript fetch() → Web-Proxy /api/stats
  │  └─ Update DOM
  │
  └─ Web-Pool: Every N seconds (configurable via pool.toml [web_pool] client_poll_interval_secs)
     └─ JavaScript fetch() → Web-Pool /api/stats
     └─ Update DOM
```

### Communication Methods Compared

| Layer | Protocol | Direction | Initiation | Connection | Format | Role |
|-------|----------|-----------|-----------|-----------|--------|------|
| **Translator ↔ Pool (SV2)** | SV2 binary | Bidirectional | Mutual | Persistent channel | Binary with framing | Core mining |
| **Translator → Stats-Proxy** | TCP + JSON | Push only | Translator | Persistent, auto-reconnect | Newline-delimited JSON | Stats collection |
| **Pool → Stats-Pool** | TCP + JSON | Push only | Pool | Persistent, auto-reconnect | Newline-delimited JSON | Stats collection |
| **Web-Proxy ← Stats-Proxy** | HTTP | Pull only | Web-Proxy | Stateless polling | JSON over HTTP | UI refresh |
| **Web-Pool ← Stats-Pool** | HTTP | Pull only | Web-Pool | Stateless polling | JSON over HTTP | UI refresh |
| **Browser ← Web-Proxy** | HTTP | Pull only | Browser | Stateless polling | JSON/HTML over HTTP | User interface |
| **Browser ← Web-Pool** | HTTP | Pull only | Browser | Stateless polling | JSON/HTML over HTTP | User interface |

### Why Hybrid Architecture?

**Push (Translator/Pool → Stats):**
- Stats are derived from core mining state
- Translator/Pool already own the data
- Fire-and-forget mechanism, no response needed
- Stats services are passive receivers
- Keeps stats decoupled from mining logic
- Translator doesn't wait for stats-proxy to acknowledge

**Pull (Web → Stats → Browser):**
- Web services need fresh data on-demand
- HTTP is standard for web services
- Multiple web services can independently consume same stats
- Web services don't block mining if they fail
- Graceful degradation if web service is slow
- Browser can control its own refresh rate

---

## Part 2: What Gets Configured

### Snapshot Polling Intervals (CONFIGURABLE)

These are stats collection frequencies, not core mining logic. They should be tunable per deployment.

| Component | Default | Config File | Section | Key | Purpose |
|-----------|---------|-------------|---------|-----|---------|
| Translator snapshot generation | 5s | config/shared/miner.toml | [stats] | snapshot_poll_interval_secs | How often translator sends snapshots to stats-proxy |
| Pool snapshot generation | 5s | config/shared/pool.toml | [stats] | snapshot_poll_interval_secs | How often pool sends snapshots to stats-pool |
| Web-Proxy stats polling | 3s | config/shared/miner.toml | [web_proxy] | stats_poll_interval_secs | How often web-proxy pulls from stats-proxy |
| Browser polls web-proxy | 3s | config/shared/miner.toml | [web_proxy] | client_poll_interval_secs | How often browser refreshes dashboard |
| Web-Pool stats polling | 3s | config/shared/pool.toml | [web_pool] | stats_poll_interval_secs | How often web-pool pulls from stats-pool |
| Browser polls web-pool | 3s | config/shared/pool.toml | [web_pool] | client_poll_interval_secs | How often browser refreshes dashboard |

### Service Configuration (CONFIGURABLE)

Service-specific settings like listen addresses, HTTP timeouts, and staleness thresholds go in dedicated config files.

| Component | Config File | Section | Key | Default | Purpose |
|-----------|-------------|---------|-----|---------|---------|
| Stats-Proxy | stats-proxy.config.toml | [server] | tcp_listen_address | 127.0.0.1:8082 | Where translator connects and pushes |
| Stats-Proxy | stats-proxy.config.toml | [server] | http_listen_address | 127.0.0.1:8084 | Where web-proxy pulls snapshots |
| Stats-Proxy | stats-proxy.config.toml | [snapshot_storage] | staleness_threshold_secs | 15 | Health check threshold |
| Stats-Proxy | stats-proxy.config.toml | [http_client] | pool_idle_timeout_secs | 300 | HTTP connection pooling |
| Stats-Proxy | stats-proxy.config.toml | [http_client] | request_timeout_secs | 60 | API call timeout |
| Stats-Pool | stats-pool.config.toml | [server] | tcp_listen_address | 127.0.0.1:9083 | Where pool connects and pushes |
| Stats-Pool | stats-pool.config.toml | [server] | http_listen_address | 127.0.0.1:9084 | Where web-pool pulls snapshots |
| Stats-Pool | stats-pool.config.toml | [snapshot_storage] | staleness_threshold_secs | 15 | Health check threshold |
| Stats-Pool | stats-pool.config.toml | [http_client] | pool_idle_timeout_secs | 300 | HTTP connection pooling |
| Stats-Pool | stats-pool.config.toml | [http_client] | request_timeout_secs | 60 | API call timeout |
| Web-Proxy | web-proxy.config.toml | [server] | listen_address | 127.0.0.1:3030 | Web dashboard listen address |
| Web-Proxy | web-proxy.config.toml | [stats_proxy] | url | http://127.0.0.1:8084 | Where to pull stats from |
| Web-Proxy | web-proxy.config.toml | [http_client] | pool_idle_timeout_secs | 300 | HTTP connection pooling |
| Web-Proxy | web-proxy.config.toml | [http_client] | request_timeout_secs | 60 | API call timeout |
| Web-Pool | web-pool.config.toml | [server] | listen_address | 127.0.0.1:8081 | Web dashboard listen address |
| Web-Pool | web-pool.config.toml | [stats_pool] | url | http://127.0.0.1:9084 | Where to pull stats from |
| Web-Pool | web-pool.config.toml | [http_client] | pool_idle_timeout_secs | 300 | HTTP connection pooling |
| Web-Pool | web-pool.config.toml | [http_client] | request_timeout_secs | 60 | API call timeout |

---

## Part 3: Configuration Files

### Shared Miner Configuration (`config/shared/miner.toml`) - MODIFY

```toml
[stats]
# Interval for translator to generate and push snapshots to stats-proxy (seconds)
# Tuning notes:
# - Development: 5s for normal monitoring
# - High-frequency monitoring: 1s for detailed metrics
# - Battery-limited: 10s to reduce overhead
snapshot_poll_interval_secs = 5

[web_proxy]
# Interval for web-proxy to pull from stats-proxy (seconds)
stats_poll_interval_secs = 3

# Interval for browser to poll web-proxy API (seconds)
client_poll_interval_secs = 3

# Existing faucet configuration
[faucet]
enabled = true
host = "127.0.0.1"
port = 8083
faucet_timeout = 3
```

### Shared Pool Configuration (`config/shared/pool.toml`) - MODIFY

```toml
[stats]
# Interval for pool to generate and push snapshots to stats-pool (seconds)
# Tuning notes:
# - Development: 5s for normal monitoring
# - High-frequency monitoring: 1s for detailed metrics
# - Battery-limited: 10s to reduce overhead
snapshot_poll_interval_secs = 5

[web_pool]
# Interval for web-pool to pull from stats-pool (seconds)
stats_poll_interval_secs = 3

# Interval for browser to poll web-pool API (seconds)
client_poll_interval_secs = 3

# Existing sv2_messaging configuration
[sv2_messaging]
enabled = true
mint_listen_address = "127.0.0.1:34260"
broadcast_buffer_size = 1000
mpsc_buffer_size = 100
max_retries = 3
timeout_ms = 5000
```

### Service-Specific Configs (NEW)

**`config/stats-proxy.config.toml`**
```toml
# Stats-Proxy Configuration
# Receives TCP snapshots from Translator, exposes HTTP API for web-proxy

[server]
# TCP server: where Translator connects and pushes snapshots
tcp_listen_address = "127.0.0.1:8082"

# HTTP server: where web-proxy pulls snapshots via HTTP GET /api/stats
http_listen_address = "127.0.0.1:8084"

[snapshot_storage]
# Database path for persistent storage (optional)
db_path = ".devenv/state/stats-proxy.db"

# Threshold in seconds for marking data as stale in /health endpoint
# Used by monitoring systems to detect if Translator stopped sending updates
staleness_threshold_secs = 15

[http_client]
# When stats-proxy makes HTTP requests to other services
pool_idle_timeout_secs = 300
request_timeout_secs = 60
```

**`config/stats-pool.config.toml`**
```toml
# Stats-Pool Configuration
# Receives TCP snapshots from Pool, exposes HTTP API for web-pool

[server]
# TCP server: where Pool connects and pushes snapshots
tcp_listen_address = "127.0.0.1:9083"

# HTTP server: where web-pool pulls snapshots via HTTP GET /api/stats
http_listen_address = "127.0.0.1:9084"

[snapshot_storage]
# Threshold in seconds for marking data as stale in /health endpoint
# Used by monitoring systems to detect if Pool stopped sending updates
staleness_threshold_secs = 15

[http_client]
# When stats-pool makes HTTP requests to other services
pool_idle_timeout_secs = 300
request_timeout_secs = 60
```

**`config/web-proxy.config.toml`**
```toml
# Web-Proxy Configuration
# Pulls snapshots from stats-proxy via HTTP, serves to browser
# Polling intervals come from config/shared/miner.toml [web_proxy]

[server]
# HTTP server: where browser and other clients fetch dashboard
listen_address = "127.0.0.1:3030"

[stats_proxy]
# HTTP endpoint where stats-proxy serves snapshots
url = "http://127.0.0.1:8084"

[http_client]
# When web-proxy pulls from stats-proxy
pool_idle_timeout_secs = 300
request_timeout_secs = 60
```

**`config/web-pool.config.toml`**
```toml
# Web-Pool Configuration
# Pulls snapshots from stats-pool via HTTP, serves to browser
# Polling intervals come from config/shared/pool.toml [web_pool]

[server]
# HTTP server: where browser and other clients fetch dashboard
listen_address = "127.0.0.1:8081"

[stats_pool]
# HTTP endpoint where stats-pool serves snapshots
url = "http://127.0.0.1:9084"

[http_client]
# When web-pool pulls from stats-pool
pool_idle_timeout_secs = 300
request_timeout_secs = 60
```

### Production Deployment Configs (MIRROR STRUCTURE)

Create parallel configs in `config/prod/`:
- `config/prod/stats-proxy.config.toml`
- `config/prod/stats-pool.config.toml`
- `config/prod/web-proxy.config.toml`
- `config/prod/web-pool.config.toml`

Adjust values per production requirements (e.g., more frequent polling for busy pools).

---

## Part 4: Implementation Plan

### PHASE 1: Create Configuration Files
**Scope:** Create new TOML files for each stats service
**Estimated effort:** 1 hour

**Files to create:**
- `config/stats-proxy.config.toml`
- `config/stats-pool.config.toml`
- `config/web-proxy.config.toml`
- `config/web-pool.config.toml`
- `config/prod/stats-proxy.config.toml`
- `config/prod/stats-pool.config.toml`
- `config/prod/web-proxy.config.toml`
- `config/prod/web-pool.config.toml`

**Tasks:**
1. Create config files with all sections and defaults
2. Add documentation comments explaining each setting
3. Ensure ports match devenv.nix current values

**Deliverable:** New config files ready for code consumption

---

### PHASE 2: Translator Snapshot Polling Configuration
**Scope:** Make translator snapshot generation frequency configurable
**Estimated effort:** 2 hours

**Files to modify:**
- `roles/translator/src/lib/proxy_config.rs` - Add snapshot_poll_interval_secs field
- `roles/translator/src/lib/mod.rs` - Load and use config value
- `roles/roles-utils/stats/src/stats_poller.rs` - Accept Duration parameter

**Tasks:**
1. Add `snapshot_poll_interval_secs: Option<u64>` to ProxyConfig struct (default 5)
2. Load from `config/shared/miner.toml [stats]` section
3. Update stats_poller.rs to accept Duration parameter instead of hardcoding
4. Pass configured interval to stats polling loop in translator initialization
5. Update stats polling loop in translator/src/lib/mod.rs:219 to use config value

**Deliverable:** Translator snapshot polling is configurable

---

### PHASE 3: Pool Snapshot Polling Configuration
**Scope:** Make pool snapshot generation frequency configurable
**Estimated effort:** 2 hours

**Files to modify:**
- `roles/pool/src/lib/pool_config.rs` - Add snapshot_poll_interval_secs field
- `roles/pool/src/lib/mining_pool/mod.rs` - Load and use config value

**Tasks:**
1. Add `snapshot_poll_interval_secs: Option<u64>` to PoolConfig struct (default 5)
2. Load from `config/shared/pool.toml [stats]` section
3. Update stats polling loop in pool/src/lib/mining_pool/mod.rs:1187 to use config value instead of hardcoding 5

**Deliverable:** Pool snapshot polling is configurable

---

### PHASE 4: Stats-Proxy Configuration Loading
**Scope:** Load stats-proxy config from TOML file
**Estimated effort:** 2 hours

**Files to modify:**
- `roles/stats-proxy/src/config.rs` - Add TOML loading
- `roles/stats-proxy/src/main.rs` - Use config values

**Tasks:**
1. Extend Config struct with all new fields (TCP/HTTP addresses, staleness threshold, HTTP timeouts)
2. Add `--config` CLI arg (keep TCP/HTTP args as fallback/override)
3. Load TOML file using `toml` crate
4. Pass staleness_threshold_secs to `/health` endpoint
5. Pass HTTP timeout config to reqwest client builder
6. Update main.rs to accept configuration path via CLI arg

**Deliverable:** stats-proxy reads from dedicated config file

---

### PHASE 5: Stats-Pool Configuration Loading
**Scope:** Load stats-pool config from TOML file
**Estimated effort:** 2 hours

**Files to modify:**
- `roles/stats-pool/src/config.rs` - Add TOML loading
- `roles/stats-pool/src/main.rs` - Use config values

**Tasks:**
1. Extend Config struct with all new fields
2. Add `--config` CLI arg (keep TCP/HTTP args as fallback/override)
3. Load TOML file using `toml` crate
4. Pass staleness_threshold_secs to `/health` endpoint
5. Pass HTTP timeout config to reqwest client builder
6. Update main.rs to accept configuration path via CLI arg

**Deliverable:** stats-pool reads from dedicated config file

---

### PHASE 6: Web-Proxy Service Config Loading
**Scope:** Load web-proxy.config.toml for service-specific settings
**Estimated effort:** 1.5 hours

**Files to modify:**
- `roles/web-proxy/src/config.rs` - Add web-proxy.config.toml loading
- `roles/web-proxy/src/main.rs` - Accept `--config` arg
- `roles/web-proxy/src/web.rs` - Use request_timeout_secs

**Tasks:**
1. Keep existing shared miner.toml loading for polling intervals
2. Add web-proxy.config.toml loading for service URLs and HTTP timeouts
3. Add `--config` CLI arg for web-proxy.config.toml path
4. Pass pool_idle_timeout_secs to reqwest client
5. Pass request_timeout_secs to HTTP calls in web.rs

**Deliverable:** web-proxy loads service-specific config + polling intervals from shared

---

### PHASE 7: Web-Pool Configuration Loading (MAJOR)
**Scope:** Load web-pool.config.toml and make all polling intervals configurable
**Estimated effort:** 3 hours

**Files to modify:**
- `roles/web-pool/src/config.rs` - Create proper config loading
- `roles/web-pool/src/main.rs` - Load configs, use polling intervals
- `roles/web-pool/src/web.rs` - Accept and use polling intervals
- `roles/web-pool/templates/dashboard.html` - Use injected polling interval

**Tasks:**
1. Create Config struct with server, stats_pool, http_client sections
2. Add web-pool.config.toml loading using `toml` crate
3. Add shared pool.toml loading for [web_pool] section
4. Add `--config` CLI arg for web-pool.config.toml path
5. Replace hardcoded `const POLL_INTERVAL_SECS: u64 = 5` with config value from pool.toml
6. Pass stats_poll_interval_secs to poll_stats_pool() function
7. Pass client_poll_interval_secs to start_web_server()
8. Update `run_http_server()` signature to accept client_poll_interval_secs
9. Update HTML template to use `{client_poll_interval_ms}` instead of hardcoded 3000
10. Pass HTTP timeout config to reqwest client

**Deliverable:** web-pool fully configurable, both polling intervals tunable

---

### PHASE 8: Update devenv.nix
**Scope:** Update service invocations to use config files
**Estimated effort:** 1 hour

**Files to modify:**
- `devenv.nix` - Update process definitions

**Tasks:**
1. Remove hardcoded port constants (statsPoolTcpPort, statsPoolHttpPort, etc.)
2. Update stats-proxy process: add `--config config/stats-proxy.config.toml`
3. Update stats-pool process: add `--config config/stats-pool.config.toml`
4. Update web-proxy process: add `--config config/web-proxy.config.toml`
5. Update web-pool process: add `--config config/web-pool.config.toml`
6. Simplify command lines since ports are now in config files

**Deliverable:** devenv.nix simplified and uses config files

---

### PHASE 9: Documentation and Testing
**Scope:** Document configuration and verify functionality
**Estimated effort:** 2 hours

**Files to modify:**
- `docs/AGENTS.md` - Add stats service configuration section

**Tasks:**
1. Document stats architecture (PUSH → PULL)
2. Explain what gets configured where
3. Add configuration reference section
4. Add tuning recommendations (e.g., when to adjust polling intervals)
5. Unit test config loading for all services
6. Integration test with devenv
7. Verify no behavior change with default config values
8. Test config override scenarios

**Deliverable:** Documentation complete, all tests passing

---

## Part 5: File Summary

### Files to Create (8)
- `config/stats-proxy.config.toml`
- `config/stats-pool.config.toml`
- `config/web-proxy.config.toml`
- `config/web-pool.config.toml`
- `config/prod/stats-proxy.config.toml`
- `config/prod/stats-pool.config.toml`
- `config/prod/web-proxy.config.toml`
- `config/prod/web-pool.config.toml`

### Files to Modify (16)
- `roles/translator/src/lib/proxy_config.rs` - Add snapshot_poll_interval_secs field
- `roles/translator/src/lib/mod.rs` - Load and use config value
- `roles/pool/src/lib/pool_config.rs` - Add snapshot_poll_interval_secs field
- `roles/pool/src/lib/mining_pool/mod.rs` - Load and use config value
- `roles/stats-proxy/src/config.rs` - Add TOML loading
- `roles/stats-proxy/src/main.rs` - Use config values
- `roles/stats-pool/src/config.rs` - Add TOML loading
- `roles/stats-pool/src/main.rs` - Use config values
- `roles/web-proxy/src/config.rs` - Add service config loading
- `roles/web-proxy/src/main.rs` - Accept config arg, use values
- `roles/web-proxy/src/web.rs` - Use configured timeouts
- `roles/web-pool/src/config.rs` - Create proper config loading
- `roles/web-pool/src/main.rs` - Load configs, use polling intervals
- `roles/web-pool/src/web.rs` - Accept and use polling intervals
- `roles/web-pool/templates/dashboard.html` - Use injected polling interval
- `roles/roles-utils/stats/src/stats_poller.rs` - Accept Duration parameter
- `config/shared/miner.toml` - Add [stats] and update [web_proxy]
- `config/shared/pool.toml` - Add [stats] and [web_pool]
- `devenv.nix` - Update service invocations
- `docs/AGENTS.md` - Add configuration documentation

---

## Part 6: Estimated Effort

| Phase | Effort | Notes |
|-------|--------|-------|
| Phase 1 | 1 hour | Config file creation |
| Phase 2 | 2 hours | Translator snapshot polling |
| Phase 3 | 2 hours | Pool snapshot polling |
| Phase 4 | 2 hours | Stats-proxy config loading |
| Phase 5 | 2 hours | Stats-pool config loading |
| Phase 6 | 1.5 hours | Web-proxy service config |
| Phase 7 | 3 hours | **Web-pool major refactor** |
| Phase 8 | 1 hour | devenv.nix updates |
| Phase 9 | 2 hours | Documentation and testing |
| **Total** | **17 hours** | Consolidate all stats configuration |

---

## Part 7: Configuration Philosophy

### Service-Specific Values → Service-Specific Config Files
✅ TCP/HTTP listen addresses
✅ HTTP timeouts
✅ Database paths
✅ Service URLs

### Shared Deployment Values → Shared Config Files
✅ Polling intervals (tuned by operators, affect system behavior)
✅ High-level feature flags (faucet enabled, etc.)
✅ Snapshot collection frequencies

### No Config File Cross-References
❌ No config files referencing paths to other config files
✅ Each service is self-contained
✅ Shared configs are loaded separately by each service

### Backward Compatibility
✅ All fields optional with sensible defaults
✅ CLI arguments still work as override mechanism
✅ Existing deployments continue working with defaults

---

## Part 8: Key Concepts

### PUSH (Translator/Pool → Stats Services)

```
Core mining logic generates snapshots every N seconds
  └─ Snapshot contains operational metrics
  └─ Translator/Pool are SOURCE of truth
  └─ Fire-and-forget send via TCP
  └─ Stats services are PASSIVE RECEIVERS
  └─ No acknowledgment needed
  └─ Mining doesn't wait for stats service

N = CONFIGURABLE via shared config
  - Allows different deployments to tune snapshot frequency
  - Not core mining logic, just stats collection
  - Development: 5s, High-freq: 1s, Battery-limited: 10s
```

### PULL (Web Services → Stats Services → Browser)

```
Web-Proxy/Web-Pool periodically request stats
  ├─ HTTP GET /api/stats from stats service
  ├─ Store snapshot in memory
  └─ When browser requests, serve fresh copy

Browser periodically requests dashboard
  ├─ JavaScript fetch() to web-proxy/web-pool API
  ├─ Parse JSON
  └─ Update DOM every N seconds

N = CONFIGURABLE via shared config
  - Independent timing for frontend refresh
  - Can be different from backend polling
```

### Configuration Strategy

```
Translator/Pool (snapshot generation)
  └─ snapshot_poll_interval_secs = config/shared/{miner|pool}.toml [stats]
  └─ Configurable, not core mining logic

Web-Proxy/Web-Pool (HTTP polling)
  └─ stats_poll_interval_secs = config/shared/{miner|pool}.toml [web_proxy|web_pool]
  └─ Configurable, separate from snapshot generation

Browser (JavaScript)
  └─ client_poll_interval_secs = config/shared/{miner|pool}.toml [web_proxy|web_pool]
  └─ Configurable, independent from backend polling

Stats Services (receivers)
  └─ TCP/HTTP addresses = stats-{proxy|pool}.config.toml
  └─ HTTP timeouts = stats-{proxy|pool}.config.toml
  └─ Staleness thresholds = stats-{proxy|pool}.config.toml
```

---

## Part 9: Tuning Recommendations

### Development Environment
```toml
[stats]
snapshot_poll_interval_secs = 5

[web_proxy]
stats_poll_interval_secs = 3
client_poll_interval_secs = 3

[web_pool]
stats_poll_interval_secs = 3
client_poll_interval_secs = 3
```

### High-Frequency Monitoring (Detailed Metrics)
```toml
[stats]
snapshot_poll_interval_secs = 1  # More detailed stats

[web_proxy]
stats_poll_interval_secs = 1     # Fresher backend data
client_poll_interval_secs = 1    # More responsive UI

[web_pool]
stats_poll_interval_secs = 1     # Fresher backend data
client_poll_interval_secs = 1    # More responsive UI
```

### Battery-Limited / Low-Power Deployments
```toml
[stats]
snapshot_poll_interval_secs = 10  # Less frequent snapshots

[web_proxy]
stats_poll_interval_secs = 10     # Reduce backend polling
client_poll_interval_secs = 10    # Less frequent UI updates

[web_pool]
stats_poll_interval_secs = 10     # Reduce backend polling
client_poll_interval_secs = 10    # Less frequent UI updates
```

### Testing / CI Environment
```toml
[stats]
snapshot_poll_interval_secs = 1   # Fast test feedback

[web_proxy]
stats_poll_interval_secs = 1
client_poll_interval_secs = 1

[web_pool]
stats_poll_interval_secs = 1
client_poll_interval_secs = 1
```

---

## Part 10: Important Notes

1. **Snapshot polling is NOT core mining logic** - It's purely stats collection infrastructure
2. **Stats services are optional** - If `stats_server_address` is not configured, stats polling doesn't start
3. **Backward compatibility maintained** - All new fields have sensible defaults
4. **devenv.nix will be simplified** - No more hardcoded port constants
5. **Production configs separate** - Create `config/prod/` variants with tuned values
6. **Health endpoints use staleness** - Operators can monitor system via `/health` endpoints (currently unused infrastructure)

---

## References

- Stats architecture: See `docs/AGENTS.md` Stats Architecture section
- Translator config: `roles/translator/src/lib/proxy_config.rs`
- Pool config: `roles/pool/src/lib/pool_config.rs`
- Stats client: `roles/roles-utils/stats/src/stats_client.rs`
- Stats poller: `roles/roles-utils/stats/src/stats_poller.rs`
