use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tracing::{info, warn};

use crate::prometheus::PrometheusClient;
use web_assets::icons::{nav_icon_css, pickaxe_favicon_inline_svg};
use web_utils::format_elapsed_time;

static DASHBOARD_PAGE_HTML: OnceLock<String> = OnceLock::new();

const DASHBOARD_PAGE_TEMPLATE: &str = include_str!("../templates/dashboard.html");
const HASHRATE_SMOOTHING_WINDOW_SECS: u64 = 300;

#[derive(Clone)]
pub struct AppState {
    pub prometheus: PrometheusClient,
    pub client_poll_interval_secs: u64,
    pub metrics_query_step_secs: u64,
}

#[derive(Deserialize)]
pub struct TimeRangeQuery {
    pub from: u64,
    pub to: u64,
}

#[derive(Default)]
struct DownstreamMetrics {
    id: u32,
    address: String,
    work_selection: bool,
    shares_submitted: u64,
    quotes_created: u64,
    ehash_mined: u64,
    last_share_at: Option<u64>,
}

pub async fn run_http_server(
    address: String,
    prometheus: PrometheusClient,
    client_poll_interval_secs: u64,
    metrics_query_step_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState {
        prometheus,
        client_poll_interval_secs,
        metrics_query_step_secs: metrics_query_step_secs.max(1),
    };

    let app = Router::new()
        .route("/favicon.ico", get(serve_favicon))
        .route("/favicon.svg", get(serve_favicon))
        .route("/", get(dashboard_page_handler))
        .route("/api/stats", get(api_stats_handler))
        .route("/api/services", get(api_services_handler))
        .route("/api/connections", get(api_connections_handler))
        .route("/api/hashrate", get(api_aggregate_hashrate_handler))
        .route(
            "/api/downstream/{id}/hashrate",
            get(api_downstream_hashrate_handler),
        )
        .route("/health", get(health_handler))
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind(&address).await?;
    info!("🌐 Web pool listening on http://{}", address);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn serve_favicon() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "image/svg+xml")],
        pickaxe_favicon_inline_svg(),
    )
}

async fn dashboard_page_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let interval_ms = state.client_poll_interval_secs * 1000;
    let html = DASHBOARD_PAGE_HTML.get_or_init(|| {
        DASHBOARD_PAGE_TEMPLATE
            .replace("/* {{NAV_ICON_CSS}} */", nav_icon_css())
            .replace("{client_poll_interval_ms}", &interval_ms.to_string())
    });
    Html(html.clone())
}

async fn api_stats_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let (listen_address, services, proxies) = match fetch_pool_snapshot(&state.prometheus).await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            warn!("Failed to fetch pool snapshot: {}", err);
            ("".to_string(), Vec::new(), Vec::new())
        }
    };

    let now = unix_timestamp();
    Json(json!({
        "listen_address": listen_address,
        "services": services,
        "downstream_proxies": proxies,
        "timestamp": now,
    }))
}

async fn api_services_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let services = match fetch_services(&state.prometheus).await {
        Ok(services) => services,
        Err(err) => {
            warn!("Failed to fetch services: {}", err);
            Vec::new()
        }
    };
    Json(json!({ "services": services }))
}

async fn api_connections_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let proxies = match fetch_downstreams(&state.prometheus).await {
        Ok(mut proxies) => {
            let now = unix_timestamp();
            proxies
                .drain(..)
                .map(|proxy| {
                    let last_share = proxy
                        .last_share_at
                        .map(|ts| format_elapsed_time(now, ts));
                    json!({
                        "id": proxy.id,
                        "address": proxy.address,
                        "channels": Vec::<u32>::new(),
                        "shares_submitted": proxy.shares_submitted,
                        "quotes_created": proxy.quotes_created,
                        "ehash_mined": proxy.ehash_mined,
                        "last_share_at": last_share,
                        "work_selection": proxy.work_selection,
                    })
                })
                .collect::<Vec<_>>()
        }
        Err(err) => {
            warn!("Failed to fetch downstream connections: {}", err);
            Vec::new()
        }
    };

    Json(json!({ "proxies": proxies }))
}

