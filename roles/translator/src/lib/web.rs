use std::sync::Arc;
use std::convert::Infallible;
use std::time::{Duration, Instant};

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{body::Bytes, Request, Response, Method, StatusCode};
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

const HTML_PAGE: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Ehash Balance</title>
    <style>
        body { 
            font-family: 'Courier New', monospace; 
            background: #1a1a1a; 
            color: #00ff00; 
            margin: 0; 
            padding: 20px;
            text-align: center;
        }
        .container { 
            max-width: 800px; 
            margin: 0 auto; 
            padding: 40px;
        }
        .balance { 
            font-size: 4em; 
            margin: 40px 0; 
            text-shadow: 0 0 10px #00ff00;
        }
        .unit { 
            font-size: 2em; 
            opacity: 0.8; 
        }
        .status { 
            margin: 20px 0; 
            padding: 10px; 
            border: 1px solid #00ff00; 
        }
        .offline { 
            color: #ff4444; 
            border-color: #ff4444; 
        }
        .nav {
            margin-bottom: 30px;
        }
        .nav a {
            color: #00ff00;
            text-decoration: none;
            margin: 0 20px;
            font-size: 1.2em;
        }
        .nav a:hover {
            text-shadow: 0 0 10px #00ff00;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="nav">
            <a href="/">📊 Balance</a> | <a href="/faucet">🚰 Faucet</a>
        </div>
        
        <h1>📊 Ehash Balance</h1>
        <div class="status" id="status">Connecting...</div>
        <div class="balance" id="balance">---</div>
        <div class="unit">HASH</div>
        <p>Balance updates in real-time as shares are accepted</p>
        <div id="debug" style="margin-top: 20px; font-size: 0.8em; opacity: 0.6;"></div>
    </div>
    
    <script>
        const balanceEl = document.getElementById('balance');
        const statusEl = document.getElementById('status');
        const debugEl = document.getElementById('debug');
        
        function log(msg) {
            console.log(msg);
            debugEl.textContent = new Date().toLocaleTimeString() + ': ' + msg;
        }
        
        function updateBalance() {
            fetch('/balance')
                .then(response => response.json())
                .then(data => {
                    statusEl.textContent = '🟢 Connected';
                    statusEl.className = 'status';
                    balanceEl.textContent = data.balance.toLocaleString();
                    log('Balance updated: ' + data.balance);
                })
                .catch(e => {
                    statusEl.textContent = '🔴 Connection Lost';
                    statusEl.className = 'status offline';
                    balanceEl.textContent = '---';
                    log('Fetch failed: ' + e.message);
                });
        }
        
        // Update balance immediately and then every 3 seconds
        updateBalance();
        setInterval(updateBalance, 3000);
    </script>
</body>
</html>"#;

const FAUCET_PAGE: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Ehash Faucet 🚰</title>
    <style>
        body { 
            font-family: 'Courier New', monospace; 
            background: #1a1a1a; 
            color: #00ff00; 
            margin: 0; 
            padding: 20px;
            text-align: center;
        }
        .container { 
            max-width: 600px; 
            margin: 0 auto; 
            padding: 40px;
        }
        .faucet-button { 
            font-size: 2em; 
            padding: 20px 40px;
            background: transparent;
            border: 2px solid #00ff00;
            color: #00ff00;
            font-family: inherit;
            cursor: pointer;
            margin: 20px;
            transition: all 0.3s;
        }
        .faucet-button:hover {
            background: #00ff00;
            color: #1a1a1a;
            text-shadow: none;
        }
        .faucet-button:disabled {
            opacity: 0.5;
            cursor: not-allowed;
        }
        .qr-container {
            margin: 30px auto;
            padding: 20px;
            border: 1px solid #00ff00;
            background: #222;
            display: none;
            width: fit-content;
        }
        .qr-code {
            margin: 20px 0;
            cursor: pointer;
            display: inline-block;
            padding: 10px;
            border: 2px solid #00ff00;
            background: white;
            border-radius: 5px;
        }
        #qr-canvas {
            background: white;
        }
        .token-info {
            font-size: 0.8em;
            margin: 20px 0;
            padding: 15px;
            border: 1px solid #00ff00;
            background: #222;
            word-break: break-all;
            max-height: 150px;
            overflow-y: auto;
            font-family: 'Courier New', monospace;
        }
        .status { 
            margin: 20px 0; 
            padding: 10px; 
        }
        .success { 
            color: #00ff00; 
        }
        .error { 
            color: #ff4444; 
        }
        .nav {
            margin-bottom: 30px;
        }
        .nav a {
            color: #00ff00;
            text-decoration: none;
            margin: 0 20px;
            font-size: 1.2em;
        }
        .nav a:hover {
            text-shadow: 0 0 10px #00ff00;
        }
    </style>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/qrcode-generator/1.4.4/qrcode.min.js"></script>
    <script>
        // Simple QR generation - no animation needed for 370 chars
        function generateQRCode(canvas, text) {
            const qr = qrcode(0, 'L'); // Type 0, error correction level L
            qr.addData(text);
            qr.make();
            
            const cellSize = 8;
            const margin = 2;
            const moduleCount = qr.getModuleCount();
            const canvasSize = (moduleCount + margin * 2) * cellSize;
            
            canvas.width = canvasSize;
            canvas.height = canvasSize;
            
            const ctx = canvas.getContext('2d');
            ctx.fillStyle = '#FFFFFF';
            ctx.fillRect(0, 0, canvasSize, canvasSize);
            
            ctx.fillStyle = '#000000';
            for (let row = 0; row < moduleCount; row++) {
                for (let col = 0; col < moduleCount; col++) {
                    if (qr.isDark(row, col)) {
                        ctx.fillRect(
                            (col + margin) * cellSize,
                            (row + margin) * cellSize,
                            cellSize,
                            cellSize
                        );
                    }
                }
            }
        }
    </script>
