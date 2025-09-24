use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing::{error, info};

use super::mining_pool::Pool;
use roles_logic_sv2::utils::Mutex;

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
                    connection_type: "Mint Service".to_string(),
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
                connection_type: "Mint Service (Disconnected)".to_string(),
            });
        }
        
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
    pub ehash_mined: u64,
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
) -> Result<Response<Full<bytes::Bytes>>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => serve_connections_page(pool).await,
        (&Method::GET, "/api/connections") => serve_connections_json(pool).await,
        _ => {
            let mut response = Response::new(Full::new(bytes::Bytes::from("Not Found")));
            *response.status_mut() = StatusCode::NOT_FOUND;
            response
        }
    };

    Ok(response)
}

async fn serve_connections_json(pool: Arc<Mutex<Pool>>) -> Response<Full<bytes::Bytes>> {
    let pool_stats = get_pool_stats(pool).await;
    let json = serde_json::to_string(&pool_stats).unwrap_or_else(|_| "{}".to_string());
    
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Full::new(bytes::Bytes::from(json)))
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
        ehash_mined,
        connections,
    }
}

async fn serve_connections_page(_pool: Arc<Mutex<Pool>>) -> Response<Full<bytes::Bytes>> {
    let html = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Mining Pool Dashboard</title>
    <link rel="icon" href="data:image/svg+xml,<svg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 100 100%22><text y=%22.9em%22 font-size=%2290%22>#️⃣</text></svg>">
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
    </style>
</head>
<body>
    <div class="container">
        <h1>Hashpool Dashboard</h1>
        
        <div class="services-section">
            <h3>🔧 Service Connections</h3>
            <table class="service-table">
                <thead>
                    <tr>
                        <th>Channel ID</th>
                        <th>Service Name</th>
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
        
        <h2>⛏️ Connected Miners</h2>
        <table>
            <thead>
                <tr>
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

        function getServiceName(connType) {
            if (connType.includes('Job Declarator')) return 'Job Declarator';
            if (connType.includes('Mint')) return 'Mint Service';
            return connType;
        }

        function isServiceConnection(connType) {
            return connType.includes('Job Declarator') || connType.includes('Mint');
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
                document.getElementById('ehash-mined').textContent = data.ehash_mined.toLocaleString();
                
                // Update services table
                const servicesTbody = document.getElementById('services-tbody');
                servicesTbody.innerHTML = '';
                
                if (services.length === 0) {
                    servicesTbody.innerHTML = '<tr><td colspan="5" style="text-align: center; opacity: 0.5;">No service connections</td></tr>';
                } else {
                    services.forEach(conn => {
                        const row = servicesTbody.insertRow();
                        const serviceName = getServiceName(conn.connection_type);
                        const disconnected = isDisconnected(conn.connection_type);
                        
                        if (disconnected) {
                            // Service is disconnected - show dashes and down status
                            row.insertCell(0).textContent = '-';
                            row.insertCell(1).textContent = serviceName;
                            row.insertCell(2).innerHTML = `<span class="address">-</span>`;
                            row.insertCell(3).textContent = '-';
                            row.insertCell(4).innerHTML = `<span class="status-dot status-down"></span><span style="color: #ff4444;">Down</span>`;
                        } else {
                            // Service is connected - show normal info
                            const addr = parseAddress(conn.address);
                            const channelId = conn.channels.length > 0 ? conn.channels[0] : conn.id;
                            const isUp = conn.connection_type.includes('Mint') || conn.connection_type.includes('Job Declarator') || conn.shares_submitted > 0 || conn.channels.length > 0;
                            
                            row.insertCell(0).textContent = channelId;
                            row.insertCell(1).textContent = serviceName;
                            row.insertCell(2).innerHTML = `<span class="address">${addr.ip}</span>`;
                            row.insertCell(3).textContent = addr.port;
                            row.insertCell(4).innerHTML = `<span class="status-dot ${isUp ? 'status-up' : 'status-down'}"></span>${isUp ? 'Up' : '<span style="color: #ff4444;">Down</span>'}`;
                        }
                    });
                }
                
                // Update miners table
                const minersTbody = document.getElementById('miners-tbody');
                minersTbody.innerHTML = '';
                
                if (miners.length === 0) {
                    minersTbody.innerHTML = '<tr><td colspan="6" style="text-align: center; opacity: 0.5;">No miners connected</td></tr>';
                } else {
                    miners.forEach(conn => {
                        const row = minersTbody.insertRow();
                        row.insertCell(0).textContent = conn.id;
                        row.insertCell(1).innerHTML = `<span class="address">${conn.address}</span>`;
                        row.insertCell(2).textContent = conn.connection_type;
                        row.insertCell(3).textContent = conn.channels.length > 0 ? conn.channels.join(', ') : 'None';
                        row.insertCell(4).textContent = conn.shares_submitted.toLocaleString();
                        row.insertCell(5).textContent = conn.last_share_time || 'Never';
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

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Full::new(bytes::Bytes::from(html)))
        .unwrap()
}