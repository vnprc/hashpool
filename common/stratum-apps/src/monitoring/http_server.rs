//! HTTP server for exposing monitoring data using Axum

use super::{
    client::{
        ExtendedChannelInfo, StandardChannelInfo, Sv2ClientInfo, Sv2ClientMetadata,
        Sv2ClientsMonitoring, Sv2ClientsSummary,
    },
    prometheus_metrics::PrometheusMetrics,
    server::{
        ServerExtendedChannelInfo, ServerMonitoring, ServerStandardChannelInfo, ServerSummary,
    },
    snapshot_cache::SnapshotCache,
    sv1::{Sv1ClientInfo, Sv1ClientsMonitoring, Sv1ClientsSummary},
    GlobalInfo,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use prometheus::{Encoder, TextEncoder};
use serde::Deserialize;
use std::{
    future::Future,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;
use tracing::info;
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "SRI Monitoring API",
        version = "0.1.0",
        description = "HTTP JSON API for monitoring SV2 applications"
    ),
    paths(
        handle_health,
        handle_global,
        handle_server,
        handle_server_channels,
        handle_clients,
        handle_client_by_id,
        handle_client_channels,
        handle_sv1_clients,
        handle_sv1_client_by_id,
    ),
    components(schemas(
        GlobalInfo,
        ServerSummary,
        Sv2ClientsSummary,
        ServerExtendedChannelInfo,
        ServerStandardChannelInfo,
        Sv2ClientInfo,
        Sv2ClientMetadata,
        ExtendedChannelInfo,
        StandardChannelInfo,
        Sv1ClientInfo,
        Sv1ClientsSummary,
        HealthResponse,
        ErrorResponse,
        ServerResponse,
        ServerChannelsResponse,
        Sv2ClientsResponse,
        Sv2ClientResponse,
        Sv2ClientChannelsResponse,
        Sv1ClientsResponse,
    )),
    tags(
        (name = "health", description = "Health check endpoints"),
        (name = "global", description = "Global statistics"),
        (name = "server", description = "Server (upstream) monitoring"),
        (name = "clients", description = "Clients (downstream) monitoring"),
        (name = "sv1", description = "Sv1 clients monitoring (Translator Proxy only)")
    )
)]
struct ApiDoc;

/// Shared state for all HTTP handlers
#[derive(Clone)]
struct ServerState {
    cache: Arc<SnapshotCache>,
    start_time: u64,
    metrics: PrometheusMetrics,
}

const DEFAULT_LIMIT: usize = 25;
const MAX_LIMIT: usize = 100;

#[derive(Deserialize, IntoParams)]
struct Pagination {
    /// Offset for pagination (default: 0)
    #[serde(default)]
    offset: usize,
    /// Limit for pagination (default: 25, max: 100)
    #[serde(default)]
    limit: Option<usize>,
}

impl Pagination {
    fn effective_limit(&self) -> usize {
        self.limit
            .map(|l| l.min(MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT)
    }
}

fn paginate<T: Clone>(items: &[T], params: &Pagination) -> (usize, Vec<T>) {
    let total = items.len();
    let limit = params.effective_limit();
    let offset = params.offset.min(total);
    let sliced = items
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    (total, sliced)
}

/// HTTP server that exposes monitoring data as JSON
pub struct MonitoringServer {
    bind_address: SocketAddr,
    state: ServerState,
    refresh_interval: Duration,
}

impl MonitoringServer {
    /// Create a new monitoring server with automatic cache refresh.
    ///
    /// This constructor creates a snapshot cache that decouples monitoring API
    /// requests from business logic locks, eliminating the DoS vulnerability where
    /// rapid API requests could cause lock contention with share validation and
    /// job distribution.
    ///
    /// The cache is automatically refreshed in the background at the specified interval.
    ///
    /// # Arguments
    ///
    /// * `bind_address` - Address to bind the HTTP server to
    /// * `server_monitoring` - Optional server (upstream) monitoring trait object
    /// * `sv2_clients_monitoring` - Optional Sv2 clients (downstream) monitoring trait object
    /// * `refresh_interval` - How often to refresh the cache (e.g., Duration::from_secs(15))
    pub fn new(
        bind_address: SocketAddr,
        server_monitoring: Option<Arc<dyn ServerMonitoring + Send + Sync + 'static>>,
        sv2_clients_monitoring: Option<Arc<dyn Sv2ClientsMonitoring + Send + Sync + 'static>>,
        refresh_interval: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let has_server = server_monitoring.is_some();
        let has_sv2_clients = sv2_clients_monitoring.is_some();

        // Create the snapshot cache
        let cache = Arc::new(SnapshotCache::new(
            refresh_interval,
            server_monitoring,
            sv2_clients_monitoring,
        ));

        // Do initial refresh
        cache.refresh();

        let metrics = PrometheusMetrics::new(has_server, has_sv2_clients, false)?;

        Ok(Self {
            bind_address,
            refresh_interval,
            state: ServerState {
                cache,
                start_time,
                metrics,
            },
        })
    }

