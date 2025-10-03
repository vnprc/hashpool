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

use proxy_stats::db::StatsDatabase;
use web_assets::icons::{nav_icon_css, pickaxe_favicon_inline_svg};

static MINERS_PAGE_HTML: OnceLock<String> = OnceLock::new();
static HTML_PAGE_HTML: OnceLock<Bytes> = OnceLock::new();
static POOL_PAGE_HTML: OnceLock<String> = OnceLock::new();

const MINERS_PAGE_TEMPLATE: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Connected Miners</title>
    <link rel="icon" type="image/svg+xml" sizes="any" href="/favicon.svg">
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
        h1 {
            text-align: center;
            margin-bottom: 30px;
        }
        .stats {
            display: flex;
            justify-content: space-around;
            margin-bottom: 40px;
        }
        .stat-box {
            text-align: center;
            padding: 20px;
            border: 1px solid #00ff00;
            min-width: 150px;
        }
        .stat-value {
            font-size: 2em;
            margin-top: 10px;
        }
        table {
            width: 100%;
            border-collapse: collapse;
            text-align: left;
        }
        th, td {
            padding: 12px;
            text-align: left;
            border-bottom: 1px solid #00ff00;
        }
        th {
            background: #0a0a0a;
            font-weight: bold;
        }
        tr:hover {
            background: #0a0a0a;
        }
        .nav {
            margin-bottom: 30px;
            text-align: center;
        }
        .nav a {
            color: #00ff00;
            text-decoration: none;
            margin: 0 20px;
            font-size: 1.2em;
            white-space: nowrap;
            display: inline-block;
        }
        .nav a:hover {
            text-shadow: 0 0 10px #00ff00;
        }
        .refresh {
            text-align: right;
            margin-bottom: 10px;
            font-size: 0.9em;
            opacity: 0.7;
        }
        /* {{NAV_ICON_CSS}} */
    </style>
</head>
<body>
    <div class="container">
        <div class="nav">
            <a href="/"><span class="wallet-icon">Wallet</span></a> | <a href="/miners"><span class="pickaxe-icon">Miners</span></a> | <a href="/pool"><span class="miner-icon">Pool</span></a>
        </div>

        <h1>Mining Devices</h1>

        <div style="margin: 30px 0; padding: 20px; border: 1px solid #00ff00; text-align: left;">
            <h3 style="margin-top: 0; text-align: center;">Connection Settings</h3>
            <div style="font-family: monospace; font-size: 1.1em;">
                <div style="margin: 10px 0;"><strong>Server:</strong> <span style="color: #ffff00;">{0}</span></div>
                <div style="margin: 10px 0;"><strong>Port:</strong> <span style="color: #ffff00;">{1}</span></div>
                <div style="margin: 10px 0;"><strong>Protocol:</strong> <span style="color: #ffff00;">Stratum V1</span></div>
                <div style="margin: 10px 0;"><strong>Username:</strong> <span style="color: #ffff00;">your-worker-name</span></div>
                <div style="margin: 10px 0;"><strong>Password:</strong> <span style="color: #ffff00;">x</span></div>
            </div>
            <div style="margin-top: 15px; font-size: 0.9em; opacity: 0.8;">
                Example: <code style="background: #333; padding: 5px;">cgminer -o stratum+tcp://{0}:{1} -u worker1 -p x</code>
            </div>
        </div>

        <div class="stats">
            <div class="stat-box">
                <div>Connected Miners</div>
                <div class="stat-value" id="total-miners">-</div>
            </div>
            <div class="stat-box">
                <div>Total Hashrate</div>
                <div class="stat-value" id="total-hashrate">-</div>
            </div>
            <div class="stat-box">
                <div>Total Shares</div>
                <div class="stat-value" id="total-shares">-</div>
            </div>
        </div>

        <div class="refresh" id="refresh-time">Loading...</div>

        <table style="width: 100%; border-collapse: collapse;">
            <thead>
                <tr>
                    <th style="width: 2.5em;"></th>
                    <th>Name</th>
                    <th>ID</th>
                    <th>Address</th>
                    <th>Hashrate</th>
                    <th>Shares</th>
                    <th>Connected</th>
                </tr>
            </thead>
            <tbody id="miners-tbody">
                <tr><td colspan="6" style="text-align: center; opacity: 0.5;">No miners connected</td></tr>
            </tbody>
        </table>
    </div>

    <script>
        async function updateMiners() {
            try {
                const response = await fetch('/api/miners');
                const data = await response.json();

                document.getElementById('total-miners').textContent = data.total_miners || 0;
                document.getElementById('total-hashrate').textContent = data.total_hashrate || '0 H/s';
                document.getElementById('total-shares').textContent = (data.total_shares || 0).toLocaleString();

                const tbody = document.getElementById('miners-tbody');
                tbody.innerHTML = '';

                if (!data.miners || data.miners.length === 0) {
                    tbody.innerHTML = '<tr><td colspan="7" style="text-align: center; opacity: 0.5;">No miners connected</td></tr>';
                } else {
                    data.miners.forEach(miner => {
                        const row = tbody.insertRow();
                        const iconCell = row.insertCell();
                        iconCell.style.textAlign = 'center';
                        iconCell.innerHTML = '<span class="pickaxe-icon" aria-hidden="true"></span>';

                        row.insertCell().textContent = miner.name || 'Unknown';
                        row.insertCell().textContent = miner.id || '-';
                        row.insertCell().textContent = miner.address || '-';
                        row.insertCell().textContent = miner.hashrate || '0 H/s';
                        row.insertCell().textContent = (miner.shares || 0).toLocaleString();
                        row.insertCell().textContent = miner.connected_time || 'Just now';
                    });
                }

                document.getElementById('refresh-time').textContent =
                    'Updated: ' + new Date().toLocaleTimeString();
            } catch (error) {
                console.error('Failed to fetch miners:', error);
                document.getElementById('refresh-time').textContent = 'Error loading data';
            }
        }

        // Update immediately and then every 3 seconds
        updateMiners();
        setInterval(updateMiners, 3000);
    </script>
