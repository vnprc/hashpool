use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing::{error, info};
use bytes::Bytes;

use super::mining_pool::Pool;
use roles_logic_sv2::utils::Mutex;
use web_assets::icons::{nav_icon_css, pickaxe_favicon_inline_svg};
use web_assets::formatting::format_hash_units;

static CONNECTIONS_PAGE_HTML: OnceLock<Bytes> = OnceLock::new();

// Extension methods for Pool  
impl Pool {
    pub fn get_connections_info(&self) -> Vec<ConnectionInfo> {
        let mut connections = Vec::new();
        
        // Add downstream mining connections (including JDC)
        for (id, downstream_arc) in &self.downstreams {
            let result = downstream_arc.safe_lock(|downstream| {
                let shares = futures::executor::block_on(downstream.shares_submitted.lock());
                let quotes = futures::executor::block_on(downstream.quotes_created.lock());
                let quotes_redeemed = futures::executor::block_on(downstream.quotes_redeemed.lock());
                let ehash_mined = futures::executor::block_on(downstream.ehash_mined.lock());
                let last_share = futures::executor::block_on(downstream.last_share_time.lock())
                    .map(|t| {
                        let elapsed = t.elapsed();
                        if elapsed.as_secs() > 0 {
                            format!("{}s ago", elapsed.as_secs())
                        } else {
                            format!("{}ms ago", elapsed.as_millis())
                        }
                    });
                let channels_list = futures::executor::block_on(downstream.channels.lock()).clone();
                
                // Determine connection type based on activity patterns
                let connection_type = if channels_list.is_empty() && *shares == 0 && *quotes == 0 {
                    // No channels, no shares, no quotes = likely JDC
                    "Job Declarator (JDC)".to_string()
                } else if channels_list.is_empty() {
                    "Mining (no channels)".to_string()
                } else {
                    format!("Mining ({} channels)", channels_list.len())
                };
                
                ConnectionInfo {
                    id: *id,
                    address: downstream.address.to_string(),
                    channels: channels_list,
                    shares_submitted: *shares,
                    quotes_created: Some(*quotes),
                    quotes_redeemed: Some(*quotes_redeemed),
                    ehash_mined: Some(*ehash_mined),
                    last_share_time: last_share,
                    connection_type,
                }
            });
            
            if let Ok(conn_info) = result {
                connections.push(conn_info);
            }
        }
        
        // Add expected services (always show, even if disconnected)
        let mint_connections = self.get_mint_connections();
        let has_mint = !mint_connections.is_empty();
        
        // Always show mint service entry
        if has_mint {
            // Mint is connected
            for (address, _sender) in mint_connections {
                connections.push(ConnectionInfo {
                    id: 0,
                    address: address.to_string(),
                    channels: vec![],
                    shares_submitted: 0,
                    quotes_created: None,
                    quotes_redeemed: None,
                    ehash_mined: None,
                    last_share_time: None,
                    connection_type: "Mint".to_string(),
                });
            }
        } else {
            // Mint is disconnected
            connections.push(ConnectionInfo {
                id: 0,
                address: "-".to_string(),
                channels: vec![],
                shares_submitted: 0,
                quotes_created: None,
                quotes_redeemed: None,
                ehash_mined: None,
                last_share_time: None,
                connection_type: "Mint (Disconnected)".to_string(),
            });
        }

        // Add pool service entry representing the dashboard itself
        connections.push(ConnectionInfo {
            id: 0,
            address: self.listen_address.clone(),
            channels: vec![],
            shares_submitted: 0,
            quotes_created: None,
            quotes_redeemed: None,
            ehash_mined: None,
            last_share_time: None,
            connection_type: "Pool".to_string(),
        });

        // Check for active JDC connections in downstreams
        // A JDC is considered active if it was identified as such and has a recent connection
        let has_active_jdc = connections.iter().any(|c| {
            c.connection_type.contains("Job Declarator") && 
            // Consider JDC active if it was detected (regardless of last_share_time since JDCs don't submit shares)
            c.connection_type.contains("Job Declarator")
        });
        
        if !has_active_jdc {
            // No active JDC found, add disconnected entry
            connections.push(ConnectionInfo {
                id: 0,
                address: "-".to_string(),
                channels: vec![],
                shares_submitted: 0,
                quotes_created: None,
                quotes_redeemed: None,
                ehash_mined: None,
                last_share_time: None,
                connection_type: "Job Declarator (Disconnected)".to_string(),
            });
        }
        
        connections
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub id: u32,
    pub address: String,
    pub channels: Vec<u32>,
    pub shares_submitted: u64,
    pub quotes_created: Option<u64>,
    pub quotes_redeemed: Option<u64>,
    pub ehash_mined: Option<u64>,
    pub last_share_time: Option<String>,
    pub connection_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStats {
    pub total_connections: usize,
    pub total_shares: u64,
    pub total_quotes: u64,
    pub quotes_redeemed: Option<u64>,
    pub ehash_mined: String,  // Formatted string
    pub ehash_mined_raw: u64, // Raw value for calculations  
    pub connections: Vec<ConnectionInfo>,
}

pub struct WebServer {
    pool: Arc<Mutex<Pool>>,
    port: u16,
}

impl WebServer {
    pub fn new(pool: Arc<Mutex<Pool>>, port: u16) -> Self {
        Self { pool, port }
    }

    pub async fn start(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        let listener = TcpListener::bind(addr).await?;
        info!("🌐 Pool web server starting on http://{}", addr);

        let pool = self.pool.clone();

        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let pool = pool.clone();

            tokio::task::spawn(async move {
                let service = service_fn(move |req| {
                    let pool = pool.clone();
                    async move { handle_request(req, pool).await }
                });

                if let Err(err) = http1::Builder::new()
                    .serve_connection(io, service)
                    .await
                {
                    error!("Error serving connection: {:?}", err);
                }
            });
        }
    }
}

async fn handle_request(
    req: Request<Incoming>,
    pool: Arc<Mutex<Pool>>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => serve_connections_page(pool).await,
        (&Method::GET, "/favicon.ico") | (&Method::GET, "/favicon.svg") => serve_favicon(),
        (&Method::GET, "/api/connections") => serve_connections_json(pool).await,
        _ => {
            let mut response = Response::new(Full::new(Bytes::from("Not Found")));
            *response.status_mut() = StatusCode::NOT_FOUND;
            response
        }
    };

