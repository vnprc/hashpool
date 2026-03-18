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

use cdk::amount::SplitTarget;
use cdk::nuts::SecretKey;
use cdk::wallet::Wallet;
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

async fn create_mint_token(wallet: Arc<Wallet>, locking_privkey: Option<SecretKey>) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Create a 32 diff token (32 sat amount)
    let amount = Amount::from(32u64);

    info!("🪙 Creating mint token for {} ehash", amount);

    // Check wallet balance first
    let balance = wallet.total_balance().await?;
    if balance < amount {
        error!("❌ Insufficient balance in wallet: {} diff available, need {} ehash", balance, amount);
        return Err("Insufficient balance in wallet".into());
    }

    // Swap to get exactly the amount needed
    let unspent_proofs = wallet.get_unspent_proofs().await
        .map_err(|e| format!("Failed to get unspent proofs: {}", e))?;

    // Sign any P2PK-locked proofs before swapping. Proofs minted with
    // SpendingConditions::new_p2pk require a witness signature or the mint
    // will reject them with "missing signature".
    let unspent_proofs = if let Some(ref key) = locking_privkey {
        unspent_proofs.into_iter().map(|mut proof| {
            if let Err(e) = proof.sign_p2pk(key.clone()) {
                warn!("Failed to sign proof for swap: {}", e);
            }
            proof
        }).collect()
    } else {
        unspent_proofs
    };

    let single_proof = match wallet.swap(Some(amount), SplitTarget::default(), unspent_proofs, None, false, false).await {
        Ok(Some(proofs)) => {
            let total_amount: Amount = proofs.iter().fold(Amount::ZERO, |acc, p| acc + p.amount);
            info!("💱 Swapped for {} proofs totaling {} ehash", proofs.len(), total_amount);
            proofs
        }
        Ok(None) => {
            error!("❌ Swap returned no proofs");
            return Err("Failed to prepare token: swap returned no proofs".into());
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
    locking_privkey: Option<SecretKey>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::POST, "/mint/tokens") => {
            // Check mint rate limiting
            match rate_limiter.check_rate_limit().await {
                Ok(()) => {
                    info!("🪙 Mint request accepted");
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
        warn!("🚰 Faucet started without locking_privkey; swapping P2PK-locked proofs will fail");
    }

    // Parse the locking key once at startup
    let parsed_key: Option<SecretKey> = locking_privkey.as_deref().and_then(|hex_str| {
        hex::decode(hex_str).ok().and_then(|bytes| SecretKey::from_slice(&bytes).ok())
    });

    info!("🚰 Faucet API listening on http://{} (timeout: {}s)", addr, timeout_secs);

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
                .serve_connection(io, service_fn(move |req| {
                    handle_request(req, wallet_clone.clone(), rate_limiter_clone.clone(), key_clone.clone())
                }))
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}
