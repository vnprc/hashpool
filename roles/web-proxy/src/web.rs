use std::convert::Infallible;
use std::sync::{Arc, OnceLock};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use tokio::net::TcpListener;
use tracing::{error, info};
use bytes::Bytes;
use serde_json::json;

use crate::SnapshotStorage;
use web_assets::icons::{nav_icon_css, pickaxe_favicon_inline_svg};

static MINERS_PAGE_HTML: OnceLock<String> = OnceLock::new();

const WALLET_PAGE_TEMPLATE: &str = include_str!("../templates/wallet.html");
const MINERS_PAGE_TEMPLATE: &str = include_str!("../templates/miners.html");
const POOL_PAGE_TEMPLATE: &str = include_str!("../templates/pool.html");

pub async fn run_http_server(
    address: String,
    storage: Arc<SnapshotStorage>,
    faucet_enabled: bool,
    faucet_url: Option<String>,
    downstream_address: String,
    downstream_port: u16,
    upstream_address: String,
    upstream_port: u16,
    client_poll_interval_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&address).await?;
    info!("🌐 Web proxy listening on http://{}", address);

    let faucet_url = Arc::new(faucet_url);
    let downstream_addr = Arc::new(downstream_address);
    let upstream_addr = Arc::new(upstream_address);
    let poll_interval_secs = Arc::new(client_poll_interval_secs);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let storage = storage.clone();
        let faucet_url = faucet_url.clone();
        let downstream_addr = downstream_addr.clone();
        let upstream_addr = upstream_addr.clone();
        let poll_interval_secs = poll_interval_secs.clone();

        tokio::task::spawn(async move {
            let service = service_fn(move |req| {
                let storage = storage.clone();
                let faucet_url = faucet_url.clone();
                let downstream_addr = downstream_addr.clone();
                let upstream_addr = upstream_addr.clone();
                let poll_interval_secs = poll_interval_secs.clone();
                async move {
                    handle_request(
                        req,
                        storage,
                        faucet_enabled,
                        faucet_url.as_deref(),
                        downstream_addr.as_str(),
                        downstream_port,
                        upstream_addr.as_str(),
                        upstream_port,
                        *poll_interval_secs,
                    )
                    .await
                }
            });

            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn handle_request(
    req: Request<Incoming>,
    storage: Arc<SnapshotStorage>,
    faucet_enabled: bool,
    faucet_url: Option<&str>,
    downstream_address: &str,
    downstream_port: u16,
    upstream_address: &str,
    upstream_port: u16,
    client_poll_interval_secs: u64,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/favicon.ico") | (&Method::GET, "/favicon.svg") => Ok(serve_favicon()),
        (&Method::GET, "/") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(wallet_page(faucet_enabled)))
        }
        (&Method::GET, "/miners") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(miners_page(downstream_address, downstream_port)))
        }
        (&Method::GET, "/pool") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(pool_page(upstream_address, upstream_port, client_poll_interval_secs)))
        }
        (&Method::GET, "/api/miners") => {
            let stats = get_miner_stats(storage).await;
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(stats.to_string())))
        }
        (&Method::GET, "/api/pool") => {
            let pool_info = get_pool_info(storage).await;
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(pool_info.to_string())))
        }
        (&Method::GET, "/balance") => {
            let balance = get_wallet_balance(storage.clone()).await;
            let json_response = json!({
                "balance": format!("{} ehash", balance),
                "balance_raw": balance,
                "unit": "HASH"
            });
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(json_response.to_string())))
        }
        (&Method::GET, "/health") => {
            let stale = storage.is_stale(15);
            let status_code = if stale {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            };
            let json_response = json!({
                "healthy": !stale,
                "stale": stale
            });
            Response::builder()
                .status(status_code)
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(json_response.to_string())))
        }
        (&Method::POST, "/mint/tokens") => {
            proxy_mint_request(faucet_enabled, faucet_url).await
        }
        _ => {
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Not Found")))
        }
    };

    Ok(response.unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Full::new(Bytes::from("Internal Server Error")))
            .unwrap()
    }))
}

fn serve_favicon() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "image/svg+xml")
        .body(Full::new(Bytes::from_static(
            pickaxe_favicon_inline_svg().as_bytes(),
        )))
        .unwrap()
}