    /// Add Sv1 clients monitoring (optional, for Translator Proxy only)
    ///
    /// This must be called before `run()` if you want SV1 monitoring.
    pub fn with_sv1_monitoring(
        mut self,
        sv1_monitoring: Arc<dyn Sv1ClientsMonitoring + Send + Sync + 'static>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Determine what sources the cache already has
        let snapshot = self.state.cache.get_snapshot();
        let has_server = snapshot.server_info.is_some();
        let has_sv2_clients = snapshot.sv2_clients_summary.is_some();

        // Add Sv1 clients source to the cache
        let cache = Arc::new(
            Arc::try_unwrap(self.state.cache)
                .unwrap_or_else(|arc| (*arc).clone())
                .with_sv1_clients_source(sv1_monitoring),
        );

        // Refresh cache with new SV1 data
        cache.refresh();

        // Re-create metrics with SV1 enabled
        self.state.metrics = PrometheusMetrics::new(has_server, has_sv2_clients, true)?;
        self.state.cache = cache;

        Ok(self)
    }

    /// Run the monitoring server until the shutdown signal completes
    ///
    /// Starts an HTTP server that exposes monitoring data as JSON.
    /// Also starts a background task that refreshes the snapshot cache periodically.
    /// Both tasks shut down gracefully when `shutdown_signal` completes.
    ///
    /// Automatically exposes:
    /// - Swagger UI at `/swagger-ui`
    /// - OpenAPI spec at `/api-docs/openapi.json`
    /// - Prometheus metrics at `/metrics`
    pub async fn run(
        self,
        shutdown_signal: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting monitoring server on http://{}", self.bind_address);
        info!("Cache refresh interval: {:?}", self.refresh_interval);

        // Spawn background task to refresh cache periodically
        let cache_for_refresh = self.state.cache.clone();
        let refresh_interval = self.refresh_interval;
        let refresh_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(refresh_interval);
            loop {
                interval.tick().await;
                cache_for_refresh.refresh();
            }
        });

        // Versioned JSON API under /api/v1
        let api_v1 = Router::new()
            .route("/health", get(handle_health))
            .route("/global", get(handle_global))
            .route("/server", get(handle_server))
            .route("/server/channels", get(handle_server_channels))
            .route("/clients", get(handle_clients))
            .route("/clients/{client_id}", get(handle_client_by_id))
            .route("/clients/{client_id}/channels", get(handle_client_channels))
            .route("/sv1/clients", get(handle_sv1_clients))
            .route("/sv1/clients/{client_id}", get(handle_sv1_client_by_id));

        let app = Router::new()
            .route("/", get(handle_root))
            .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
            .nest("/api/v1", api_v1)
            .route("/metrics", get(handle_prometheus_metrics))
            .with_state(self.state);

        let listener = TcpListener::bind(self.bind_address).await?;

        info!(
            "Swagger UI available at http://{}/swagger-ui",
            self.bind_address
        );
        info!(
            "Prometheus metrics available at http://{}/metrics",
            self.bind_address
        );

        let server_handle = axum::serve(listener, app).with_graceful_shutdown(async move {
            shutdown_signal.await;
            info!("Monitoring server received shutdown signal, stopping...");
        });

        // Run server and wait for shutdown
        let result = server_handle.await;

        // Stop the refresh task
        refresh_handle.abort();

        info!("Monitoring server stopped");
        result.map_err(|e| e.into())
    }
}

// Response types - used for both actual responses and OpenAPI documentation
#[derive(serde::Serialize, ToSchema)]
struct HealthResponse {
    status: String,
    timestamp: u64,
}

#[derive(serde::Serialize, ToSchema)]
struct ErrorResponse {
    error: String,
}

#[derive(serde::Serialize, ToSchema)]
struct ServerResponse {
    extended_channels_count: usize,
    standard_channels_count: usize,
    total_hashrate: f32,
}

#[derive(serde::Serialize, ToSchema)]
struct ServerChannelsResponse {
    offset: usize,
    limit: usize,
    total_extended: usize,
    total_standard: usize,
    extended_channels: Vec<ServerExtendedChannelInfo>,
    standard_channels: Vec<ServerStandardChannelInfo>,
}

#[derive(serde::Serialize, ToSchema)]
struct Sv2ClientsResponse {
    offset: usize,
    limit: usize,
    total: usize,
    items: Vec<Sv2ClientMetadata>,
}

#[derive(serde::Serialize, ToSchema)]
struct Sv2ClientResponse {
    client_id: usize,
    extended_channels_count: usize,
    standard_channels_count: usize,
    total_hashrate: f32,
}

#[derive(serde::Serialize, ToSchema)]
struct Sv2ClientChannelsResponse {
    client_id: usize,
    offset: usize,
    limit: usize,
    total_extended: usize,
    total_standard: usize,
    extended_channels: Vec<ExtendedChannelInfo>,
    standard_channels: Vec<StandardChannelInfo>,
}

#[derive(serde::Serialize, ToSchema)]
struct Sv1ClientsResponse {
    offset: usize,
    limit: usize,
    total: usize,
    items: Vec<Sv1ClientInfo>,
}

/// Root endpoint - lists all available APIs
async fn handle_root() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "service": "SRI Monitoring API",
        "version": "0.1.0",
        "endpoints": {
            "/": "This endpoint - API listing",
            "/swagger-ui": "Swagger UI (interactive API documentation)",
            "/api-docs/openapi.json": "OpenAPI specification",
            "/api/v1/health": "Health check",
            "/api/v1/global": "Global statistics",
            "/api/v1/server": "Server metadata",
            "/api/v1/server/channels": "Server channels (paginated)",
            "/api/v1/clients": "All Sv2 clients metadata (paginated)",
            "/api/v1/clients/{id}": "Single Sv2 client metadata",
            "/api/v1/clients/{id}/channels": "Sv2 client channels (paginated)",
            "/api/v1/sv1/clients": "Sv1 clients (Translator Proxy only, paginated)",
            "/api/v1/sv1/clients/{id}": "Single Sv1 client (Translator Proxy only)",
            "/metrics": "Prometheus metrics"
        }
    }))
}

