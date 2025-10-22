use std::convert::Infallible;
use std::sync::OnceLock;
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
use std::sync::Arc;

static DASHBOARD_PAGE_HTML: OnceLock<String> = OnceLock::new();
static CLIENT_POLL_INTERVAL_SECS: OnceLock<u64> = OnceLock::new();

const DASHBOARD_PAGE_TEMPLATE: &str = include_str!("../templates/dashboard.html");

pub async fn run_http_server(
    address: String,
    storage: Arc<SnapshotStorage>,
    client_poll_interval_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&address).await?;
    info!("🌐 Web pool listening on http://{}", address);
    info!("Client polling interval: {} seconds", client_poll_interval_secs);

    // Store the polling interval for use in dashboard_page
    let _ = CLIENT_POLL_INTERVAL_SECS.set(client_poll_interval_secs);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let storage = storage.clone();

        tokio::task::spawn(async move {
            let service = service_fn(move |req| {
                let storage = storage.clone();
                async move { handle_request(req, storage).await }
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
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/favicon.ico") | (&Method::GET, "/favicon.svg") => Ok(serve_favicon()),
        (&Method::GET, "/") => Response::builder()
            .header("content-type", "text/html; charset=utf-8")
            .body(Full::new(dashboard_page())),
        (&Method::GET, "/api/stats") => {
            let stats = get_pool_stats(storage).await;
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(stats.to_string())))
        }
        (&Method::GET, "/api/services") => {
            let services = get_services(storage).await;
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(services.to_string())))
        }
        (&Method::GET, "/api/connections") => {
            let connections = get_connections(storage).await;
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(connections.to_string())))
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
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("Not Found"))),
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

fn dashboard_page() -> Bytes {
    let interval_ms = CLIENT_POLL_INTERVAL_SECS.get().copied().unwrap_or(3) * 1000;
    let html = DASHBOARD_PAGE_HTML
        .get_or_init(|| {
            DASHBOARD_PAGE_TEMPLATE
                .replace("/* {{NAV_ICON_CSS}} */", nav_icon_css())
                .replace("{client_poll_interval_ms}", &interval_ms.to_string())
        });
    Bytes::from(html.clone())
}

async fn get_pool_stats(storage: Arc<SnapshotStorage>) -> serde_json::Value {
    match storage.get() {
        Some(snapshot) => {
            json!({
                "listen_address": snapshot.listen_address,
                "services": snapshot.services,
                "downstream_proxies": snapshot.downstream_proxies,
                "timestamp": snapshot.timestamp
            })
        }
        None => {
            json!({
                "listen_address": "",
                "services": [],
                "downstream_proxies": [],
                "timestamp": 0
            })
        }
    }
}

async fn get_services(storage: Arc<SnapshotStorage>) -> serde_json::Value {
    match storage.get() {
        Some(snapshot) => {
            let services: Vec<serde_json::Value> = snapshot
                .services
                .iter()
                .map(|s| {
                    json!({
                        "service_type": format!("{:?}", s.service_type),
                        "address": s.address
                    })
                })
                .collect();
            json!({ "services": services })
        }
        None => json!({ "services": [] }),
    }
}

async fn get_connections(storage: Arc<SnapshotStorage>) -> serde_json::Value {
    match storage.get() {
        Some(snapshot) => {
            let proxies: Vec<serde_json::Value> = snapshot
                .downstream_proxies
                .iter()
                .map(|p| {
                    let last_share = p.last_share_at.map(|ts| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs();
                        let elapsed = now.saturating_sub(ts);
                        if elapsed < 60 {
                            format!("{}s ago", elapsed)
                        } else if elapsed < 3600 {
                            format!("{}m ago", elapsed / 60)
                        } else if elapsed < 86400 {
                            format!("{}h ago", elapsed / 3600)
                        } else {
                            format!("{}d ago", elapsed / 86400)
                        }
                    });

                    json!({
                        "id": p.id,
                        "address": p.address,
                        "channels": p.channels,
                        "shares_submitted": p.shares_submitted,
                        "quotes_created": p.quotes_created,
                        "ehash_mined": p.ehash_mined,
                        "last_share_at": last_share,
                        "work_selection": p.work_selection
                    })
                })
                .collect();
            json!({ "proxies": proxies })
        }
        None => json!({ "proxies": [] }),
    }
}