fn wallet_page(faucet_enabled: bool) -> Bytes {
    let html = WALLET_PAGE_TEMPLATE.replace("/* {{NAV_ICON_CSS}} */", nav_icon_css());

    let html = if !faucet_enabled {
        // Remove mint button if faucet is disabled
        html.replace(r#"<button class="mint-button" id="drip-btn" onclick="requestDrip()">Mint</button>"#, "")
    } else {
        html
    };

    Bytes::from(html)
}

fn miners_page(downstream_address: &str, downstream_port: u16) -> Bytes {
    let html = MINERS_PAGE_HTML.get_or_init(|| {
        MINERS_PAGE_TEMPLATE.replace("/* {{NAV_ICON_CSS}} */", nav_icon_css())
    });

    let formatted_html = html
        .replace("{downstream_address}", downstream_address)
        .replace("{downstream_port}", &downstream_port.to_string());

    Bytes::from(formatted_html)
}

fn pool_page(upstream_address: &str, upstream_port: u16, client_poll_interval_secs: u64) -> Bytes {
    let html = POOL_PAGE_TEMPLATE.replace("/* {{NAV_ICON_CSS}} */", nav_icon_css());

    // Convert seconds to milliseconds for JavaScript setInterval
    let client_poll_interval_ms = client_poll_interval_secs * 1000;

    let formatted_html = html
        .replace("{upstream_address}", upstream_address)
        .replace("{upstream_port}", &upstream_port.to_string())
        .replace("{client_poll_interval_ms}", &client_poll_interval_ms.to_string());

    Bytes::from(formatted_html)
}

async fn get_wallet_balance(storage: Arc<SnapshotStorage>) -> u64 {
    match storage.get() {
        Some(snapshot) => snapshot.ehash_balance,
        None => 0,
    }
}

async fn get_pool_info(storage: Arc<SnapshotStorage>) -> serde_json::Value {
    match storage.get() {
        Some(snapshot) => {
            json!({
                "blockchain_network": snapshot.blockchain_network,
                "upstream_pool": snapshot.upstream_pool,
                "connected": snapshot.upstream_pool.is_some()
            })
        }
        None => {
            json!({
                "blockchain_network": "unknown",
                "upstream_pool": null,
                "connected": false
            })
        }
    }
}

async fn get_miner_stats(storage: Arc<SnapshotStorage>) -> serde_json::Value {
    let snapshot = match storage.get() {
        Some(snapshot) => snapshot,
        None => {
            return json!({
                "total_miners": 0,
                "total_hashrate": "0 H/s",
                "total_shares": 0,
                "miners": []
            })
        }
    };

    let total_miners = snapshot.downstream_miners.len();
    let total_shares: u64 = snapshot
        .downstream_miners
        .iter()
        .map(|m| m.shares_submitted)
        .sum();
    let total_hashrate_raw: f64 = snapshot.downstream_miners.iter().map(|m| m.hashrate).sum();

    let total_hashrate = format_hashrate(total_hashrate_raw);

    let miners: Vec<serde_json::Value> = snapshot
        .downstream_miners
        .iter()
        .map(|m| {
            let connected_time = {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let elapsed = now.saturating_sub(m.connected_at);
                if elapsed < 60 {
                    format!("{}s", elapsed)
                } else if elapsed < 3600 {
                    format!("{}m", elapsed / 60)
                } else if elapsed < 86400 {
                    format!("{}h", elapsed / 3600)
                } else {
                    format!("{}d", elapsed / 86400)
                }
            };

            json!({
                "name": m.name,
                "id": m.id,
                "address": m.address,
                "hashrate": format_hashrate(m.hashrate),
                "shares": m.shares_submitted,
                "connected_time": connected_time
            })
        })
        .collect();

    json!({
        "total_miners": total_miners,
        "total_hashrate": total_hashrate,
        "total_shares": total_shares,
        "miners": miners
    })
}

fn format_hashrate(hashrate: f64) -> String {
    if hashrate >= 1_000_000_000_000.0 {
        format!("{:.1} TH/s", hashrate / 1_000_000_000_000.0)
    } else if hashrate >= 1_000_000_000.0 {
        format!("{:.1} GH/s", hashrate / 1_000_000_000.0)
    } else if hashrate >= 1_000_000.0 {
        format!("{:.1} MH/s", hashrate / 1_000_000.0)
    } else if hashrate >= 1_000.0 {
        format!("{:.1} KH/s", hashrate / 1_000.0)
    } else {
        format!("{:.1} H/s", hashrate)
    }
}

async fn proxy_mint_request(
    faucet_enabled: bool,
    faucet_url: Option<&str>,
) -> Result<Response<Full<Bytes>>, hyper::http::Error> {
    if !faucet_enabled {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(r#"{"error":"Faucet is disabled"}"#)));
    }

    let Some(faucet_url) = faucet_url else {
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(r#"{"error":"Faucet URL not configured"}"#)));
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
                Ok(body) => Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(Full::new(Bytes::from(body))),
                Err(e) => {
                    error!("Failed to read response from translator: {}", e);
                    let json_response = json!({
                        "success": false,
                        "error": "Failed to read mint response"
                    });
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .header("content-type", "application/json")
                        .body(Full::new(Bytes::from(json_response.to_string())))
                }
            }
        }
        Err(e) => {
            error!("Failed to proxy mint request to translator: {}", e);
            let json_response = json!({
                "success": false,
                "error": format!("Faucet unavailable: {}", e)
            });
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(json_response.to_string())))
        }
    }
}
