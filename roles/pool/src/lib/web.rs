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
                    quotes_created: Some(*quotes), // Show quotes for mining connections
                    last_share_time: last_share,
                    connection_type,
                }
            });
            
            if let Ok(conn_info) = result {
                connections.push(conn_info);
            }
        }
        
        // Add mint connections
        for (address, _sender) in self.get_mint_connections() {
            connections.push(ConnectionInfo {
                id: 0, // Mint connections don't have downstream IDs
                address: address.to_string(),
                channels: vec![],
                shares_submitted: 0,
                quotes_created: None, // Don't show quotes for mint connections
                last_share_time: None,
                connection_type: "Mint Service".to_string(),
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
    pub quotes_created: Option<u64>, // Only show for mint connections
    pub last_share_time: Option<String>,
    pub connection_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStats {
    pub total_connections: usize,
    pub total_shares: u64,
    pub total_quotes: u64,
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
    
    PoolStats {
        total_connections,
        total_shares,
        total_quotes,
        connections,
    }
}

async fn serve_connections_page(_pool: Arc<Mutex<Pool>>) -> Response<Full<bytes::Bytes>> {
    let html = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Connections Dashboard</title>
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
            text-shadow: 0 0 10px #00ff00;
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
    </style>
</head>
<body>
    <div class="container">
        <h1>🔗 Pool Connections</h1>
        
        <div class="stats">
            <div class="stat-box">
                <div>Connections</div>
                <div class="stat-value" id="total-connections">-</div>
            </div>
            <div class="stat-box">
                <div>Total Shares</div>
                <div class="stat-value" id="total-shares">-</div>
            </div>
            <div class="stat-box">
                <div>Total Quotes</div>
                <div class="stat-value" id="total-quotes">-</div>
            </div>
        </div>

        <div class="refresh" id="refresh-time">Loading...</div>
        
        <table>
            <thead>
                <tr>
                    <th>ID</th>
                    <th>Address</th>
                    <th>Type</th>
                    <th>Channels</th>
                    <th>Shares</th>
                    <th>Quotes</th>
                    <th>Last Share</th>
                </tr>
            </thead>
            <tbody id="connections-tbody">
            </tbody>
        </table>
    </div>

    <script>
        async function updateConnections() {
            try {
                const response = await fetch('/api/connections');
                const data = await response.json();
                
                document.getElementById('total-connections').textContent = data.total_connections;
                document.getElementById('total-shares').textContent = data.total_shares.toLocaleString();
                document.getElementById('total-quotes').textContent = data.total_quotes.toLocaleString();
                
                const tbody = document.getElementById('connections-tbody');
                tbody.innerHTML = '';
                
                if (data.connections.length === 0) {
                    tbody.innerHTML = '<tr><td colspan="7" style="text-align: center; opacity: 0.5;">No active connections</td></tr>';
                } else {
                    data.connections.forEach(conn => {
                        const row = tbody.insertRow();
                        row.insertCell(0).textContent = conn.id;
                        row.insertCell(1).innerHTML = `<span class="address">${conn.address}</span>`;
                        row.insertCell(2).textContent = conn.connection_type;
                        row.insertCell(3).textContent = conn.channels.length > 0 ? conn.channels.join(', ') : 'None';
                        row.insertCell(4).textContent = conn.shares_submitted.toLocaleString();
                        row.insertCell(5).textContent = conn.quotes_created !== null ? conn.quotes_created.toLocaleString() : '-';
                        row.insertCell(6).textContent = conn.last_share_time || 'Never';
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