/// Health check endpoint
#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse)
    )
)]
async fn handle_health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    })
}

/// Get global statistics
///
/// Returns aggregated statistics for the server (upstream) and clients (downstream).
/// Fields are omitted from the response if that type of monitoring is not enabled.
///
/// **Typical responses:**
/// - **Pool/JDC**: `server` + `clients` (Sv2 downstream)
/// - **tProxy**: `server` + `sv1_clients` (Sv1 miners)
#[utoipa::path(
    get,
    path = "/api/v1/global",
    tag = "global",
    responses(
        (status = 200, description = "Global statistics", body = GlobalInfo)
    )
)]
async fn handle_global(State(state): State<ServerState>) -> Json<GlobalInfo> {
    let uptime_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        - state.start_time;

    let snapshot = state.cache.get_snapshot();

    Json(GlobalInfo {
        server: snapshot.server_summary,
        sv2_clients: snapshot.sv2_clients_summary,
        sv1_clients: snapshot.sv1_clients_summary,
        uptime_secs,
    })
}

/// Get server (upstream) metadata - use /server/channels for channel details
#[utoipa::path(
    get,
    path = "/api/v1/server",
    tag = "server",
    responses(
        (status = 200, description = "Server metadata", body = ServerResponse),
        (status = 404, description = "Server monitoring not available", body = ErrorResponse)
    )
)]
async fn handle_server(State(state): State<ServerState>) -> Response {
    let snapshot = state.cache.get_snapshot();

    match snapshot.server_summary {
        Some(summary) => Json(ServerResponse {
            extended_channels_count: summary.extended_channels,
            standard_channels_count: summary.standard_channels,
            total_hashrate: summary.total_hashrate,
        })
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Server monitoring not available".to_string(),
            }),
        )
            .into_response(),
    }
}

/// Get server channels (paginated)
#[utoipa::path(
    get,
    path = "/api/v1/server/channels",
    tag = "server",
    params(Pagination),
    responses(
        (status = 200, description = "Server channels (paginated)", body = ServerChannelsResponse),
        (status = 404, description = "Server monitoring not available", body = ErrorResponse)
    )
)]
async fn handle_server_channels(
    Query(params): Query<Pagination>,
    State(state): State<ServerState>,
) -> Response {
    let snapshot = state.cache.get_snapshot();

    match snapshot.server_info {
        Some(server) => {
            let (total_extended, extended_channels) = paginate(&server.extended_channels, &params);
            let (total_standard, standard_channels) = paginate(&server.standard_channels, &params);

            Json(ServerChannelsResponse {
                offset: params.offset,
                limit: params.effective_limit(),
                total_extended,
                total_standard,
                extended_channels,
                standard_channels,
            })
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Server monitoring not available".to_string(),
            }),
        )
            .into_response(),
    }
}

/// Get all Sv2 clients (downstream) - returns metadata only, use /clients/{id}/channels for
/// channels
#[utoipa::path(
    get,
    path = "/api/v1/clients",
    tag = "clients",
    params(Pagination),
    responses(
        (status = 200, description = "List of Sv2 clients (metadata only)", body = Sv2ClientsResponse),
        (status = 404, description = "Sv2 clients monitoring not available", body = ErrorResponse)
    )
)]
async fn handle_clients(
    Query(params): Query<Pagination>,
    State(state): State<ServerState>,
) -> Response {
    let snapshot = state.cache.get_snapshot();

    match snapshot.sv2_clients {
        Some(ref sv2_clients) => {
            let metadata: Vec<Sv2ClientMetadata> =
                sv2_clients.iter().map(|c| c.to_metadata()).collect();
            let (total, items) = paginate(&metadata, &params);

            Json(Sv2ClientsResponse {
                offset: params.offset,
                limit: params.effective_limit(),
                total,
                items,
            })
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Sv2 clients monitoring not available".to_string(),
            }),
        )
            .into_response(),
    }
}

/// Get a single Sv2 client by ID - returns metadata only, use /clients/{id}/channels for channels
#[utoipa::path(
    get,
    path = "/api/v1/clients/{client_id}",
    tag = "clients",
    params(
        ("client_id" = usize, Path, description = "Sv2 Client ID")
    ),
    responses(
        (status = 200, description = "Sv2 client metadata", body = Sv2ClientResponse),
        (status = 404, description = "Sv2 client not found", body = ErrorResponse)
    )
)]
async fn handle_client_by_id(
    Path(client_id): Path<usize>,
    State(state): State<ServerState>,
) -> Response {
    let snapshot = state.cache.get_snapshot();

    let sv2_clients = match snapshot.sv2_clients {
        Some(ref clients) => clients,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Sv2 clients monitoring not available".to_string(),
                }),
            )
                .into_response();
        }
    };

    match sv2_clients.iter().find(|c| c.client_id == client_id) {
        Some(client) => Json(Sv2ClientResponse {
            client_id,
            extended_channels_count: client.extended_channels.len(),
            standard_channels_count: client.standard_channels.len(),
            total_hashrate: client.total_hashrate(),
        })
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Sv2 client {} not found", client_id),
            }),
        )
            .into_response(),
    }
}