</body>
</html>"#;

const HTML_PAGE_TEMPLATE: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Ehash Wallet</title>
    <link rel="icon" type="image/svg+xml" sizes="any" href="/favicon.svg">
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
        .wallet {
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
            display: inline-block;
        }
        .offline {
            color: #ff4444;
            border-color: #ff4444;
        }
        .status-dot {
            display: inline-block;
            width: 10px;
            height: 10px;
            border-radius: 50%;
            margin-right: 8px;
        }
        .status-up {
            background-color: #00ff00;
            box-shadow: 0 0 5px #00ff00;
        }
        .status-down {
            background-color: #ff4444;
            box-shadow: 0 0 5px #ff4444;
        }
        .nav {
            margin-bottom: 30px;
        }
        .nav a {
            color: #00ff00;
            text-decoration: none;
            margin: 0 20px;
            font-size: 1.2em;
            white-space: nowrap;
            display: inline-block;
        }
        .nav a:hover {
            text-shadow: 0 0 10px #00ff00;
        }
        .mint-button {
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
        .mint-button:hover {
            background: #00ff00;
            color: #1a1a1a;
            text-shadow: none;
        }
        .mint-button:disabled {
            opacity: 0.5;
            cursor: not-allowed;
        }
        .qr-container {
            display: grid;
            place-items: center;
            margin: 30px auto;
            padding: 40px;
            border: 1px solid #00ff00;
            background: #222;
            border-radius: 5px;
            opacity: 0;
            visibility: hidden;
            transition: opacity 0.3s ease, visibility 0.3s ease;
            width: 400px;
            height: 400px;
            box-sizing: border-box;
        }
        .qr-container.visible {
            opacity: 1;
            visibility: visible;
        }
        .qr-code {
            cursor: pointer;
            padding: 15px;
            background: white;
            border-radius: 5px;
            display: block;
            width: 280px;
            height: 280px;
            box-sizing: border-box;
        }
        #qr-canvas {
            background: white;
            width: 100%;
            height: 100%;
            object-fit: contain;
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
        /* {{NAV_ICON_CSS}} */
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
            <a href="/"><span class="wallet-icon">Wallet</span></a> | <a href="/miners"><span class="pickaxe-icon">Miners</span></a> | <a href="/pool"><span class="miner-icon">Pool</span></a>
        </div>

        <h1>Ehash Wallet</h1>
        <div class="wallet" id="wallet">---</div>

        <button class="mint-button" id="drip-btn" onclick="requestDrip()">
            <span class="qr-icon"></span>Mint
        </button>

        <div class="status" id="status" style="text-align: center; border: none; display: block; margin: 20px auto;"></div>

        <div class="qr-container" id="qr-container">
            <canvas id="qr-canvas" class="qr-code" onclick="copyToken()" title="Click to copy token"></canvas>
        </div>
        <div id="qr-status" style="margin-top: 10px; font-size: 0.9em; color: #00ff00;"></div>
        <p id="qr-instruction" style="margin: 10px 0; opacity: 0; transition: opacity 0.3s ease;">click to copy</p>

        <div id="debug" style="margin-top: 20px; font-size: 0.8em; opacity: 0.6;"></div>
    </div>

    <script>
        const walletEl = document.getElementById('wallet');
        const debugEl = document.getElementById('debug');

        function log(msg) {
            console.log(msg);
            if (debugEl) {
                debugEl.textContent = new Date().toLocaleTimeString() + ': ' + msg;
            }
        }

        function updateWalletDisplay() {
            if (!walletEl) return; // Skip if element doesn't exist

            fetch('/balance')
                .then(response => response.json())
                .then(data => {
                    // Format balance with commas using the raw value
                    walletEl.textContent = data.balance_raw.toLocaleString() + ' ehash';
                })
                .catch(e => {
                    walletEl.textContent = '---';
                    log('Fetch failed: ' + e.message);
                });
        }

        // Update wallet immediately and then every 3 seconds
        updateWalletDisplay();
        setInterval(updateWalletDisplay, 3000);

        // Faucet functionality
        function setButtonClockState(btn, label) {
            btn.innerHTML = `<span class="clock-icon" aria-hidden="true"></span><span>${label}</span>`;
        }

        async function requestDrip() {
            const btn = document.getElementById('drip-btn');
            const status = document.getElementById('status');
            const qrContainer = document.getElementById('qr-container');

            btn.disabled = true;
            setButtonClockState(btn, 'Minting...');
            status.textContent = 'Creating your ehash tokens...';
            status.className = 'status';
            qrContainer.classList.remove('visible');
            document.getElementById('qr-instruction').style.opacity = '0';

            try {
                const response = await fetch('/mint/tokens', { method: 'POST' });
                const data = await response.json();

                if (response.ok && data.success) {
                    status.innerHTML = `Success! Minted ${data.amount} ehash<br><br>Redeem <a href="https://wallet.hashpool.dev" target="_blank" style="color: #00ff00; text-decoration: underline;">here</a>`;
                    status.className = 'status success';

                    // Generate QR code for the token
                    generateQR(data.token);
                    qrContainer.classList.add('visible');
                    document.getElementById('qr-instruction').style.opacity = '1';

                    // Re-enable button immediately - server handles rate limiting
                    btn.disabled = false;
                    btn.innerHTML = '<span class="qr-icon"></span>Mint';
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
                btn.textContent = 'Request Tokens';
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
            status.textContent = '';
            status.className = 'status';

            function updateCountdown() {
                if (remaining <= 0) {
                    // Countdown finished
                    clearInterval(countdownTimer);
                    btn.disabled = false;
                    btn.innerHTML = '<span class="qr-icon"></span>Mint';
                    status.textContent = '';
                    status.className = 'status';
                    countdownTimer = null;
                } else {
                    // Update button with countdown
                    setButtonClockState(btn, `Wait ${remaining}s`);
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

            try {
                generateQRCode(canvas, token);
            } catch (error) {
                console.error('QR generation failed:', error);
                status.textContent = `❌ QR generation failed: ${error.message}`;

                // Display error message on canvas
                const ctx = canvas.getContext('2d');
                canvas.width = 300;
                canvas.height = 300;
                ctx.fillStyle = '#222222';
                ctx.fillRect(0, 0, 300, 300);
                ctx.fillStyle = '#ff4444';
                ctx.font = '16px Courier New';
                ctx.textAlign = 'center';
                ctx.fillText('QR Generation Failed', 150, 130);
                ctx.fillText(`${token.length} characters`, 150, 150);
                ctx.fillText('Copy text below', 150, 170);
            }
        }

        function copyToken() {
            if (currentToken) {
                navigator.clipboard.writeText(currentToken).then(() => {
                    const status = document.getElementById('status');
                    const btn = document.getElementById('drip-btn');
                    const originalHTML = status.innerHTML;
                    // Only replace the first line, keep the redeem link
                    const lines = originalHTML.split('<br>');
                    lines[0] = 'Token copied to clipboard!';
                    status.innerHTML = lines.join('<br>');

                    setTimeout(() => {
                        // Don't restore message if we're in countdown mode
                        if (!btn.disabled || !btn.textContent.includes('Wait')) {
                            status.innerHTML = originalHTML;
                        } else {
                            status.innerHTML = '';
                        }
                    }, 2000);
                }).catch(err => {
                    console.error('Copy failed:', err);
                });
            }
        }
    </script>
</body>
</html>"#;

const POOL_PAGE_TEMPLATE: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Pool Settings</title>
    <link rel="icon" type="image/svg+xml" sizes="any" href="/favicon.svg">
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
            text-align: center;
        }
        h1 {
            text-align: center;
            margin-bottom: 30px;
        }
        .nav {
            margin-bottom: 30px;
        }
        .nav a {
            color: #00ff00;
            text-decoration: none;
            margin: 0 20px;
            font-size: 1.2em;
            white-space: nowrap;
            display: inline-block;
        }
        .nav a:hover {
            text-shadow: 0 0 10px #00ff00;
        }
        .stats {
            display: flex;
            justify-content: space-around;
            margin-bottom: 40px;
        }
        .stat-box {
            text-align: center;
            padding: 20px;
            border: 1px solid #00ff00;
            min-width: 150px;
        }
        .stat-value {
            font-size: 2em;
            margin-top: 10px;
        }
        .status {
            margin: 20px 0;
            padding: 10px;
            border: 1px solid #00ff00;
            display: inline-block;
        }
        .offline {
            color: #ff4444;
            border-color: #ff4444;
        }
        .status-dot {
            display: inline-block;
            width: 10px;
            height: 10px;
            border-radius: 50%;
            margin-right: 8px;
        }
        .status-up {
            background-color: #00ff00;
            box-shadow: 0 0 5px #00ff00;
        }
        .status-down {
            background-color: #ff4444;
            box-shadow: 0 0 5px #ff4444;
        }
        /* {{NAV_ICON_CSS}} */
    </style>
</head>
<body>
    <div class="container">
        <div class="nav">
            <a href="/"><span class="wallet-icon">Wallet</span></a> | <a href="/miners"><span class="pickaxe-icon">Miners</span></a> | <a href="/pool"><span class="miner-icon">Pool</span></a>
        </div>

        <h1>Mining Pool</h1>

        <div style="margin: 30px 0; padding: 20px; border: 1px solid #00ff00; text-align: left;">
            <h3 style="margin-top: 0; text-align: center;">Pool Settings</h3>
            <div style="font-family: monospace; font-size: 1.1em;">
                <div style="margin: 10px 0;"><strong>Pool:</strong> <span style="color: #ffff00;">Hashpool</span></div>
                <div style="margin: 10px 0;"><strong>Server:</strong> <span style="color: #ffff00;">{upstream_address}</span></div>
                <div style="margin: 10px 0;"><strong>Port:</strong> <span style="color: #ffff00;">{upstream_port}</span></div>
                <div style="margin: 10px 0;"><strong>Protocol:</strong> <span style="color: #ffff00;">Stratum V2</span></div>
            </div>
        </div>

        <div class="stats">
            <div class="stat-box">
                <div>Blockchain</div>
                <div class="stat-value" id="blockchain-status">{blockchain_network}</div>
            </div>
            <div class="stat-box">
                <div>Block Height</div>
                <div class="stat-value" id="block-height-status">-</div>
            </div>
            <div class="stat-box">
                <div>Last Block Found</div>
                <div class="stat-value" id="last-block-status">-</div>
            </div>
        </div>

        <div class="status" id="status">Connecting...</div>
    </div>

    <script>
        const statusEl = document.getElementById('status');
        const blockchainEl = document.getElementById('blockchain-status');
        const blockHeightEl = document.getElementById('block-height-status');
        const lastBlockEl = document.getElementById('last-block-status');

        function updatePoolStatus() {
            if (!statusEl) return; // Skip if element doesn't exist

            fetch('/balance')
                .then(response => response.json())
                .then(data => {
                    statusEl.innerHTML = '<span class="status-dot status-up"></span>Connected';
                    statusEl.className = 'status';

                    // TODO: Update these with real data when available
                    // For now, keep blockchain static and others as placeholders
                    if (blockHeightEl) blockHeightEl.textContent = '-';
                    if (lastBlockEl) lastBlockEl.textContent = '-';
                })
                .catch(e => {
                    statusEl.innerHTML = '<span class="status-dot status-down"></span>Connection Lost';
                    statusEl.className = 'status offline';

                    // Show disconnected state for status boxes
                    if (blockHeightEl) blockHeightEl.textContent = '-';
                    if (lastBlockEl) lastBlockEl.textContent = '-';
                });
        }

        // Update immediately and then every 3 seconds
        updatePoolStatus();
        setInterval(updatePoolStatus, 3000);
    </script>
</body>
</html>"#;

pub async fn run_http_server(
    address: String,
    db: Arc<StatsDatabase>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&address).await?;
    info!("🌐 HTTP dashboard listening on http://{}", address);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let db = db.clone();

        tokio::task::spawn(async move {
            let service = service_fn(move |req| {
                let db = db.clone();
                async move { handle_request(req, db).await }
            });

            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn handle_request(
    req: Request<Incoming>,
    db: Arc<StatsDatabase>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/favicon.ico") | (&Method::GET, "/favicon.svg") => Ok(serve_favicon()),
        (&Method::GET, "/") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(html_page()))
        }
        (&Method::GET, "/miners") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(miners_page("localhost", 3333)))
        }
        (&Method::GET, "/pool") => {
            Response::builder()
                .header("content-type", "text/html; charset=utf-8")
                .body(Full::new(pool_page("localhost".to_string(), 34254)))
        }
        (&Method::GET, "/api/miners") => {
            let stats = get_miner_stats(db).await;
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(stats.to_string())))
        }
        (&Method::POST, "/mint/tokens") => {
            // Proxy mint request to translator's faucet API
            let translator_faucet_url = "http://127.0.0.1:8083/mint/tokens";

            match reqwest::Client::new()
                .post(translator_faucet_url)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    match response.text().await {
                        Ok(body) => {
                            Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(body)))
                        }
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
        (&Method::GET, "/balance") => {
            // Return translator wallet balance
            let balance = get_wallet_balance(db).await;
            let json_response = json!({
                "balance": format!("{} ehash", balance),
                "balance_raw": balance,
                "unit": "HASH"
            });
            Response::builder()
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(json_response.to_string())))
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

