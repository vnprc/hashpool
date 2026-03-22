use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{body::Bytes, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{info, error, warn};
use serde_json::json;

use cdk::nuts::SecretKey;
use cdk::wallet::{SendOptions, Wallet};
use cdk::Amount;

#[derive(Debug)]
struct RateLimiter {
    last_request: Mutex<Option<Instant>>,
    timeout: Duration,
}

impl RateLimiter {
    fn new(timeout_secs: u64) -> Self {
        Self {
            last_request: Mutex::new(None),
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    async fn check_rate_limit(&self) -> Result<(), Duration> {
        let mut last_request = self.last_request.lock().await;
        let now = Instant::now();

        if let Some(last) = *last_request {
            let elapsed = now.duration_since(last);
            if elapsed < self.timeout {
                let remaining = self.timeout - elapsed;
                return Err(remaining);
            }
        }

        *last_request = Some(now);
        Ok(())
    }
}

async fn create_mint_token(
    wallet: Arc<Wallet>,
    locking_privkey: Option<SecretKey>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let amount = Amount::from(32u64);
    info!("Creating mint token for {} ehash", amount);

    let token = wallet
        .prepare_send(
            amount,
            SendOptions {
                p2pk_signing_keys: locking_privkey.into_iter().collect(),
                ..Default::default()
            },
        )
        .await?
        .confirm(None)
        .await?;

    info!("Mint token created successfully");
    Ok(token.to_string())
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    wallet: Arc<Wallet>,
    rate_limiter: Arc<RateLimiter>,
    locking_privkey: Option<SecretKey>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::POST, "/mint/tokens") => {
            match rate_limiter.check_rate_limit().await {
                Ok(()) => {
                    info!("Mint request accepted");
                    match create_mint_token(wallet, locking_privkey).await {
                        Ok(token) => {
                            let json_response = json!({
                                "success": true,
                                "token": token,
                                "amount": 32
                            });
                            Response::builder()
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(json_response.to_string())))
                        }
                        Err(e) => {
                            error!("Failed to create mint token: {}", e);
                            let json_response = json!({
                                "success": false,
                                "error": format!("Minting failed: {}", e)
                            });
                            Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(json_response.to_string())))
                        }
                    }
                }
                Err(remaining) => {
                    warn!(
                        "Mint request rate limited - {} seconds remaining",
                        remaining.as_secs()
                    );
                    let json_response = json!({
                        "success": false,
                        "error": format!("Rate limited. Try again in {} seconds", remaining.as_secs())
                    });
                    Response::builder()
                        .status(StatusCode::TOO_MANY_REQUESTS)
                        .header("content-type", "application/json")
                        .body(Full::new(Bytes::from(json_response.to_string())))
                }
            }
        }
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("Not Found"))),
    };

    Ok(response.unwrap())
}

pub async fn run_faucet_api(
    port: u16,
    wallet: Arc<Wallet>,
    timeout_secs: u64,
    locking_privkey: Option<String>,
) {
    let addr = format!("127.0.0.1:{}", port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind faucet API to {}: {}", addr, e);
            return;
        }
    };

    if locking_privkey.is_none() {
        warn!("Faucet started without locking_privkey; P2PK-locked proof swaps will fail");
    }

    let parsed_key: Option<SecretKey> = locking_privkey.as_deref().and_then(|hex_str| {
        hex::decode(hex_str)
            .ok()
            .and_then(|bytes| SecretKey::from_slice(&bytes).ok())
    });

    info!(
        "Faucet API listening on http://{} (timeout: {}s)",
        addr, timeout_secs
    );

    let rate_limiter = Arc::new(RateLimiter::new(timeout_secs));

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("Failed to accept connection: {}", e);
                continue;
            }
        };

        let io = TokioIo::new(stream);
        let wallet_clone = wallet.clone();
        let rate_limiter_clone = rate_limiter.clone();
        let key_clone = parsed_key.clone();

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(
                    io,
                    service_fn(move |req| {
                        handle_request(
                            req,
                            wallet_clone.clone(),
                            rate_limiter_clone.clone(),
                            key_clone.clone(),
                        )
                    }),
                )
                .await
            {
                error!("Error serving faucet connection: {:?}", err);
            }
        });
    }
}
