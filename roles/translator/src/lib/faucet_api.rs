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

use cdk::wallet::Wallet;
use cdk::Amount;

// Rate limiting: 30 second global cooldown
const RATE_LIMIT_DURATION: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct RateLimiter {
    last_request: Mutex<Option<Instant>>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            last_request: Mutex::new(None),
        }
    }

    async fn check_rate_limit(&self) -> Result<(), Duration> {
        let mut last_request = self.last_request.lock().await;
        let now = Instant::now();

        if let Some(last) = *last_request {
            let elapsed = now.duration_since(last);
            if elapsed < RATE_LIMIT_DURATION {
                let remaining = RATE_LIMIT_DURATION - elapsed;
                return Err(remaining);
            }
        }

        *last_request = Some(now);
        Ok(())
    }
}

async fn create_mint_token(wallet: Arc<Wallet>) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Create a 32 diff token (32 sat amount)
    let amount = Amount::from(32u64);

    info!("🪙 Creating mint token for {} ehash", amount);

    // Check wallet balance first
    let balance = wallet.total_balance().await?;
    if balance < amount {
        error!("❌ Insufficient balance in wallet: {} diff available, need {} ehash", balance, amount);
        return Err("Insufficient balance in wallet".into());
    }

    // First, swap to get exactly one proof of 32 sats
    // This ensures we have the exact denomination we need
    let single_proof = match wallet.swap_from_unspent(amount, None, false).await {
        Ok(proofs) => {
            let total_amount: Amount = proofs.iter().fold(Amount::ZERO, |acc, p| acc + p.amount);
            info!("💱 Swapped for {} proofs totaling {} ehash", proofs.len(), total_amount);
            proofs
        }
        Err(e) => {
            error!("❌ Failed to swap for exact amount: {:?}", e);
            return Err(format!("Failed to prepare token: {}", e).into());
        }
    };

    // Now create the token from our exact proofs
    let token = cdk::nuts::nut00::token::Token::new(
        wallet.mint_url.clone(),
        single_proof.clone(),
        None, // No memo
        wallet.unit.clone()
    );

    let token_string = token.to_string();
    info!("✅ Mint token created successfully with {} proofs", single_proof.len());
    Ok(token_string)
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    wallet: Arc<Wallet>,
    rate_limiter: Arc<RateLimiter>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::POST, "/mint/tokens") => {
            // Check mint rate limiting
            match rate_limiter.check_rate_limit().await {
                Ok(()) => {
                    info!("🪙 Mint request accepted");
                    match create_mint_token(wallet).await {
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
                    warn!("⏳ Mint request rate limited - {} seconds remaining", remaining.as_secs());
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
        _ => {
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Not Found")))
        }
    };

    Ok(response.unwrap())
}

pub async fn run_faucet_api(
    port: u16,
    wallet: Arc<Wallet>,
) {
    let addr = format!("127.0.0.1:{}", port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind faucet API to {}: {}", addr, e);
            return;
        }
    };

    info!("🚰 Faucet API listening on http://{}", addr);

    let rate_limiter = Arc::new(RateLimiter::new());

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

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(move |req| {
                    handle_request(req, wallet_clone.clone(), rate_limiter_clone.clone())
                }))
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}