/// Get channels for a specific Sv2 client (paginated)
#[utoipa::path(
    get,
    path = "/api/v1/clients/{client_id}/channels",
    tag = "clients",
    params(
        ("client_id" = usize, Path, description = "Sv2 Client ID"),
        Pagination
    ),
    responses(
        (status = 200, description = "Sv2 client channels (paginated)", body = Sv2ClientChannelsResponse),
        (status = 404, description = "Sv2 client not found", body = ErrorResponse)
    )
)]
async fn handle_client_channels(
    Path(client_id): Path<usize>,
    Query(params): Query<Pagination>,
    State(state): State<ServerState>,
) -> Response {
    let snapshot = state.cache.get_snapshot();

    let sv2_clients = match snapshot.sv2_clients {
        Some(ref clients) => clients,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Sv2 clients monitoring not available".to_string(),
                }),
            )
                .into_response();
        }
    };

    match sv2_clients.iter().find(|c| c.client_id == client_id) {
        Some(client) => {
            let (total_extended, extended_channels) = paginate(&client.extended_channels, &params);
            let (total_standard, standard_channels) = paginate(&client.standard_channels, &params);

            Json(Sv2ClientChannelsResponse {
                client_id,
                offset: params.offset,
                limit: params.effective_limit(),
                total_extended,
                total_standard,
                extended_channels,
                standard_channels,
            })
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Sv2 client {} not found", client_id),
            }),
        )
            .into_response(),
    }
}

/// Get Sv1 clients (Translator Proxy only)
#[utoipa::path(
    get,
    path = "/api/v1/sv1/clients",
    tag = "sv1",
    params(Pagination),
    responses(
        (status = 200, description = "List of Sv1 clients", body = Sv1ClientsResponse),
        (status = 404, description = "Sv1 monitoring not available", body = ErrorResponse)
    )
)]
async fn handle_sv1_clients(
    Query(params): Query<Pagination>,
    State(state): State<ServerState>,
) -> Response {
    let snapshot = state.cache.get_snapshot();

    match snapshot.sv1_clients {
        Some(ref sv1_clients) => {
            let (total, items) = paginate(sv1_clients, &params);

            Json(Sv1ClientsResponse {
                offset: params.offset,
                limit: params.effective_limit(),
                total,
                items,
            })
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Sv1 client monitoring not available".to_string(),
            }),
        )
            .into_response(),
    }
}

/// Get a single Sv1 client by ID
#[utoipa::path(
    get,
    path = "/api/v1/sv1/clients/{client_id}",
    tag = "sv1",
    params(
        ("client_id" = usize, Path, description = "Sv1 client ID")
    ),
    responses(
        (status = 200, description = "Sv1 client details", body = Sv1ClientInfo),
        (status = 404, description = "Sv1 client not found", body = ErrorResponse)
    )
)]
async fn handle_sv1_client_by_id(
    Path(client_id): Path<usize>,
    State(state): State<ServerState>,
) -> Response {
    let snapshot = state.cache.get_snapshot();

    let sv1_clients = match snapshot.sv1_clients {
        Some(ref clients) => clients,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Sv1 client monitoring not available".to_string(),
                }),
            )
                .into_response();
        }
    };

    match sv1_clients.iter().find(|c| c.client_id == client_id) {
        Some(client) => Json(client.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Sv1 client {} not found", client_id),
            }),
        )
            .into_response(),
    }
}