fn miners_page(address: &str, port: u16) -> Bytes {
    let html = MINERS_PAGE_HTML.get_or_init(|| {
        MINERS_PAGE_TEMPLATE
            .replace("/* {{NAV_ICON_CSS}} */", nav_icon_css())
    });

    let formatted_html = html
        .replace("{0}", address)
        .replace("{1}", &port.to_string());
    Bytes::from(formatted_html)
}

fn html_page() -> Bytes {
    HTML_PAGE_HTML
        .get_or_init(|| {
            Bytes::from(HTML_PAGE_TEMPLATE.replace("/* {{NAV_ICON_CSS}} */", nav_icon_css()))
        })
        .clone()
}

fn pool_page(upstream_address: String, upstream_port: u16) -> Bytes {
    let blockchain_network = std::env::var("BITCOIND_NETWORK")
        .unwrap_or_else(|_| "testnet4".to_string());

    let html = POOL_PAGE_HTML.get_or_init(|| {
        POOL_PAGE_TEMPLATE
            .replace("/* {{NAV_ICON_CSS}} */", nav_icon_css())
    });

    let formatted_html = html
        .replace("{upstream_address}", &upstream_address)
        .replace("{upstream_port}", &upstream_port.to_string())
        .replace("{blockchain_network}", &blockchain_network);

    Bytes::from(formatted_html)
}

async fn get_wallet_balance(db: Arc<StatsDatabase>) -> u64 {
    db.get_balance().unwrap_or(0)
}

async fn get_miner_stats(db: Arc<StatsDatabase>) -> serde_json::Value {
    let stats = match db.get_current_stats() {
        Ok(stats) => stats,
        Err(_) => return json!({
            "total_miners": 0,
            "total_hashrate": "0 H/s",
            "total_shares": 0,
            "miners": []
        })
    };

    let total_miners = stats.len();
    let total_shares: u64 = stats.iter().map(|s| s.shares_submitted).sum();
    let total_hashrate_raw: f64 = stats.iter().map(|s| s.current_hashrate).sum();

    let total_hashrate = format_hashrate(total_hashrate_raw);

    let miners: Vec<serde_json::Value> = stats.iter().map(|s| {
        let connected_time = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let elapsed = (now - s.connected_at) as u64;
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
            "name": s.name,
            "id": s.downstream_id,
            "address": "-",
            "hashrate": format_hashrate(s.current_hashrate),
            "shares": s.shares_submitted,
            "connected_time": connected_time
        })
    }).collect();

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