async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let healthy = match state
        .prometheus
        .query_instant("hashpool_pool_info")
        .await
    {
        Ok(results) => !results.is_empty(),
        Err(_) => false,
    };

    let status_code = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let json_response = json!({
        "healthy": healthy,
        "stale": !healthy,
    });

    (status_code, Json(json_response))
}

async fn api_aggregate_hashrate_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TimeRangeQuery>,
) -> impl IntoResponse {
    let window = format!("{}s", HASHRATE_SMOOTHING_WINDOW_SECS);
    let query = format!(
        "sum(avg_over_time(hashpool_pool_downstream_hashrate_hs[{}]))",
        window
    );
    let result = state
        .prometheus
        .query_range(&query, params.from, params.to, state.metrics_query_step_secs)
        .await;

    let data = match result {
        Ok(series) => series
            .first()
            .map(|sample| sample.values.iter().map(range_point_to_hashrate).collect())
            .unwrap_or_else(Vec::new),
        Err(err) => {
            warn!("Failed to fetch aggregate hashrate: {}", err);
            Vec::new()
        }
    };

    (StatusCode::OK, Json(json!({ "data": data }))).into_response()
}

async fn api_downstream_hashrate_handler(
    State(state): State<Arc<AppState>>,
    Path(downstream_id): Path<u32>,
    Query(params): Query<TimeRangeQuery>,
) -> impl IntoResponse {
    let window = format!("{}s", HASHRATE_SMOOTHING_WINDOW_SECS);
    let query = format!(
        "avg_over_time(hashpool_pool_downstream_hashrate_hs{{downstream_id=\"{}\"}}[{}])",
        downstream_id, window
    );

    let result = state
        .prometheus
        .query_range(&query, params.from, params.to, state.metrics_query_step_secs)
        .await;

    let data = match result {
        Ok(series) => series
            .first()
            .map(|sample| sample.values.iter().map(range_point_to_hashrate).collect())
            .unwrap_or_else(Vec::new),
        Err(err) => {
            warn!("Failed to fetch downstream hashrate: {}", err);
            Vec::new()
        }
    };

    (StatusCode::OK, Json(json!({ "data": data }))).into_response()
}

async fn fetch_pool_snapshot(
    prometheus: &PrometheusClient,
) -> Result<(String, Vec<serde_json::Value>, Vec<serde_json::Value>), String> {
    let listen_address = fetch_pool_listen_address(prometheus).await.unwrap_or_default();
    let services = fetch_services(prometheus).await?;
    let downstreams = fetch_downstreams(prometheus).await?;

    let proxies = downstreams
        .into_iter()
        .map(|proxy| {
            json!({
                "id": proxy.id,
                "address": proxy.address,
                "channels": Vec::<u32>::new(),
                "shares_submitted": proxy.shares_submitted,
                "quotes_created": proxy.quotes_created,
                "ehash_mined": proxy.ehash_mined,
                "last_share_at": proxy.last_share_at,
                "work_selection": proxy.work_selection,
            })
        })
        .collect::<Vec<_>>();

    Ok((listen_address, services, proxies))
}

async fn fetch_pool_listen_address(prometheus: &PrometheusClient) -> Option<String> {
    let results = prometheus.query_instant("hashpool_pool_info").await.ok()?;
    results
        .first()
        .and_then(|sample| sample.metric.get("listen_address"))
        .cloned()
}

async fn fetch_services(
    prometheus: &PrometheusClient,
) -> Result<Vec<serde_json::Value>, String> {
    let results = prometheus.query_instant("hashpool_pool_service_info").await?;

    let services = results
        .into_iter()
        .filter_map(|sample| {
            let service_type = sample.metric.get("service_type")?.clone();
            let address = sample.metric.get("address")?.clone();
            Some(json!({
                "service_type": service_type,
                "address": address,
            }))
        })
        .collect();

    Ok(services)
}

