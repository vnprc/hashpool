use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tracing::{error, info, warn};

use crate::prometheus::PrometheusClient;
use web_assets::icons::{nav_icon_css, pickaxe_favicon_inline_svg};
use web_utils::{format_elapsed_time, format_hashrate};

static MINERS_PAGE_HTML: OnceLock<String> = OnceLock::new();

const WALLET_PAGE_TEMPLATE: &str = include_str!("../templates/wallet.html");
const MINERS_PAGE_TEMPLATE: &str = include_str!("../templates/miners.html");
const POOL_PAGE_TEMPLATE: &str = include_str!("../templates/pool.html");

pub struct AppState {
    pub prometheus: PrometheusClient,
    pub faucet_enabled: bool,
    pub faucet_url: Option<String>,
    pub downstream_address: String,
    pub downstream_port: u16,
    pub upstream_address: String,
    pub upstream_port: u16,
    pub client_poll_interval_secs: u64,
    pub metrics_query_step_secs: u64,
}

#[derive(Default)]
struct MinerMetrics {
    id: u32,
    name: String,
    address: String,
    hashrate_hs: f64,
    shares: u64,
    connected_at: u64,
}

pub async fn run_http_server(
    address: String,
    prometheus: PrometheusClient,
    faucet_enabled: bool,
    faucet_url: Option<String>,
    downstream_address: String,
    downstream_port: u16,
    upstream_address: String,
    upstream_port: u16,
    client_poll_interval_secs: u64,
    metrics_query_step_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState {
        prometheus,
        faucet_enabled,
        faucet_url,
        downstream_address,
        downstream_port,
        upstream_address,
        upstream_port,
        client_poll_interval_secs,
        metrics_query_step_secs: metrics_query_step_secs.max(1),
    };

    let app = Router::new()
        .route("/favicon.ico", get(serve_favicon))
        .route("/favicon.svg", get(serve_favicon))
        .route("/", get(wallet_page_handler))
        .route("/miners", get(miners_page_handler))
        .route("/pool", get(pool_page_handler))
        .route("/api/miners", get(api_miners_handler))
        .route("/api/pool", get(api_pool_handler))
        .route("/balance", get(balance_handler))
        .route("/health", get(health_handler))
        .route("/mint/tokens", post(mint_tokens_handler))
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind(&address).await?;
    info!("🌐 Web proxy listening on http://{}", address);

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

async fn wallet_page_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let html = WALLET_PAGE_TEMPLATE.replace("/* {{NAV_ICON_CSS}} */", nav_icon_css());

    let html = if !state.faucet_enabled {
        // Remove mint button if faucet is disabled
        html.replace(
            r#"<button class=\"mint-button\" id=\"drip-btn\" onclick=\"requestDrip()\">Mint</button>"#,
            "",
        )
    } else {
        html
    };

    Html(html)
}

async fn miners_page_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let html = MINERS_PAGE_HTML.get_or_init(|| {
        MINERS_PAGE_TEMPLATE.replace("/* {{NAV_ICON_CSS}} */", nav_icon_css())
    });

    let formatted_html = html
        .replace("{downstream_address}", &state.downstream_address)
        .replace("{downstream_port}", &state.downstream_port.to_string());

    Html(formatted_html)
}

async fn pool_page_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let html = POOL_PAGE_TEMPLATE.replace("/* {{NAV_ICON_CSS}} */", nav_icon_css());

    // Convert seconds to milliseconds for JavaScript setInterval
    let client_poll_interval_ms = state.client_poll_interval_secs * 1000;

    let formatted_html = html
        .replace("{upstream_address}", &state.upstream_address)
        .replace("{upstream_port}", &state.upstream_port.to_string())
        .replace(
            "{client_poll_interval_ms}",
            &client_poll_interval_ms.to_string(),
        );

    Html(formatted_html)
}

