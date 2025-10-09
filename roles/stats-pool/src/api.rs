use std::convert::Infallible;
use std::sync::Arc;
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

use stats_pool::db::StatsData;

pub async fn run_http_server(
    address: String,
    stats: Arc<StatsData>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&address).await?;
    info!("🌐 HTTP dashboard listening on http://{}", address);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let stats = stats.clone();

        tokio::task::spawn(async move {
            let service = service_fn(move |req| {
                let stats = stats.clone();
                async move { handle_request(req, stats).await }
            });

            if let Err(err) = http1::Builder::new()
                .keep_alive(true)
                .serve_connection(io, service)
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn handle_request(
    req: Request<Incoming>,
    stats: Arc<StatsData>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/api/stats") => serve_stats_json(stats.clone()).await,
        (&Method::GET, "/api/services") => serve_services_json(stats.clone()).await,
        (&Method::GET, "/api/connections") => serve_connections_json(stats.clone()).await,
        (&Method::GET, "/health") => serve_health(stats).await,
        _ => {
            let mut response = Response::new(Full::new(Bytes::from("Not Found")));
            *response.status_mut() = StatusCode::NOT_FOUND;
            response
        }
    };

    Ok(response)
}

async fn serve_stats_json(stats: Arc<StatsData>) -> Response<Full<Bytes>> {
    match stats.get_latest_snapshot() {
        Some(snapshot) => {
            let json = serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string());
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(json)))
                .unwrap()
        }
        None => {
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(r#"{"error":"no data available"}"#)))
                .unwrap()
        }
    }
}

async fn serve_services_json(stats: Arc<StatsData>) -> Response<Full<Bytes>> {
    match stats.get_latest_snapshot() {
        Some(snapshot) => {
            let json = serde_json::to_string(&snapshot.services).unwrap_or_else(|_| "[]".to_string());
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(json)))
                .unwrap()
        }
        None => {
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from("[]")))
                .unwrap()
        }
    }
}

async fn serve_connections_json(stats: Arc<StatsData>) -> Response<Full<Bytes>> {
    match stats.get_latest_snapshot() {
        Some(snapshot) => {
            let json = serde_json::to_string(&snapshot.downstream_proxies).unwrap_or_else(|_| "[]".to_string());
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(json)))
                .unwrap()
        }
        None => {
            Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from("[]")))
                .unwrap()
        }
    }
}

async fn serve_health(stats: Arc<StatsData>) -> Response<Full<Bytes>> {
    let stale = stats.is_stale(15);
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
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(json_response.to_string())))
        .unwrap()
}