async fn fetch_downstreams(
    prometheus: &PrometheusClient,
) -> Result<Vec<DownstreamMetrics>, String> {
    let mut downstreams: HashMap<u32, DownstreamMetrics> = HashMap::new();

    let info_samples = prometheus
        .query_instant("hashpool_pool_downstream_info")
        .await?;
    for sample in info_samples {
        if let Some(id) = metric_id(&sample.metric) {
            let entry = downstreams.entry(id).or_insert_with(DownstreamMetrics::default);
            entry.id = id;
            entry.address = sample
                .metric
                .get("address")
                .cloned()
                .unwrap_or_default();
            entry.work_selection = sample
                .metric
                .get("work_selection")
                .map(|value| value == "true")
                .unwrap_or(false);
        }
    }

    merge_metric(
        &mut downstreams,
        "hashpool_pool_downstream_shares_total",
        |entry, value| entry.shares_submitted = value,
        prometheus,
    )
    .await?;

    merge_metric(
        &mut downstreams,
        "hashpool_pool_downstream_quotes_total",
        |entry, value| entry.quotes_created = value,
        prometheus,
    )
    .await?;

    merge_metric(
        &mut downstreams,
        "hashpool_pool_downstream_ehash_mined_total",
        |entry, value| entry.ehash_mined = value,
        prometheus,
    )
    .await?;

    let last_share_samples = prometheus
        .query_instant("hashpool_pool_downstream_last_share_at_seconds")
        .await?;
    for sample in last_share_samples {
        if let Some(id) = metric_id(&sample.metric) {
            let entry = downstreams.entry(id).or_insert_with(DownstreamMetrics::default);
            let value = parse_sample_value(&sample.value.1) as u64;
            entry.last_share_at = if value == 0 { None } else { Some(value) };
        }
    }

    let mut result: Vec<DownstreamMetrics> = downstreams.into_values().collect();
    result.sort_by_key(|d| d.id);
    Ok(result)
}

async fn merge_metric<F>(
    downstreams: &mut HashMap<u32, DownstreamMetrics>,
    metric_name: &str,
    apply: F,
    prometheus: &PrometheusClient,
) -> Result<(), String>
where
    F: Fn(&mut DownstreamMetrics, u64),
{
    let samples = prometheus.query_instant(metric_name).await?;
    for sample in samples {
        if let Some(id) = metric_id(&sample.metric) {
            let entry = downstreams.entry(id).or_insert_with(DownstreamMetrics::default);
            let value = parse_sample_value(&sample.value.1) as u64;
            apply(entry, value);
        }
    }
    Ok(())
}

fn metric_id(metric: &HashMap<String, String>) -> Option<u32> {
    metric.get("downstream_id")?.parse::<u32>().ok()
}

fn parse_sample_value(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0)
}

fn range_point_to_hashrate(point: &(f64, String)) -> serde_json::Value {
    let hashrate = parse_sample_value(&point.1);
    json!({
        "timestamp": point.0 as u64,
        "hashrate_hs": hashrate,
    })
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_id_parsing() {
        let mut labels = HashMap::new();
        labels.insert("downstream_id".to_string(), "42".to_string());
        assert_eq!(metric_id(&labels), Some(42));

        labels.insert("downstream_id".to_string(), "not-a-number".to_string());
        assert_eq!(metric_id(&labels), None);
    }

    #[test]
    fn test_parse_sample_value() {
        assert_eq!(parse_sample_value("123.45"), 123.45);
        assert_eq!(parse_sample_value("not-a-number"), 0.0);
    }

    #[test]
    fn test_range_point_to_hashrate() {
        let point = (1_700_000_000.0, "2500".to_string());
        let value = range_point_to_hashrate(&point);
        assert_eq!(value["timestamp"].as_u64(), Some(1_700_000_000));
        assert_eq!(value["hashrate_hs"].as_f64(), Some(2500.0));
    }
}