async fn api_miners_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stats = match get_miner_stats(&state.prometheus).await {
        Ok(stats) => stats,
        Err(err) => {
            warn!("Failed to fetch miner stats: {}", err);
            json!({
                "total_miners": 0,
                "total_hashrate": "0 H/s",
                "total_shares": 0,
                "miners": []
            })
        }
    };

    Json(stats)
}

async fn api_pool_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pool_info = match get_pool_info(&state.prometheus).await {
        Ok(info) => info,
        Err(err) => {
            warn!("Failed to fetch pool info: {}", err);
            json!({
                "blockchain_network": "unknown",
                "upstream_pool": null,
                "connected": false
            })
        }
    };

    Json(pool_info)
}

async fn balance_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let balance = match get_wallet_balance(&state.prometheus).await {
        Ok(balance) => balance,
        Err(err) => {
            warn!("Failed to fetch wallet balance: {}", err);
            0
        }
    };

    let json_response = json!({
        "balance": format!("{} ehash", balance),
        "balance_raw": balance,
        "unit": "HASH"
    });
    Json(json_response)
}

async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let healthy = match state
        .prometheus
        .query_instant("hashpool_translator_info")
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
        "stale": !healthy
    });
    (status_code, Json(json_response))
}

async fn mint_tokens_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.faucet_enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Faucet is disabled"})),
        );
    }

    let Some(faucet_url) = &state.faucet_url else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Faucet URL not configured"})),
        );
    };

    // Proxy mint request to translator's faucet API
    let translator_faucet_url = format!("{}/mint/tokens", faucet_url);

    match reqwest::Client::new()
        .post(&translator_faucet_url)
        .header("content-length", "0")
        .body("")
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            match response.text().await {
                Ok(body) => {
                    let status_code = StatusCode::from_u16(status.as_u16())
                        .unwrap_or_else(|_| {
                            error!("Invalid status code from translator: {}", status);
                            StatusCode::INTERNAL_SERVER_ERROR
                        });
                    // Parse body as JSON if possible, otherwise wrap as raw text
                    let json_body = serde_json::from_str::<serde_json::Value>(&body)
                        .unwrap_or_else(|_| json!({"response": body}));
                    (status_code, Json(json_body))
                }
                Err(e) => {
                    error!("Failed to read response from translator: {}", e);
                    let json_response = json!({
                        "success": false,
                        "error": "Failed to read mint response"
                    });
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json_response))
                }
            }
        }
        Err(e) => {
            error!("Failed to proxy mint request to translator: {}", e);
            let json_response = json!({
                "success": false,
                "error": format!("Faucet unavailable: {}", e)
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(json_response))
        }
    }
}

async fn get_wallet_balance(prometheus: &PrometheusClient) -> Result<u64, String> {
    let samples = prometheus
        .query_instant("hashpool_translator_wallet_balance_ehash")
        .await?;
    let balance = samples
        .first()
        .map(|sample| parse_sample_value(&sample.value.1) as u64)
        .unwrap_or(0);
    Ok(balance)
}

async fn get_pool_info(prometheus: &PrometheusClient) -> Result<serde_json::Value, String> {
    let samples = prometheus.query_instant("hashpool_translator_info").await?;

    if let Some(sample) = samples.first() {
        let blockchain_network = sample
            .metric
            .get("blockchain_network")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let upstream_address = sample
            .metric
            .get("upstream_address")
            .cloned()
            .filter(|value| !value.is_empty());

        Ok(json!({
            "blockchain_network": blockchain_network,
            "upstream_pool": upstream_address.as_ref().map(|address| json!({"address": address})),
            "connected": upstream_address.is_some()
        }))
    } else {
        Ok(json!({
            "blockchain_network": "unknown",
            "upstream_pool": null,
            "connected": false
        }))
    }
}

