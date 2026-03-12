use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use stats_sv2::metrics::derive_hashrate;
use std::sync::Arc;
use tracing::info;

use crate::TranslatorSv2;

pub async fn run_monitoring_server(
    address: String,
    translator: TranslatorSv2,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(Arc::new(translator));

    let listener = tokio::net::TcpListener::bind(&address).await?;
    info!("📈 Translator monitoring listening on http://{}", address);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn metrics_handler(State(translator): State<Arc<TranslatorSv2>>) -> impl IntoResponse {
    let metrics = build_metrics(&translator).await;

    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        metrics,
    )
}

async fn build_metrics(translator: &TranslatorSv2) -> String {
    let mut output = String::with_capacity(2048);

    let upstream_address = translator
        .config
        .upstreams
        .first()
        .map(|upstream| format!("{}:{}", upstream.address, upstream.port))
        .unwrap_or_default();

    let blockchain_network = std::env::var("BITCOIND_NETWORK")
        .unwrap_or_else(|_| "unknown".to_string())
        .to_lowercase();

    output.push_str("# TYPE hashpool_translator_info gauge\n");
    output.push_str(&format!(
        "hashpool_translator_info{{{}}} 1\n",
        format_labels(&[
            ("blockchain_network", blockchain_network.as_str()),
            ("upstream_address", upstream_address.as_str()),
        ])
    ));

    output.push_str("# TYPE hashpool_translator_wallet_balance_ehash gauge\n");
    let balance = if let Some(wallet) = translator.wallet.as_ref() {
        wallet
            .total_balance()
            .await
            .map(u64::from)
            .unwrap_or(0)
    } else {
        0
    };
    output.push_str(&format!(
        "hashpool_translator_wallet_balance_ehash {}\n",
        balance
    ));

    output.push_str("# TYPE hashpool_translator_miner_info gauge\n");
    output.push_str("# TYPE hashpool_translator_miner_shares_total counter\n");
    output.push_str("# TYPE hashpool_translator_miner_hashrate_hs gauge\n");
    output.push_str("# TYPE hashpool_translator_miner_connected_at_seconds gauge\n");

    let miners = translator.miner_tracker.get_all_miners().await;
    let now = unix_timestamp();

    for miner in miners {
        let connected_timestamp = now.saturating_sub(miner.connected_time.elapsed().as_secs());
        let address = if translator.config.redact_ip {
            "REDACTED".to_string()
        } else {
            miner.address.to_string()
        };
        let sum_difficulty = miner.metrics_collector.sum_difficulty_in_window();
        let window_seconds = miner.metrics_collector.window_seconds();
        let hashrate = derive_hashrate(sum_difficulty, window_seconds);
        let id_str = miner.id.to_string();

        output.push_str(&format!(
            "hashpool_translator_miner_info{{{}}} 1\n",
            format_labels(&[
                ("miner_id", id_str.as_str()),
                ("name", miner.name.as_str()),
                ("address", address.as_str()),
            ])
        ));
        output.push_str(&format!(
            "hashpool_translator_miner_shares_total{{{}}} {}\n",
            format_labels(&[("miner_id", id_str.as_str())]),
            miner.shares_submitted
        ));
        output.push_str(&format!(
            "hashpool_translator_miner_hashrate_hs{{{}}} {}\n",
            format_labels(&[("miner_id", id_str.as_str())]),
            hashrate
        ));
        output.push_str(&format!(
            "hashpool_translator_miner_connected_at_seconds{{{}}} {}\n",
            format_labels(&[("miner_id", id_str.as_str())]),
            connected_timestamp
        ));
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

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