</head>
<body>
    <div class="container">
        <div class="nav">
            <a href="/">📊 Balance</a> | <a href="/faucet">🚰 Faucet</a>
        </div>
        
        <h1>🚰 Ehash Faucet</h1>
        <p>Get free ehash tokens for testing!</p>
        
        <button class="faucet-button" id="drip-btn" onclick="requestDrip()">
            💧 Request Tokens
        </button>
        
        <div class="status" id="status"></div>
        
        <div class="qr-container" id="qr-container">
            <h3>🎫 Your Token</h3>
            <canvas id="qr-canvas" class="qr-code" onclick="copyToken()" title="Click to copy token" width="400" height="400"></canvas>
            <div style="margin: 10px 0;">
                <span id="qr-status" style="font-size: 0.9em; color: #00ff00;"></span>
            </div>
            <div class="token-info" id="token-info" style="display: none;"></div>
            <p>👆 Click QR to copy token • Animated UR codes work with compatible wallets</p>
        </div>
    </div>
    
    <script>
        // No library loading needed - the qrcode-generator library is much more reliable

        async function requestDrip() {
            const btn = document.getElementById('drip-btn');
            const status = document.getElementById('status');
            const qrContainer = document.getElementById('qr-container');
            
            btn.disabled = true;
            btn.textContent = '⏳ Minting...';
            status.textContent = 'Creating your ehash tokens...';
            status.className = 'status';
            qrContainer.style.display = 'none';
            
            try {
                const response = await fetch('/faucet/drip', { method: 'POST' });
                const data = await response.json();
                
                if (response.ok && data.success) {
                    status.textContent = `✅ Success! Minted ${data.amount} ehash tokens (${data.token.length} chars)`;
                    status.className = 'status success';
                    
                    // Generate QR code for the token
                    generateQR(data.token);
                    document.getElementById('token-info').textContent = data.token;
                    qrContainer.style.display = 'block';
                    
                    // Re-enable button immediately - server handles rate limiting
                    btn.disabled = false;
                    btn.textContent = '💧 Request Tokens';
                } else {
                    throw new Error(data.error || 'Unknown error');
                }
            } catch (error) {
                // Check if it's a rate limit error with remaining time
                if (error.message.includes('Rate limited') && error.message.includes('seconds')) {
                    const match = error.message.match(/(\d+) seconds/);
                    if (match) {
                        startCountdown(parseInt(match[1]), btn, status);
                        return;
                    }
                }
                
                // For non-rate-limit errors, show error message
                status.textContent = `❌ Error: ${error.message}`;
                status.className = 'status error';
                btn.disabled = false;
                btn.textContent = '💧 Request Tokens';
            }
        }
        
        let currentToken = '';
        let countdownTimer = null;
        
        function startCountdown(seconds, btn, status) {
            // Clear any existing countdown
            if (countdownTimer) {
                clearInterval(countdownTimer);
            }
            
            let remaining = seconds;
            btn.disabled = true;
            
            function updateCountdown() {
                if (remaining <= 0) {
                    // Countdown finished
                    clearInterval(countdownTimer);
                    btn.disabled = false;
                    btn.textContent = '💧 Request Tokens';
                    status.textContent = '';
                    status.className = 'status';
                    countdownTimer = null;
                } else {
                    // Update button with countdown
                    btn.textContent = `⏱️ Wait ${remaining}s`;
                    remaining--;
                }
            }
            
            // Start immediately and then every second
            updateCountdown();
            countdownTimer = setInterval(updateCountdown, 1000);
        }
        
        function generateQR(token) {
            currentToken = token;
            const canvas = document.getElementById('qr-canvas');
            const status = document.getElementById('qr-status');
            
            console.log('Generating QR for token length:', token.length);
            
            // Always show token info for debugging
            document.getElementById('token-info').textContent = token;
            document.getElementById('token-info').style.display = 'block';
            
            try {
                generateQRCode(canvas, token);
                status.textContent = `✅ QR Code Generated (${token.length} chars)`;
            } catch (error) {
                console.error('QR generation failed:', error);
                status.textContent = `❌ QR generation failed: ${error.message}`;
                
                // Display error message on canvas
                const ctx = canvas.getContext('2d');
                ctx.fillStyle = '#222222';
                ctx.fillRect(0, 0, 400, 400);
                ctx.fillStyle = '#ff4444';
                ctx.font = '16px Courier New';
                ctx.textAlign = 'center';
                ctx.fillText('QR Generation Failed', 200, 180);
                ctx.fillText(`${token.length} characters`, 200, 200);
                ctx.fillText('Copy text below', 200, 220);
            }
        }
        
        function copyToken() {
            if (currentToken) {
                navigator.clipboard.writeText(currentToken).then(() => {
                    const status = document.getElementById('status');
                    const originalText = status.textContent;
                    status.textContent = '📋 Token copied to clipboard!';
                    setTimeout(() => {
                        status.textContent = originalText;
                    }, 2000);
                }).catch(err => {
                    console.error('Copy failed:', err);
                });
            }
        }
    </script>
