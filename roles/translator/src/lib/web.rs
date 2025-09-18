use std::sync::Arc;
use std::convert::Infallible;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{body::Bytes, Request, Response, Method, StatusCode};
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use tokio::net::TcpListener;
use tracing::{info, error};
use serde_json::json;

use cdk::wallet::Wallet;

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
    </style>
</head>
<body>
    <div class="container">
        <h1>Ehash Balance</h1>
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

pub async fn start_web_server(wallet: Arc<Wallet>, port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    info!("🌐 Web server starting on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let wallet_clone = wallet.clone();

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(move |req| {
                    handle_request(req, wallet_clone.clone())
                }))
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    wallet: Arc<Wallet>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(Bytes::from(HTML_PAGE)))
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

