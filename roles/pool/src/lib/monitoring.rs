use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use stats_sv2::metrics::derive_hashrate;
use std::sync::Arc;
use stratum_common::roles_logic_sv2::utils::Mutex;
use tracing::info;

use crate::mining_pool::Pool;

#[derive(Clone)]
pub struct MonitoringState {
    pool: Arc<Mutex<Pool>>,
    listen_address: String,
}

pub async fn run_monitoring_server(
    address: String,
    pool: Arc<Mutex<Pool>>,
    listen_address: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = MonitoringState {
        pool,
        listen_address,
    };

    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind(&address).await?;
    info!("📈 Pool monitoring listening on http://{}", address);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn metrics_handler(State(state): State<Arc<MonitoringState>>) -> impl IntoResponse {
    match state.pool.safe_lock(|pool| build_metrics(pool, &state.listen_address)) {
        Ok(metrics) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4")],
            metrics,
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/plain; version=0.0.4")],
            "hashpool_pool_scrape_error 1\n".to_string(),
        ),
    }
}

fn build_metrics(pool: &Pool, listen_address: &str) -> String {
    let mut output = String::with_capacity(1024);

    output.push_str("# TYPE hashpool_pool_info gauge\n");
    output.push_str(&format!(
        "hashpool_pool_info{{{}}} 1\n",
        format_labels(&[("listen_address", listen_address)])
    ));

    output.push_str("# TYPE hashpool_pool_service_info gauge\n");
    output.push_str(&format!(
        "hashpool_pool_service_info{{{}}} 1\n",
        format_labels(&[("service_type", "Pool"), ("address", listen_address)])
    ));

    if pool.mint_connection.is_some() {
        let mint_address = pool.mint_manager.mint_address();
        output.push_str(&format!(
            "hashpool_pool_service_info{{{}}} 1\n",
            format_labels(&[("service_type", "Mint"), ("address", mint_address)])
        ));
    }

    if let Some(jds_address) = pool.jd_server_address.as_deref() {
        output.push_str(&format!(
            "hashpool_pool_service_info{{{}}} 1\n",
            format_labels(&[("service_type", "JobDeclarator"), ("address", jds_address)])
        ));
    }

    output.push_str("# TYPE hashpool_pool_downstream_info gauge\n");
    output.push_str("# TYPE hashpool_pool_downstream_shares_total counter\n");
    output.push_str("# TYPE hashpool_pool_downstream_quotes_total counter\n");
    output.push_str("# TYPE hashpool_pool_downstream_ehash_mined_total counter\n");
    output.push_str("# TYPE hashpool_pool_downstream_last_share_at_seconds gauge\n");
    output.push_str("# TYPE hashpool_pool_downstream_hashrate_hs gauge\n");

    for (id, downstream) in &pool.downstreams {
        if let Ok((address, work_selection)) = downstream.safe_lock(|d| {
            (d.address.to_string(), d.requires_custom_work)
        }) {
            let id_str = id.to_string();
            output.push_str(&format!(
                "hashpool_pool_downstream_info{{{}}} 1\n",
                format_labels(&[
                    ("downstream_id", id_str.as_str()),
                    ("address", address.as_str()),
                    (
                        "work_selection",
                        if work_selection { "true" } else { "false" },
                    ),
                ])
            ));

            if let Some(stats) = pool.stats_registry.get_stats(*id) {
                let shares = stats.shares_submitted.load(std::sync::atomic::Ordering::Relaxed);
                let quotes = stats.quotes_created.load(std::sync::atomic::Ordering::Relaxed);
                let ehash = stats.ehash_mined.load(std::sync::atomic::Ordering::Relaxed);
                let last_share = stats.last_share_at.load(std::sync::atomic::Ordering::Relaxed);

                let sum_difficulty = stats.sum_difficulty_in_window();
                let window_seconds = stats.window_seconds();
                let hashrate = derive_hashrate(sum_difficulty, window_seconds);

                output.push_str(&format!(
                    "hashpool_pool_downstream_shares_total{{{}}} {}\n",
                    format_labels(&[("downstream_id", id_str.as_str())]),
                    shares
                ));
                output.push_str(&format!(
                    "hashpool_pool_downstream_quotes_total{{{}}} {}\n",
                    format_labels(&[("downstream_id", id_str.as_str())]),
                    quotes
                ));
                output.push_str(&format!(
                    "hashpool_pool_downstream_ehash_mined_total{{{}}} {}\n",
                    format_labels(&[("downstream_id", id_str.as_str())]),
                    ehash
                ));
                output.push_str(&format!(
                    "hashpool_pool_downstream_last_share_at_seconds{{{}}} {}\n",
                    format_labels(&[("downstream_id", id_str.as_str())]),
                    last_share
                ));
                output.push_str(&format!(
                    "hashpool_pool_downstream_hashrate_hs{{{}}} {}\n",
                    format_labels(&[("downstream_id", id_str.as_str())]),
                    hashrate
                ));
            }
        }
    }

    output
}

fn format_labels(labels: &[(&str, &str)]) -> String {
    labels
        .iter()
        .map(|(key, value)| format!("{}=\"{}\"", key, escape_label_value(value)))
        .collect::<Vec<_>>()
        .join(",")
}

fn escape_label_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