/// Handler for Prometheus metrics endpoint
async fn handle_prometheus_metrics(State(state): State<ServerState>) -> Response {
    let snapshot = state.cache.get_snapshot();

    let uptime_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        - state.start_time;
    state.metrics.sv2_uptime_seconds.set(uptime_secs as f64);

    // Reset per-channel metrics before repopulating
    if let Some(ref metric) = state.metrics.sv2_client_channel_hashrate {
        metric.reset();
    }
    if let Some(ref metric) = state.metrics.sv2_client_shares_accepted_total {
        metric.reset();
    }
    if let Some(ref metric) = state.metrics.sv2_server_channel_hashrate {
        metric.reset();
    }
    if let Some(ref metric) = state.metrics.sv2_server_shares_accepted_total {
        metric.reset();
    }

    // Collect server metrics
    if let Some(ref summary) = snapshot.server_summary {
        if let Some(ref metric) = state.metrics.sv2_server_channels {
            metric
                .with_label_values(&["extended"])
                .set(summary.extended_channels as f64);
            metric
                .with_label_values(&["standard"])
                .set(summary.standard_channels as f64);
        }
        if let Some(ref metric) = state.metrics.sv2_server_hashrate_total {
            metric.set(summary.total_hashrate as f64);
        }
    }

    if let Some(ref server) = snapshot.server_info {
        for channel in &server.extended_channels {
            let channel_id = channel.channel_id.to_string();
            let user = &channel.user_identity;

            if let Some(ref metric) = state.metrics.sv2_server_shares_accepted_total {
                metric
                    .with_label_values(&[&channel_id, user])
                    .set(channel.shares_acknowledged as f64);
            }
            if let (Some(ref metric), Some(hashrate)) = (
                &state.metrics.sv2_server_channel_hashrate,
                channel.nominal_hashrate,
            ) {
                metric
                    .with_label_values(&[&channel_id, user])
                    .set(hashrate as f64);
            }
        }

        for channel in &server.standard_channels {
            let channel_id = channel.channel_id.to_string();
            let user = &channel.user_identity;

            if let Some(ref metric) = state.metrics.sv2_server_shares_accepted_total {
                metric
                    .with_label_values(&[&channel_id, user])
                    .set(channel.shares_accepted as f64);
            }
            if let (Some(ref metric), Some(hashrate)) = (
                &state.metrics.sv2_server_channel_hashrate,
                channel.nominal_hashrate,
            ) {
                metric
                    .with_label_values(&[&channel_id, user])
                    .set(hashrate as f64);
            }
        }

        if let Some(ref metric) = state.metrics.sv2_server_blocks_found_total {
            let total: u64 = server
                .extended_channels
                .iter()
                .map(|c| c.blocks_found as u64)
                .chain(
                    server
                        .standard_channels
                        .iter()
                        .map(|c| c.blocks_found as u64),
                )
                .sum();
            metric.set(total as f64);
        }
    }

    // Collect Sv2 clients metrics
    if let Some(ref summary) = snapshot.sv2_clients_summary {
        if let Some(ref metric) = state.metrics.sv2_clients_total {
            metric.set(summary.total_clients as f64);
        }
        if let Some(ref metric) = state.metrics.sv2_client_channels {
            metric
                .with_label_values(&["extended"])
                .set(summary.extended_channels as f64);
            metric
                .with_label_values(&["standard"])
                .set(summary.standard_channels as f64);
        }
        if let Some(ref metric) = state.metrics.sv2_client_hashrate_total {
            metric.set(summary.total_hashrate as f64);
        }

        let mut client_blocks_total: u64 = 0;

        for client in snapshot.sv2_clients.as_deref().unwrap_or(&[]) {
            let client_id = client.client_id.to_string();

            for channel in &client.extended_channels {
                let channel_id = channel.channel_id.to_string();
                let user = &channel.user_identity;

                if let Some(ref metric) = state.metrics.sv2_client_shares_accepted_total {
                    metric
                        .with_label_values(&[&client_id, &channel_id, user])
                        .set(channel.shares_accepted as f64);
                }
                if let Some(ref metric) = state.metrics.sv2_client_channel_hashrate {
                    metric
                        .with_label_values(&[&client_id, &channel_id, user])
                        .set(channel.nominal_hashrate as f64);
                }
                client_blocks_total += channel.blocks_found as u64;
            }

            for channel in &client.standard_channels {
                let channel_id = channel.channel_id.to_string();
                let user = &channel.user_identity;

                if let Some(ref metric) = state.metrics.sv2_client_shares_accepted_total {
                    metric
                        .with_label_values(&[&client_id, &channel_id, user])
                        .set(channel.shares_accepted as f64);
                }
                if let Some(ref metric) = state.metrics.sv2_client_channel_hashrate {
                    metric
                        .with_label_values(&[&client_id, &channel_id, user])
                        .set(channel.nominal_hashrate as f64);
                }
                client_blocks_total += channel.blocks_found as u64;
            }
        }

        if let Some(ref metric) = state.metrics.sv2_client_blocks_found_total {
            metric.set(client_blocks_total as f64);
        }
    }

    // Collect SV1 client metrics
    if let Some(ref summary) = snapshot.sv1_clients_summary {
        if let Some(ref metric) = state.metrics.sv1_clients_total {
            metric.set(summary.total_clients as f64);
        }
        if let Some(ref metric) = state.metrics.sv1_hashrate_total {
            metric.set(summary.total_hashrate as f64);
        }
    }

    // Encode and return metrics
    let encoder = TextEncoder::new();
    let metric_families = state.metrics.registry.gather();
    let mut buffer = Vec::new();

    match encoder.encode(&metric_families, &mut buffer) {
        Ok(_) => match String::from_utf8(buffer) {
            Ok(metrics_text) => (StatusCode::OK, metrics_text).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("UTF-8 error: {}", e),
                }),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Encoding error: {}", e),
            }),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // ── helpers ──────────────────────────────────────────────────────

    fn create_extended_channel_info(
        channel_id: u32,
        hashrate: f32,
    ) -> super::super::client::ExtendedChannelInfo {
        super::super::client::ExtendedChannelInfo {
            channel_id,
            user_identity: format!("user-ext-{}", channel_id),
            nominal_hashrate: hashrate,
            target_hex: "00ff".into(),
            requested_max_target_hex: "00ff".into(),
            extranonce_prefix_hex: "aa".into(),
            full_extranonce_size: 16,
            rollable_extranonce_size: 4,
            expected_shares_per_minute: 1.0,
            shares_accepted: 10,
            share_work_sum: 100.0,
            last_share_sequence_number: 5,
            best_diff: 50.0,
            last_batch_accepted: 3,
            last_batch_work_sum: 30.0,
            share_batch_size: 10,
            blocks_found: 0,
        }
    }

    fn create_standard_channel_info(
        channel_id: u32,
        hashrate: f32,
    ) -> super::super::client::StandardChannelInfo {
        super::super::client::StandardChannelInfo {
            channel_id,
            user_identity: format!("user-std-{}", channel_id),
            nominal_hashrate: hashrate,
            target_hex: "00ff".into(),
            requested_max_target_hex: "00ff".into(),
            extranonce_prefix_hex: "bb".into(),
            expected_shares_per_minute: 2.0,
            shares_accepted: 20,
            share_work_sum: 200.0,
            last_share_sequence_number: 8,
            best_diff: 80.0,
            last_batch_accepted: 5,
            last_batch_work_sum: 50.0,
            share_batch_size: 20,
            blocks_found: 0,
        }
    }

    fn create_server_extended_channel_info(
        channel_id: u32,
        hashrate: Option<f32>,
    ) -> ServerExtendedChannelInfo {
        ServerExtendedChannelInfo {
            channel_id,
            user_identity: format!("pool-ext-{}", channel_id),
            nominal_hashrate: hashrate,
            target_hex: "00ff".into(),
            extranonce_prefix_hex: "aa".into(),
            full_extranonce_size: 16,
            rollable_extranonce_size: 4,
            version_rolling: true,
            shares_acknowledged: 10,
            shares_rejected: 0,
            share_work_sum: 100.0,
            shares_submitted: 12,
            best_diff: 50.0,
            blocks_found: 0,
        }
    }

    fn create_server_standard_channel_info(
        channel_id: u32,
        hashrate: Option<f32>,
    ) -> ServerStandardChannelInfo {
        ServerStandardChannelInfo {
            channel_id,
            user_identity: format!("pool-std-{}", channel_id),
            nominal_hashrate: hashrate,
            target_hex: "00ff".into(),
            extranonce_prefix_hex: "bb".into(),
            shares_accepted: 20,
            share_work_sum: 200.0,
            shares_submitted: 22,
            best_diff: 80.0,
            blocks_found: 0,
        }
    }

    fn create_sv1_client_info(id: usize, hashrate: Option<f32>) -> Sv1ClientInfo {
        Sv1ClientInfo {
            client_id: id,
            channel_id: Some(id as u32),
            authorized_worker_name: format!("worker-{}", id),
            user_identity: format!("miner-{}", id),
            target_hex: "00ff".into(),
            hashrate,
            extranonce1_hex: "aabb".into(),
            extranonce2_len: 8,
            version_rolling_mask: Some("ffffffff".into()),
            version_rolling_min_bit: Some("00000000".into()),
        }
    }

    struct MockServer(super::super::server::ServerInfo);
    impl ServerMonitoring for MockServer {
        fn get_server(&self) -> super::super::server::ServerInfo {
            self.0.clone()
        }
    }

    struct MockClients(Vec<Sv2ClientInfo>);
    impl super::super::client::Sv2ClientsMonitoring for MockClients {
        fn get_sv2_clients(&self) -> Vec<Sv2ClientInfo> {
            self.0.clone()
        }
    }

    struct MockSv1Clients(Vec<Sv1ClientInfo>);
    impl super::super::sv1::Sv1ClientsMonitoring for MockSv1Clients {
        fn get_sv1_clients(&self) -> Vec<Sv1ClientInfo> {
            self.0.clone()
        }
    }

    /// Build a full Router with mock data for integration testing.
    fn build_test_app(
        server: Option<Arc<dyn ServerMonitoring + Send + Sync>>,
        clients: Option<Arc<dyn super::super::client::Sv2ClientsMonitoring + Send + Sync>>,
        sv1: Option<Arc<dyn super::super::sv1::Sv1ClientsMonitoring + Send + Sync>>,
    ) -> Router {
        let cache = Arc::new(SnapshotCache::new(Duration::from_secs(60), server, clients));

        let cache = if let Some(sv1_source) = sv1 {
            Arc::new(
                Arc::try_unwrap(cache)
                    .unwrap_or_else(|arc| (*arc).clone())
                    .with_sv1_clients_source(sv1_source),
            )
        } else {
            cache
        };

        cache.refresh();

        let has_server = cache.get_snapshot().server_info.is_some();
        let has_clients = cache.get_snapshot().sv2_clients_summary.is_some();
        let has_sv1 = cache.get_snapshot().sv1_clients.is_some();

        let metrics = PrometheusMetrics::new(has_server, has_clients, has_sv1).unwrap();

        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let state = ServerState {
            cache,
            start_time,
            metrics,
        };

        let api_v1 = Router::new()
            .route("/health", get(handle_health))
            .route("/global", get(handle_global))
            .route("/server", get(handle_server))
            .route("/server/channels", get(handle_server_channels))
            .route("/clients", get(handle_clients))
            .route("/clients/{client_id}", get(handle_client_by_id))
            .route("/clients/{client_id}/channels", get(handle_client_channels))
            .route("/sv1/clients", get(handle_sv1_clients))
            .route("/sv1/clients/{client_id}", get(handle_sv1_client_by_id));

        Router::new()
            .route("/", get(handle_root))
            .nest("/api/v1", api_v1)
            .route("/metrics", get(handle_prometheus_metrics))
            .with_state(state)
    }

    async fn get_body(response: axum::response::Response) -> String {
        let body = response.into_body();
        let bytes = body.collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn make_request(uri: &str) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    // ── Pagination unit tests ───────────────────────────────────────

    #[test]
    fn pagination_effective_limit_default() {
        let p = Pagination {
            offset: 0,
            limit: None,
        };
        assert_eq!(p.effective_limit(), DEFAULT_LIMIT);
    }

    #[test]
    fn pagination_effective_limit_capped_at_max() {
        let p = Pagination {
            offset: 0,
            limit: Some(500),
        };
        assert_eq!(p.effective_limit(), MAX_LIMIT);
    }

    #[test]
    fn pagination_effective_limit_respects_small_value() {
        let p = Pagination {
            offset: 0,
            limit: Some(5),
        };
        assert_eq!(p.effective_limit(), 5);
    }

    #[test]
    fn paginate_empty_slice() {
        let items: Vec<i32> = vec![];
        let params = Pagination {
            offset: 0,
            limit: Some(10),
        };
        let (total, result) = paginate(&items, &params);
        assert_eq!(total, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn paginate_basic() {
        let items: Vec<i32> = (0..50).collect();
        let params = Pagination {
            offset: 10,
            limit: Some(5),
        };
        let (total, result) = paginate(&items, &params);
        assert_eq!(total, 50);
        assert_eq!(result, vec![10, 11, 12, 13, 14]);
    }

    #[test]
    fn paginate_offset_beyond_length() {
        let items: Vec<i32> = vec![1, 2, 3];
        let params = Pagination {
            offset: 100,
            limit: Some(10),
        };
        let (total, result) = paginate(&items, &params);
        assert_eq!(total, 3);
        assert!(result.is_empty());
    }

    #[test]
    fn paginate_limit_exceeds_remaining() {
        let items: Vec<i32> = vec![1, 2, 3, 4, 5];
        let params = Pagination {
            offset: 3,
            limit: Some(10),
        };
        let (total, result) = paginate(&items, &params);
        assert_eq!(total, 5);
        assert_eq!(result, vec![4, 5]);
    }

    // ── HTTP endpoint integration tests ─────────────────────────────

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let app = build_test_app(None, None, None);
        let response = app.oneshot(make_request("/api/v1/health")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["timestamp"].as_u64().is_some());
    }

    #[tokio::test]
    async fn root_endpoint_lists_endpoints() {
        let app = build_test_app(None, None, None);
        let response = app.oneshot(make_request("/")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["service"], "SRI Monitoring API");
        assert!(json["endpoints"].is_object());
    }

    #[tokio::test]
    async fn global_endpoint_with_no_sources() {
        let app = build_test_app(None, None, None);
        let response = app.oneshot(make_request("/api/v1/global")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["server"].is_null());
        assert!(json["sv2_clients"].is_null());
        assert!(json["uptime_secs"].as_u64().is_some());
    }

    #[tokio::test]
    async fn global_endpoint_with_data() {
        let server = Arc::new(MockServer(super::super::server::ServerInfo {
            extended_channels: vec![create_server_extended_channel_info(1, Some(100.0))],
            standard_channels: vec![],
        }));
        let clients = Arc::new(MockClients(vec![Sv2ClientInfo {
            client_id: 1,
            extended_channels: vec![create_extended_channel_info(1, 50.0)],
            standard_channels: vec![],
        }]));

        let app = build_test_app(
            Some(server as Arc<dyn ServerMonitoring + Send + Sync>),
            Some(clients as Arc<dyn super::super::client::Sv2ClientsMonitoring + Send + Sync>),
            None,
        );
        let response = app.oneshot(make_request("/api/v1/global")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["server"]["extended_channels"], 1);
        assert_eq!(json["sv2_clients"]["total_clients"], 1);
    }

    #[tokio::test]
    async fn server_endpoint_not_available() {
        let app = build_test_app(None, None, None);
        let response = app.oneshot(make_request("/api/v1/server")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn server_endpoint_with_data() {
        let server = Arc::new(MockServer(super::super::server::ServerInfo {
            extended_channels: vec![create_server_extended_channel_info(1, Some(100.0))],
            standard_channels: vec![create_server_standard_channel_info(2, Some(50.0))],
        }));

        let app = build_test_app(
            Some(server as Arc<dyn ServerMonitoring + Send + Sync>),
            None,
            None,
        );
        let response = app.oneshot(make_request("/api/v1/server")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["extended_channels_count"], 1);
        assert_eq!(json["standard_channels_count"], 1);
    }

    #[tokio::test]
    async fn server_channels_endpoint_with_pagination() {
        let server = Arc::new(MockServer(super::super::server::ServerInfo {
            extended_channels: vec![
                create_server_extended_channel_info(1, Some(100.0)),
                create_server_extended_channel_info(2, Some(200.0)),
                create_server_extended_channel_info(3, Some(300.0)),
            ],
            standard_channels: vec![],
        }));

        let app = build_test_app(
            Some(server as Arc<dyn ServerMonitoring + Send + Sync>),
            None,
            None,
        );
        let response = app
            .oneshot(make_request("/api/v1/server/channels?offset=1&limit=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["total_extended"], 3);
        assert_eq!(json["offset"], 1);
        assert_eq!(json["limit"], 1);
        assert_eq!(json["extended_channels"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn clients_endpoint_not_available() {
        let app = build_test_app(None, None, None);
        let response = app.oneshot(make_request("/api/v1/clients")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn clients_endpoint_returns_metadata() {
        let clients = Arc::new(MockClients(vec![
            Sv2ClientInfo {
                client_id: 1,
                extended_channels: vec![create_extended_channel_info(1, 100.0)],
                standard_channels: vec![],
            },
            Sv2ClientInfo {
                client_id: 2,
                extended_channels: vec![],
                standard_channels: vec![create_standard_channel_info(1, 50.0)],
            },
        ]));

        let app = build_test_app(
            None,
            Some(clients as Arc<dyn super::super::client::Sv2ClientsMonitoring + Send + Sync>),
            None,
        );
        let response = app.oneshot(make_request("/api/v1/clients")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["total"], 2);
        assert_eq!(json["items"].as_array().unwrap().len(), 2);
        assert_eq!(json["items"][0]["client_id"], 1);
    }

    #[tokio::test]
    async fn client_by_id_found() {
        let clients = Arc::new(MockClients(vec![Sv2ClientInfo {
            client_id: 42,
            extended_channels: vec![create_extended_channel_info(1, 100.0)],
            standard_channels: vec![create_standard_channel_info(2, 50.0)],
        }]));

        let app = build_test_app(
            None,
            Some(clients as Arc<dyn super::super::client::Sv2ClientsMonitoring + Send + Sync>),
            None,
        );
        let response = app
            .oneshot(make_request("/api/v1/clients/42"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["client_id"], 42);
        assert_eq!(json["extended_channels_count"], 1);
        assert_eq!(json["standard_channels_count"], 1);
    }

    #[tokio::test]
    async fn client_by_id_not_found() {
        let clients = Arc::new(MockClients(vec![Sv2ClientInfo {
            client_id: 1,
            extended_channels: vec![],
            standard_channels: vec![],
        }]));

        let app = build_test_app(
            None,
            Some(clients as Arc<dyn super::super::client::Sv2ClientsMonitoring + Send + Sync>),
            None,
        );
        let response = app
            .oneshot(make_request("/api/v1/clients/999"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn client_channels_with_pagination() {
        let clients = Arc::new(MockClients(vec![Sv2ClientInfo {
            client_id: 1,
            extended_channels: vec![
                create_extended_channel_info(10, 100.0),
                create_extended_channel_info(11, 200.0),
                create_extended_channel_info(12, 300.0),
            ],
            standard_channels: vec![create_standard_channel_info(20, 50.0)],
        }]));

        let app = build_test_app(
            None,
            Some(clients as Arc<dyn super::super::client::Sv2ClientsMonitoring + Send + Sync>),
            None,
        );
        let response = app
            .oneshot(make_request("/api/v1/clients/1/channels?offset=1&limit=2"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["client_id"], 1);
        assert_eq!(json["total_extended"], 3);
        assert_eq!(json["extended_channels"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn sv1_clients_not_available() {
        let app = build_test_app(None, None, None);
        let response = app
            .oneshot(make_request("/api/v1/sv1/clients"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn sv1_clients_with_data() {
        let sv1 = Arc::new(MockSv1Clients(vec![
            create_sv1_client_info(1, Some(100.0)),
            create_sv1_client_info(2, Some(200.0)),
        ]));

        let app = build_test_app(
            None,
            None,
            Some(sv1 as Arc<dyn super::super::sv1::Sv1ClientsMonitoring + Send + Sync>),
        );
        let response = app
            .oneshot(make_request("/api/v1/sv1/clients"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["total"], 2);
        assert_eq!(json["items"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn sv1_client_by_id_found() {
        let sv1 = Arc::new(MockSv1Clients(vec![create_sv1_client_info(7, Some(500.0))]));

        let app = build_test_app(
            None,
            None,
            Some(sv1 as Arc<dyn super::super::sv1::Sv1ClientsMonitoring + Send + Sync>),
        );
        let response = app
            .oneshot(make_request("/api/v1/sv1/clients/7"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["client_id"], 7);
    }

    #[tokio::test]
    async fn sv1_client_by_id_not_found() {
        let sv1 = Arc::new(MockSv1Clients(vec![create_sv1_client_info(1, Some(100.0))]));

        let app = build_test_app(
            None,
            None,
            Some(sv1 as Arc<dyn super::super::sv1::Sv1ClientsMonitoring + Send + Sync>),
        );
        let response = app
            .oneshot(make_request("/api/v1/sv1/clients/999"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_prometheus_format() {
        let server = Arc::new(MockServer(super::super::server::ServerInfo {
            extended_channels: vec![create_server_extended_channel_info(1, Some(100.0))],
            standard_channels: vec![],
        }));
        let clients = Arc::new(MockClients(vec![Sv2ClientInfo {
            client_id: 1,
            extended_channels: vec![create_extended_channel_info(1, 50.0)],
            standard_channels: vec![],
        }]));

        let app = build_test_app(
            Some(server as Arc<dyn ServerMonitoring + Send + Sync>),
            Some(clients as Arc<dyn super::super::client::Sv2ClientsMonitoring + Send + Sync>),
            None,
        );
        let response = app.oneshot(make_request("/metrics")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        assert!(body.contains("sv2_uptime_seconds"));
        assert!(body.contains("sv2_server_channels"));
        assert!(body.contains("sv2_clients_total"));
    }

    #[tokio::test]
    async fn metrics_endpoint_with_no_sources() {
        let app = build_test_app(None, None, None);
        let response = app.oneshot(make_request("/metrics")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = get_body(response).await;
        // Uptime is always present
        assert!(body.contains("sv2_uptime_seconds"));
        // Server/client metrics should NOT be present when sources are None
        assert!(!body.contains("sv2_server_channels"));
        assert!(!body.contains("sv2_clients_total"));
    }
}
