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

use stats_proxy::db::StatsData;

pub async fn run_http_server(
    address: String,
    db: Arc<StatsData>,
    redact_ip: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&address).await?;
    info!("🌐 HTTP API listening on http://{}", address);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let db = db.clone();

        tokio::task::spawn(async move {
            let service = service_fn(move |req| {
                let db = db.clone();
                async move { handle_request(req, db, redact_ip).await }
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
    db: Arc<StatsData>,
    redact_ip: bool,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/api/stats") => {
            let snapshot = get_snapshot(db.clone()).await;
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(snapshot)))
        }
        (&Method::GET, "/api/miners") => {
            let stats = get_miner_stats(db, redact_ip).await;
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(stats.to_string())))
        }
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("Not Found"))),
    };

    Ok(response.unwrap_or_else(|e| {
        error!("Error building response: {:?}", e);
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Full::new(Bytes::from("Internal Server Error")))
            .unwrap()
    }))
}

async fn get_snapshot(db: Arc<StatsData>) -> String {
    match db.get_latest_snapshot() {
        Some(snapshot) => serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string()),
        None => r#"{"error":"no data available"}"#.to_string(),
    }
}

async fn get_miner_stats(db: Arc<StatsData>, redact_ip: bool) -> serde_json::Value {
    match db.get_latest_snapshot() {
        Some(snapshot) => {
            let miners: Vec<_> = snapshot.downstream_miners.into_iter().map(|mut m| {
                if redact_ip {
                    m.address = m.address.split(':').next().unwrap_or("").to_string() + ":****";
                }
                m
            }).collect();
            json!({ "miners": miners })
        }
        None => json!({ "miners": [] }),
    }
}