    Ok(response)
}

async fn serve_connections_json(pool: Arc<Mutex<Pool>>) -> Response<Full<Bytes>> {
    let pool_stats = get_pool_stats(pool).await;
    let json = serde_json::to_string(&pool_stats).unwrap_or_else(|_| "{}".to_string());
    
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(json)))
        .unwrap()
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

async fn get_pool_stats(pool: Arc<Mutex<Pool>>) -> PoolStats {
    let connections = match pool.safe_lock(|p| p.get_connections_info()) {
        Ok(conns) => conns,
        Err(_) => Vec::new(),
    };
    let total_connections = connections.len();
    let total_shares: u64 = connections.iter().map(|c| c.shares_submitted).sum();
    let total_quotes: u64 = connections.iter().map(|c| c.quotes_created.unwrap_or(0)).sum();
    let ehash_mined: u64 = connections.iter().map(|c| c.ehash_mined.unwrap_or(0)).sum();
    
    // TODO: Implement mint stats request over SV2 message layer to get issued quotes count
    // This requires adding a new SV2 message type to request statistics from the mint
    // For now, quotes_redeemed is unavailable until proper inter-service communication is implemented
    let quotes_redeemed = None;
    
    PoolStats {
        total_connections,
        total_shares,
        total_quotes,
        quotes_redeemed,
        ehash_mined: format_hash_units(ehash_mined),
        ehash_mined_raw: ehash_mined,
        connections,
    }
}

