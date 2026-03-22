# Monitoring Module

HTTP JSON API and Prometheus metrics for SV2 applications.

## API Endpoints

Endpoints returning lists support pagination via `?offset=N&limit=M` query params.

| Endpoint | Description |
|----------|-------------|
| `/swagger-ui` | Swagger UI (interactive API docs) |
| `/api-docs/openapi.json` | OpenAPI specification |
| `/api/v1/health` | Health check |
| `/api/v1/global` | Global statistics |
| `/api/v1/server` | Server metadata |
| `/api/v1/server/channels` | Server channels (paginated) |
| `/api/v1/clients` | All Sv2 clients metadata (paginated) |
| `/api/v1/clients/{id}` | Single Sv2 client metadata |
| `/api/v1/clients/{id}/channels` | Sv2 client channels (paginated) |
| `/api/v1/sv1/clients` | Sv1 clients (Translator Proxy only, paginated) |
| `/api/v1/sv1/clients/{id}` | Single Sv1 client (Translator Proxy only) |
| `/metrics` | Prometheus metrics |

Server and client endpoints return metadata only (counts, hashrate). Use `/channels` sub-resource for channel details.

## Traits

Applications implement these traits on their data structures:

- `ServerMonitoring` - For upstream connection info
- `Sv2ClientsMonitoring` - For Sv2 downstream client info (Pool, JDC)
- `Sv1ClientsMonitoring` - For Sv1 downstream client info (Translator Proxy only)

## Usage

```rust
use stratum_apps::monitoring::MonitoringServer;
use std::sync::Arc;

let server = MonitoringServer::new(
    "127.0.0.1:9090".parse()?,
    Some(Arc::new(channel_manager.clone())), // server monitoring
    Some(Arc::new(channel_manager.clone())), // Sv2 clients monitoring
    std::time::Duration::from_secs(15),      // cache refresh interval
)?;

// For Translator, add SV1 monitoring
let server = server.with_sv1_monitoring(Arc::new(sv1_server.clone()))?;

// Create a shutdown signal (any Future that completes when shutdown is needed)
let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
let shutdown_signal = async move {
    shutdown_rx.recv().await.ok();
};

// Spawn monitoring server
tokio::spawn(async move {
    if let Err(e) = server.run(shutdown_signal).await {
        eprintln!("Monitoring server error: {}", e);
    }
});

// Later, trigger shutdown:
// shutdown_tx.send(()).ok();
```

## Prometheus Metrics

**System:**
- `sv2_uptime_seconds` - Server uptime

**Server:**
- `sv2_server_channels{channel_type}` - Server channels by type (extended/standard)
- `sv2_server_hashrate_total` - Total server hashrate
- `sv2_server_channel_hashrate{channel_id, user_identity}` - Per-channel hashrate
- `sv2_server_shares_accepted_total{channel_id, user_identity}` - Per-channel shares
- `sv2_server_blocks_found_total` - Total blocks found across all current server channels

**Clients:**
- `sv2_clients_total` - Connected client count
- `sv2_client_channels{channel_type}` - Client channels by type (extended/standard)
- `sv2_client_hashrate_total` - Total client hashrate
- `sv2_client_channel_hashrate{client_id, channel_id, user_identity}` - Per-channel hashrate
- `sv2_client_shares_accepted_total{client_id, channel_id, user_identity}` - Per-channel shares
- `sv2_client_blocks_found_total` - Total blocks found across all current client channels

**Sv1 (Translator Proxy only):**
- `sv1_clients_total` - Sv1 client count
- `sv1_hashrate_total` - Sv1 total hashrate
