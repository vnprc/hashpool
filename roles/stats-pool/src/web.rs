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

use stats_pool::db::StatsData;
use web_assets::icons::{nav_icon_css, pickaxe_favicon_inline_svg};

static CONNECTIONS_PAGE_HTML: OnceLock<Bytes> = OnceLock::new();

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

            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
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
        (&Method::GET, "/") => serve_connections_page().await,
        (&Method::GET, "/favicon.ico") | (&Method::GET, "/favicon.svg") => serve_favicon(),
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

fn serve_favicon() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "image/svg+xml")
        .body(Full::new(Bytes::from_static(
            pickaxe_favicon_inline_svg().as_bytes(),
        )))
        .unwrap()
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

async fn serve_connections_page() -> Response<Full<Bytes>> {
    let html = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Mining Pool Dashboard</title>
    <link rel="icon" type="image/svg+xml" sizes="any" href="/favicon.svg">
    <style>
        body {
            font-family: 'Courier New', monospace;
            background: #1a1a1a;
            color: #00ff00;
            margin: 0;
            padding: 20px;
        }
        .container {
            max-width: 1200px;
            margin: 0 auto;
        }
        h1 {
            text-align: center;
            margin-bottom: 30px;
        }
        h2 {
            text-align: center;
            margin: 30px 0 20px 0;
            font-size: 1.5em;
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
        .services-section {
            margin-bottom: 40px;
            padding: 20px;
            border: 1px solid #00ff00;
            background: #222;
        }
        .services-section h3 {
            margin-top: 0;
            text-align: center;
            color: #ffff00;
        }
        table {
            width: 100%;
            border-collapse: collapse;
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
        .address {
            font-family: monospace;
            font-size: 0.9em;
        }
        .refresh {
            text-align: right;
            margin-bottom: 10px;
            font-size: 0.9em;
            opacity: 0.7;
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
        .service-table {
            margin-bottom: 20px;
        }
        .service-table th {
            background: #333;
        }
        .services-section h3 {
            color: #00ff00;
        }
        .service-icon-header {
            width: 2.5em;
        }
        .service-icon-cell {
            width: 2.5em;
            text-align: center;
        }
        .miners-table {
            width: 100%;
            border-collapse: collapse;
        }
        .miners-table th,
        .miners-table td {
            padding: 12px;
            border-bottom: 1px solid #00ff00;
        }
        .miners-icon-header {
            width: 2.5em;
        }
        .miners-icon-cell {
            width: 2.5em;
            text-align: center;
        }
        /* {{NAV_ICON_CSS}} */
    </style>
</head>
<body>
    <div class="container">
        <h1 class="pickaxe-icon">Hashpool Dashboard</h1>

        <div class="services-section">
            <h3>Service Connections</h3>
            <table class="service-table">
                <thead>
                    <tr>
                        <th class="service-icon-header"></th>
                        <th>Service Name</th>
                        <th>Channel ID</th>
                        <th>Service (IP)</th>
                        <th>Port</th>
                        <th>Status</th>
                    </tr>
                </thead>
                <tbody id="services-tbody">
                </tbody>
            </table>
        </div>

        <div class="stats">
            <div class="stat-box">
                <div>Connected Miners</div>
                <div class="stat-value" id="total-miners">-</div>
            </div>
            <div class="stat-box">
                <div>Total Shares</div>
                <div class="stat-value" id="total-shares">-</div>
            </div>
            <div class="stat-box">
                <div>Quotes Redeemed</div>
                <div class="stat-value" id="quotes-redeemed">-</div>
            </div>
            <div class="stat-box">
                <div>Ehash Mined</div>
                <div class="stat-value" id="ehash-mined">-</div>
            </div>
        </div>

        <div class="refresh" id="refresh-time">Loading...</div>

        <h2>Connected Proxies</h2>
        <table class="miners-table">
            <thead>
                <tr>
                    <th class="miners-icon-header"></th>
                    <th>ID</th>
                    <th>Address</th>
                    <th>Type</th>
                    <th>Channels</th>
                    <th>Shares</th>
                    <th>Last Share</th>
                </tr>
            </thead>
            <tbody id="miners-tbody">
            </tbody>
        </table>
    </div>

    <script>
        function parseAddress(address) {
            const parts = address.split(':');
            if (parts.length === 2) {
                return { ip: parts[0], port: parts[1] };
            }
            return { ip: address, port: '-' };
        }

        function getServiceMetadata(connType) {
            if (connType.includes('Job Declarator')) {
                return { label: 'Job Declarator', iconClass: 'block-icon' };
            }
            if (connType.includes('Mint')) {
                return { label: 'Mint', iconClass: 'coins-icon' };
            }
            if (connType.includes('Pool')) {
                return { label: 'Pool', iconClass: 'pickaxe-icon' };
            }
            return { label: connType, iconClass: null };
        }

        function isServiceConnection(connType) {
            return connType.includes('Job Declarator') || connType.includes('Mint') || connType.includes('Pool');
        }

        function isDisconnected(connType) {
            return connType.includes('(Disconnected)');
        }

        async function updateConnections() {
            try {
                const response = await fetch('/api/stats');
                const snapshot = await response.json();

                if (snapshot.error) {
                    throw new Error(snapshot.error);
                }

                // Extract services and downstream proxies
                const services = snapshot.services || [];
                const proxies = snapshot.downstream_proxies || [];

                // Calculate aggregate stats
                const totalShares = proxies.reduce((sum, p) => sum + p.shares_submitted, 0);
                const totalQuotes = proxies.reduce((sum, p) => sum + p.quotes_created, 0);
                const totalEhash = proxies.reduce((sum, p) => sum + p.ehash_mined, 0);

                document.getElementById('total-miners').textContent = proxies.length;
                document.getElementById('total-shares').textContent = totalShares.toLocaleString();
                document.getElementById('quotes-redeemed').textContent = '?';
                document.getElementById('ehash-mined').textContent = totalEhash.toLocaleString() + ' ehash';

                // Update services table
                const servicesTbody = document.getElementById('services-tbody');
                servicesTbody.innerHTML = '';

                // Add pool itself
                const poolAddr = parseAddress(snapshot.listen_address);
                const poolRow = servicesTbody.insertRow();
                poolRow.insertCell().innerHTML = '<span class="pickaxe-icon" aria-hidden="true"></span>';
                poolRow.insertCell().textContent = 'Pool';
                poolRow.insertCell().textContent = '-';
                poolRow.insertCell().innerHTML = `<span class="address">${poolAddr.ip}</span>`;
                poolRow.insertCell().textContent = poolAddr.port;
                poolRow.insertCell().innerHTML = '<span class="status-dot status-up"></span>Up';

                // Add services
                services.forEach(service => {
                    const row = servicesTbody.insertRow();
                    const addr = parseAddress(service.address);
                    const iconClass = service.service_type === 'Mint' ? 'coins-icon' : 'block-icon';
                    const label = service.service_type === 'Mint' ? 'Mint' : 'Job Declarator';

                    row.insertCell().innerHTML = `<span class="${iconClass}" aria-hidden="true"></span>`;
                    row.insertCell().textContent = label;
                    row.insertCell().textContent = '-';
                    row.insertCell().innerHTML = `<span class="address">${addr.ip}</span>`;
                    row.insertCell().textContent = addr.port;
                    row.insertCell().innerHTML = '<span class="status-dot status-up"></span>Up';
                });

                // Update proxies table
                const minersTbody = document.getElementById('miners-tbody');
                minersTbody.innerHTML = '';

                if (proxies.length === 0) {
                    minersTbody.innerHTML = '<tr><td colspan="7" style="text-align: center; opacity: 0.5;">No proxies connected</td></tr>';
                } else {
                    proxies.forEach(proxy => {
                        const row = minersTbody.insertRow();
                        row.insertCell().innerHTML = '<span class="miner-icon" aria-hidden="true"></span>';
                        row.insertCell().textContent = proxy.id;
                        row.insertCell().innerHTML = `<span class="address">${proxy.address}</span>`;
                        row.insertCell().textContent = 'Translator';
                        row.insertCell().textContent = proxy.channels.length > 0 ? proxy.channels.join(', ') : 'None';
                        row.insertCell().textContent = proxy.shares_submitted.toLocaleString();

                        // Format last_share_at
                        let lastShareText = 'Never';
                        if (proxy.last_share_at) {
                            const now = Math.floor(Date.now() / 1000);
                            const elapsed = now - proxy.last_share_at;
                            if (elapsed < 60) {
                                lastShareText = `${elapsed}s ago`;
                            } else if (elapsed < 3600) {
                                lastShareText = `${Math.floor(elapsed / 60)}m ago`;
                            } else {
                                lastShareText = `${Math.floor(elapsed / 3600)}h ago`;
                            }
                        }
                        row.insertCell().textContent = lastShareText;
                    });
                }

                document.getElementById('refresh-time').textContent =
                    'Updated: ' + new Date().toLocaleTimeString();
            } catch (error) {
                console.error('Failed to fetch stats:', error);
                document.getElementById('refresh-time').textContent = 'Error loading data';
            }
        }

        // Update immediately and then every 3 seconds
        updateConnections();
        setInterval(updateConnections, 3000);
    </script>
</body>
</html>"#;

    let body = CONNECTIONS_PAGE_HTML
        .get_or_init(|| {
            Bytes::from(html.replace("/* {{NAV_ICON_CSS}} */", nav_icon_css()))
        })
        .clone();

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Full::new(body))
        .unwrap()
}