async fn serve_connections_page(_pool: Arc<Mutex<Pool>>) -> Response<Full<Bytes>> {
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
                const response = await fetch('/api/connections');
                const data = await response.json();
                
                // Separate services from miners
                const services = data.connections.filter(conn => isServiceConnection(conn.connection_type));
                const miners = data.connections.filter(conn => !isServiceConnection(conn.connection_type));
                
                document.getElementById('total-miners').textContent = miners.length;
                document.getElementById('total-shares').textContent = data.total_shares.toLocaleString();
                document.getElementById('quotes-redeemed').textContent = data.quotes_redeemed === null ? '?' : data.quotes_redeemed.toLocaleString();
                document.getElementById('ehash-mined').textContent = data.ehash_mined;
                
                // Update services table
                const servicesTbody = document.getElementById('services-tbody');
                servicesTbody.innerHTML = '';
                
                if (services.length === 0) {
                    servicesTbody.innerHTML = '<tr><td colspan="6" style="text-align: center; opacity: 0.5;">No service connections</td></tr>';
                } else {
                    services.forEach(conn => {
                        const row = servicesTbody.insertRow();
                        const serviceMeta = getServiceMetadata(conn.connection_type);
                        const disconnected = isDisconnected(conn.connection_type);

                        const iconCell = row.insertCell();
                        iconCell.className = 'service-icon-cell';
                        iconCell.innerHTML = serviceMeta.iconClass
                            ? `<span class="${serviceMeta.iconClass}" aria-hidden="true"></span>`
                            : '';

                        if (disconnected) {
                            // Service is disconnected - show dashes and down status
                            row.insertCell().textContent = serviceMeta.label;
                            row.insertCell().textContent = '-';
                            row.insertCell().innerHTML = `<span class="address">-</span>`;
                            row.insertCell().textContent = '-';
                            row.insertCell().innerHTML = `<span class="status-dot status-down"></span><span style="color: #ff4444;">Down</span>`;
                        } else {
                            // Service is connected - show normal info
                            const addr = parseAddress(conn.address);
                            const channelId = serviceMeta.label === 'Pool'
                                ? '-' : (conn.channels.length > 0 ? conn.channels[0] : conn.id);
                            const isUp = conn.connection_type.includes('Mint') || conn.connection_type.includes('Job Declarator') || conn.shares_submitted > 0 || conn.channels.length > 0;
                            
                            row.insertCell().textContent = serviceMeta.label;
                            row.insertCell().textContent = channelId;
                            row.insertCell().innerHTML = `<span class="address">${addr.ip}</span>`;
                            row.insertCell().textContent = addr.port;
                            const poolServiceUp = serviceMeta.label === 'Pool';
                            const serviceUp = poolServiceUp || isUp;
                            row.insertCell().innerHTML = `<span class="status-dot ${serviceUp ? 'status-up' : 'status-down'}"></span>${serviceUp ? 'Up' : '<span style="color: #ff4444;">Down</span>'}`;
                        }
                    });
                }
                
                // Update miners table
                const minersTbody = document.getElementById('miners-tbody');
                minersTbody.innerHTML = '';
                
                if (miners.length === 0) {
                    minersTbody.innerHTML = '<tr><td colspan="7" style="text-align: center; opacity: 0.5;">No miners connected</td></tr>';
                } else {
                    miners.forEach(conn => {
                        const row = minersTbody.insertRow();
                        const iconCell = row.insertCell();
                        iconCell.className = 'miners-icon-cell';
                        iconCell.innerHTML = '<span class="miner-icon" aria-hidden="true"></span>';

                        row.insertCell().textContent = conn.id;
                        row.insertCell().innerHTML = `<span class="address">${conn.address}</span>`;
                        row.insertCell().textContent = conn.connection_type;
                        row.insertCell().textContent = conn.channels.length > 0 ? conn.channels.join(', ') : 'None';
                        row.insertCell().textContent = conn.shares_submitted.toLocaleString();
                        row.insertCell().textContent = conn.last_share_time || 'Never';
                    });
                }
                
                document.getElementById('refresh-time').textContent = 
                    'Updated: ' + new Date().toLocaleTimeString();
            } catch (error) {
                console.error('Failed to fetch connections:', error);
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