async fn get_miner_stats(prometheus: &PrometheusClient) -> Result<serde_json::Value, String> {
    let miners = fetch_miners(prometheus).await?;
    let total_miners = miners.len();
    let total_shares: u64 = miners.iter().map(|m| m.shares).sum();
    let total_hashrate_raw: f64 = miners.iter().map(|m| m.hashrate_hs).sum();
    let total_hashrate = format_hashrate(total_hashrate_raw);

    let now = unix_timestamp();

    let mut miners = miners;
    miners.sort_by_key(|miner| miner.id);
    let miners_json = miners
        .into_iter()
        .map(|miner| {
            let connected_time = if miner.connected_at == 0 {
                "Just now".to_string()
            } else {
                format_elapsed_time(now, miner.connected_at)
            };

            json!({
                "name": miner.name,
                "id": miner.id,
                "address": miner.address,
                "hashrate": format_hashrate(miner.hashrate_hs),
                "shares": miner.shares,
                "connected_time": connected_time
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "total_miners": total_miners,
        "total_hashrate": total_hashrate,
        "total_shares": total_shares,
        "miners": miners_json
    }))
}

async fn fetch_miners(prometheus: &PrometheusClient) -> Result<Vec<MinerMetrics>, String> {
    let mut miners: HashMap<u32, MinerMetrics> = HashMap::new();

    let info_samples = prometheus
        .query_instant("present_over_time(hashpool_translator_miner_info[30s])")
        .await?;
    for sample in info_samples {
        if let Some(id) = metric_id(&sample.metric) {
            let entry = miners.entry(id).or_insert_with(MinerMetrics::default);
            entry.id = id;
            entry.name = sample
                .metric
                .get("name")
                .cloned()
                .unwrap_or_else(|| format!("miner_{}", id));
            entry.address = sample
                .metric
                .get("address")
                .cloned()
                .unwrap_or_else(|| "REDACTED".to_string());
        }
    }

    merge_miner_metric(
        &mut miners,
        "hashpool_translator_miner_hashrate_hs",
        |entry, value| entry.hashrate_hs = value,
        prometheus,
    )
    .await?;

    let shares_samples = prometheus
        .query_instant("hashpool_translator_miner_shares_total")
        .await?;
    for sample in shares_samples {
        if let Some(id) = metric_id(&sample.metric) {
            if let Some(entry) = miners.get_mut(&id) {
                entry.shares = parse_sample_value(&sample.value.1) as u64;
            }
        }
    }

    let connected_samples = prometheus
        .query_instant("hashpool_translator_miner_connected_at_seconds")
        .await?;
    for sample in connected_samples {
        if let Some(id) = metric_id(&sample.metric) {
            if let Some(entry) = miners.get_mut(&id) {
                entry.connected_at = parse_sample_value(&sample.value.1) as u64;
            }
        }
    }

    Ok(miners.into_values().collect())
}

async fn merge_miner_metric<F>(
    miners: &mut HashMap<u32, MinerMetrics>,
    metric_name: &str,
    apply: F,
    prometheus: &PrometheusClient,
) -> Result<(), String>
where
    F: Fn(&mut MinerMetrics, f64),
{
    let samples = prometheus.query_instant(metric_name).await?;
    for sample in samples {
        if let Some(id) = metric_id(&sample.metric) {
            if let Some(entry) = miners.get_mut(&id) {
                let value = parse_sample_value(&sample.value.1);
                apply(entry, value);
            }
        }
    }
    Ok(())
}

fn metric_id(metric: &HashMap<String, String>) -> Option<u32> {
    metric.get("miner_id")?.parse::<u32>().ok()
}

fn parse_sample_value(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0)
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
        labels.insert("miner_id".to_string(), "7".to_string());
        assert_eq!(metric_id(&labels), Some(7));

        labels.insert("miner_id".to_string(), "nope".to_string());
        assert_eq!(metric_id(&labels), None);
    }

    #[test]
    fn test_parse_sample_value() {
        assert_eq!(parse_sample_value("98.76"), 98.76);
        assert_eq!(parse_sample_value("invalid"), 0.0);
    }
}
