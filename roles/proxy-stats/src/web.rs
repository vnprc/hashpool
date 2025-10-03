use std::convert::Infallible;
use std::sync::Arc;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use tokio::net::TcpListener;
use tracing::{error, info};
use bytes::Bytes;

use proxy_stats::db::StatsDatabase;
use web_assets::icons::{nav_icon_css, pickaxe_favicon_inline_svg};

pub async fn run_http_server(
    address: String,
    db: Arc<StatsDatabase>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&address).await?;
    info!("HTTP dashboard listening on http://{}", address);

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
        (&Method::GET, "/") => serve_dashboard().await,
        (&Method::GET, "/favicon.ico") | (&Method::GET, "/favicon.svg") => serve_favicon(),
        (&Method::GET, "/api/stats") => serve_stats_json(db).await,
        (&Method::GET, path) if path.starts_with("/api/hashrate") => {
            serve_hashrate_json(req, db).await
        }
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

async fn serve_stats_json(db: Arc<StatsDatabase>) -> Response<Full<Bytes>> {
    match db.get_current_stats() {
        Ok(stats) => {
            let json = serde_json::to_string(&stats).unwrap_or_else(|_| "[]".to_string());
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(json)))
                .unwrap()
        }
        Err(e) => {
            error!("Error getting stats: {}", e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from("Internal server error")))
                .unwrap()
        }
    }
}

async fn serve_hashrate_json(req: Request<Incoming>, db: Arc<StatsDatabase>) -> Response<Full<Bytes>> {
    // Parse query parameter for hours
    let hours = req
        .uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("hours="))
                .and_then(|p| p.strip_prefix("hours="))
                .and_then(|h| h.parse::<i64>().ok())
        })
        .unwrap_or(24);

    match db.get_hashrate_history(hours) {
        Ok(points) => {
            let json = serde_json::to_string(&points).unwrap_or_else(|_| "[]".to_string());
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(json)))
                .unwrap()
        }
        Err(e) => {
            error!("Error getting hashrate history: {}", e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from("Internal server error")))
                .unwrap()
        }
    }
}

async fn serve_dashboard() -> Response<Full<Bytes>> {
    let nav_icon_css_content = nav_icon_css();
    let html = format!(r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Hashpool Pool Dashboard</title>
    <link rel="icon" type="image/svg+xml" sizes="any" href="/favicon.svg">
    <style>
        body {{
            font-family: 'Courier New', monospace;
            background: #1a1a1a;
            color: #00ff00;
            margin: 0;
            padding: 20px;
        }}
        .container {{
            max-width: 1200px;
            margin: 0 auto;
        }}
        h1 {{
            text-align: center;
            margin-bottom: 30px;
        }}
        .stats {{
            display: flex;
            justify-content: space-around;
            margin-bottom: 40px;
        }}
        .stat-box {{
            text-align: center;
            padding: 20px;
            border: 1px solid #00ff00;
            min-width: 150px;
        }}
        .stat-value {{
            font-size: 2em;
            margin-top: 10px;
        }}
        table {{
            width: 100%;
            border-collapse: collapse;
        }}
        th, td {{
            padding: 12px;
            text-align: left;
            border-bottom: 1px solid #00ff00;
        }}
        th {{
            background: #0a0a0a;
            font-weight: bold;
        }}
        tr:hover {{
            background: #0a0a0a;
        }}
        .refresh {{
            text-align: right;
            margin-bottom: 10px;
            font-size: 0.9em;
            opacity: 0.7;
        }}
        {}
    </style>
</head>
<body>
    <div class="container">
        <h1 class="pickaxe-icon">Hashpool Pool Dashboard</h1>

        <div class="stats">
            <div class="stat-box">
                <div>Connected Downstreams</div>
                <div class="stat-value" id="total-downstreams">-</div>
            </div>
            <div class="stat-box">
                <div>Total Shares</div>
                <div class="stat-value" id="total-shares">-</div>
            </div>
            <div class="stat-box">
                <div>Total Quotes</div>
                <div class="stat-value" id="total-quotes">-</div>
            </div>
            <div class="stat-box">
                <div>Ehash Mined</div>
                <div class="stat-value" id="ehash-mined">-</div>
            </div>
        </div>

        <div class="refresh" id="refresh-time">Loading...</div>

        <h2>Downstream Connections</h2>
        <table>
            <thead>
                <tr>
                    <th>ID</th>
                    <th>Shares</th>
                    <th>Quotes</th>
                    <th>Ehash Mined</th>
                    <th>Channels</th>
                    <th>Last Share</th>
                </tr>
            </thead>
            <tbody id="downstreams-tbody">
            </tbody>
        </table>
    </div>

    <script>
        function updateDashboard() {{
            fetch('/api/stats')
                .then(response => response.json())
                .then(data => {{
                    // Update summary stats
                    document.getElementById('total-downstreams').textContent = data.length;

                    let totalShares = 0;
                    let totalQuotes = 0;
                    let totalEhash = 0;

                    data.forEach(downstream => {{
                        totalShares += downstream.shares_submitted;
                        totalQuotes += downstream.quotes_created;
                        totalEhash += downstream.ehash_mined;
                    }});

                    document.getElementById('total-shares').textContent = totalShares;
                    document.getElementById('total-quotes').textContent = totalQuotes;
                    document.getElementById('ehash-mined').textContent = totalEhash + ' ehash';

                    // Update table
                    const tbody = document.getElementById('downstreams-tbody');
                    tbody.innerHTML = '';

                    data.forEach(downstream => {{
                        const row = tbody.insertRow();
                        row.insertCell().textContent = downstream.downstream_id;
                        row.insertCell().textContent = downstream.shares_submitted;
                        row.insertCell().textContent = downstream.quotes_created;
                        row.insertCell().textContent = downstream.ehash_mined;
                        row.insertCell().textContent = downstream.channels.join(', ');

                        let lastShare = '-';
                        if (downstream.last_share_time) {{
                            const now = Math.floor(Date.now() / 1000);
                            const elapsed = now - downstream.last_share_time;
                            if (elapsed < 60) {{
                                lastShare = elapsed + 's ago';
                            }} else if (elapsed < 3600) {{
                                lastShare = Math.floor(elapsed / 60) + 'm ago';
                            }} else {{
                                lastShare = Math.floor(elapsed / 3600) + 'h ago';
                            }}
                        }}
                        row.insertCell().textContent = lastShare;
                    }});

                    document.getElementById('refresh-time').textContent = 'Last updated: ' + new Date().toLocaleTimeString();
                }})
                .catch(error => {{
                    console.error('Error fetching stats:', error);
                }});
        }}

        // Initial load
        updateDashboard();

        // Refresh every 5 seconds
        setInterval(updateDashboard, 5000);
    </script>
</body>
</html>"#, nav_icon_css_content);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html")
        .body(Full::new(Bytes::from(html)))
        .unwrap()
}
