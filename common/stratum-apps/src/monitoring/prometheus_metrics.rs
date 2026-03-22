//! Prometheus metrics definitions for SV2 monitoring

use prometheus::{Gauge, GaugeVec, Opts, Registry};

/// Prometheus metrics for the monitoring server.
/// Metrics are optional - only registered when the corresponding monitoring type is enabled.
#[derive(Clone)]
pub struct PrometheusMetrics {
    pub registry: Registry,
    // System metrics
    pub sv2_uptime_seconds: Gauge,
    // Server metrics (upstream connection)
    pub sv2_server_channels: Option<GaugeVec>,
    pub sv2_server_hashrate_total: Option<Gauge>,
    pub sv2_server_channel_hashrate: Option<GaugeVec>,
    pub sv2_server_shares_accepted_total: Option<GaugeVec>,
    pub sv2_server_blocks_found_total: Option<Gauge>,
    // Clients metrics (downstream connections)
    pub sv2_clients_total: Option<Gauge>,
    pub sv2_client_channels: Option<GaugeVec>,
    pub sv2_client_hashrate_total: Option<Gauge>,
    pub sv2_client_channel_hashrate: Option<GaugeVec>,
    pub sv2_client_shares_accepted_total: Option<GaugeVec>,
    pub sv2_client_blocks_found_total: Option<Gauge>,
    // SV1 metrics
    pub sv1_clients_total: Option<Gauge>,
    pub sv1_hashrate_total: Option<Gauge>,
}

impl PrometheusMetrics {
    pub fn new(
        enable_server_metrics: bool,
        enable_clients_metrics: bool,
        enable_sv1_metrics: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let registry = Registry::new();

        // System metrics (always enabled)
        let sv2_uptime_seconds = Gauge::new("sv2_uptime_seconds", "Server uptime in seconds")?;
        registry.register(Box::new(sv2_uptime_seconds.clone()))?;

        // Server metrics (upstream connection)
        let (
            sv2_server_channels,
            sv2_server_hashrate_total,
            sv2_server_channel_hashrate,
            sv2_server_shares_accepted_total,
            sv2_server_blocks_found_total,
        ) = if enable_server_metrics {
            let channels = GaugeVec::new(
                Opts::new("sv2_server_channels", "Number of server channels by type"),
                &["channel_type"],
            )?;
            registry.register(Box::new(channels.clone()))?;

            let hashrate = Gauge::new(
                "sv2_server_hashrate_total",
                "Total hashrate for channels opened with the server",
            )?;
            registry.register(Box::new(hashrate.clone()))?;

            let channel_hashrate = GaugeVec::new(
                Opts::new(
                    "sv2_server_channel_hashrate",
                    "Hashrate for individual server channels",
                ),
                &["channel_id", "user_identity"],
            )?;
            registry.register(Box::new(channel_hashrate.clone()))?;

            let shares_accepted = GaugeVec::new(
                Opts::new(
                    "sv2_server_shares_accepted_total",
                    "Total shares accepted per server channel",
                ),
                &["channel_id", "user_identity"],
            )?;
            registry.register(Box::new(shares_accepted.clone()))?;

            let blocks_found = Gauge::new(
                "sv2_server_blocks_found_total",
                "Total blocks found across all current server channels",
            )?;
            registry.register(Box::new(blocks_found.clone()))?;

            (
                Some(channels),
                Some(hashrate),
                Some(channel_hashrate),
                Some(shares_accepted),
                Some(blocks_found),
            )
        } else {
            (None, None, None, None, None)
        };

        // Clients metrics (downstream connections)
        let (
            sv2_clients_total,
            sv2_client_channels,
            sv2_client_hashrate_total,
            sv2_client_channel_hashrate,
            sv2_client_shares_accepted_total,
            sv2_client_blocks_found_total,
        ) = if enable_clients_metrics {
            let clients_total =
                Gauge::new("sv2_clients_total", "Total number of connected clients")?;
            registry.register(Box::new(clients_total.clone()))?;

            let channels = GaugeVec::new(
                Opts::new("sv2_client_channels", "Number of client channels by type"),
                &["channel_type"],
            )?;
            registry.register(Box::new(channels.clone()))?;

            let hashrate = Gauge::new(
                "sv2_client_hashrate_total",
                "Total hashrate for channels opened with clients",
            )?;
            registry.register(Box::new(hashrate.clone()))?;

            let channel_hashrate = GaugeVec::new(
                Opts::new(
                    "sv2_client_channel_hashrate",
                    "Hashrate for individual client channels",
                ),
                &["client_id", "channel_id", "user_identity"],
            )?;
            registry.register(Box::new(channel_hashrate.clone()))?;

            let shares_accepted = GaugeVec::new(
                Opts::new(
                    "sv2_client_shares_accepted_total",
                    "Total shares accepted per client channel",
                ),
                &["client_id", "channel_id", "user_identity"],
            )?;
            registry.register(Box::new(shares_accepted.clone()))?;

            let blocks_found = Gauge::new(
                "sv2_client_blocks_found_total",
                "Total blocks found across all current client channels",
            )?;
            registry.register(Box::new(blocks_found.clone()))?;

            (
                Some(clients_total),
                Some(channels),
                Some(hashrate),
                Some(channel_hashrate),
                Some(shares_accepted),
                Some(blocks_found),
            )
        } else {
            (None, None, None, None, None, None)
        };

        // SV1 metrics
        let (sv1_clients_total, sv1_hashrate_total) = if enable_sv1_metrics {
            let clients = Gauge::new("sv1_clients_total", "Total number of SV1 clients")?;
            registry.register(Box::new(clients.clone()))?;

            let hashrate = Gauge::new("sv1_hashrate_total", "Total hashrate from SV1 clients")?;
            registry.register(Box::new(hashrate.clone()))?;

            (Some(clients), Some(hashrate))
        } else {
            (None, None)
        };

        Ok(Self {
            registry,
            sv2_uptime_seconds,
            sv2_server_channels,
            sv2_server_hashrate_total,
            sv2_server_channel_hashrate,
            sv2_server_shares_accepted_total,
            sv2_server_blocks_found_total,
            sv2_clients_total,
            sv2_client_channels,
            sv2_client_hashrate_total,
            sv2_client_channel_hashrate,
            sv2_client_shares_accepted_total,
            sv2_client_blocks_found_total,
            sv1_clients_total,
            sv1_hashrate_total,
        })
    }
}