</body>
</html>"#;

pub async fn start_web_server(wallet: Arc<Wallet>, port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    let faucet_rate_limiter = Arc::new(RateLimiter::new());
    info!("🌐 Web server starting on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let wallet_clone = wallet.clone();
        let faucet_rate_limiter_clone = faucet_rate_limiter.clone();

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(move |req| {
                    handle_request(req, wallet_clone.clone(), faucet_rate_limiter_clone.clone())
                }))
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn create_faucet_token(wallet: Arc<Wallet>) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Create a 32 diff token (32 sat amount)
    let amount = Amount::from(32u64);
    
    info!("🚰 Creating faucet token for {} diff", amount);
    
    // Check wallet balance first
    let balance = wallet.total_balance().await?;
    if balance < amount {
        error!("❌ Insufficient balance in wallet: {} diff available, need {} diff", balance, amount);
        return Err("Insufficient balance in wallet".into());
    }
    
    // First, swap to get exactly one proof of 32 sats
    // This ensures we have the exact denomination we need
    let single_proof = match wallet.swap_from_unspent(amount, None, false).await {
        Ok(proofs) => {
            let total_amount: Amount = proofs.iter().fold(Amount::ZERO, |acc, p| acc + p.amount);
            info!("💱 Swapped for {} proofs totaling {} diff", proofs.len(), total_amount);
            proofs
        }
        Err(e) => {
            error!("❌ Failed to swap for single proof: {}", e);
            return Err(Box::new(e));
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
    info!("✅ Faucet token created successfully with {} proofs", single_proof.len());
    Ok(token_string)
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    wallet: Arc<Wallet>,
    faucet_rate_limiter: Arc<RateLimiter>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(Bytes::from(HTML_PAGE)))
        }
        (&Method::GET, "/faucet") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(Bytes::from(FAUCET_PAGE)))
        }
        (&Method::POST, "/faucet/drip") => {
            // Check faucet rate limiting - ONLY for faucet requests
            match faucet_rate_limiter.check_rate_limit().await {
                Ok(()) => {
                    info!("🚰 Faucet request accepted");
                    match create_faucet_token(wallet).await {
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
                            error!("Failed to create faucet token: {}", e);
                            let json_response = json!({
                                "success": false,
                                "error": format!("Failed to create token: {}", e)
                            });
                            Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(json_response.to_string())))
                        }
                    }
                }
                Err(remaining) => {
                    warn!("🚫 Rate limited - {} seconds remaining", remaining.as_secs());
                    let json_response = json!({
                        "success": false,
                        "error": format!("Rate limited. Please wait {} seconds before requesting again.", remaining.as_secs())
                    });
                    Response::builder()
                        .status(StatusCode::TOO_MANY_REQUESTS)
                        .header("content-type", "application/json")
                        .body(Full::new(Bytes::from(json_response.to_string())))
                }
            }
        }
        (&Method::GET, "/balance") => {
            match wallet.total_balance().await {
                Ok(balance) => {
                    let json_response = json!({
                        "balance": u64::from(balance),
                        "unit": "HASH"
                    });
                    Response::builder()
                        .header("content-type", "application/json")
                        .body(Full::new(Bytes::from(json_response.to_string())))
                }
                Err(e) => {
                    error!("Failed to get wallet balance: {}", e);
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Full::new(Bytes::from("Error getting balance")))
                }
            }
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

