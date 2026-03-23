//! Monitoring system for SV2 applications.
//!
//! Provides HTTP JSON API and Prometheus metrics for monitoring.
//! Read-only - does not modify any state.
//!
//! ## Architecture
//!
//! - **Server**: The upstream connection (pool, JDS) - typically one per app
//! - **Clients**: Downstream connections (miners) - multiple per app
//! - **SV1 clients**: Legacy SV1 connections (Translator only)

pub mod client;
pub mod http_server;
pub mod prometheus_metrics;
pub mod server;
pub mod snapshot_cache;
pub mod sv1;

pub use client::{
    ExtendedChannelInfo, StandardChannelInfo, Sv2ClientInfo, Sv2ClientMetadata,
    Sv2ClientsMonitoring, Sv2ClientsSummary,
};
pub use http_server::MonitoringServer;
pub use server::{
    ServerExtendedChannelInfo, ServerInfo, ServerMonitoring, ServerStandardChannelInfo,
    ServerSummary,
};
pub use snapshot_cache::{MonitoringSnapshot, SnapshotCache};
pub use sv1::{Sv1ClientInfo, Sv1ClientsMonitoring, Sv1ClientsSummary};

use utoipa::ToSchema;

/// Global statistics from `/api/v1/global` endpoint
///
/// Fields are `Option` to distinguish "not monitored" (`None`) from "monitored but empty" (`Some`
/// with zeros).
///
/// Typical configurations:
/// - **Pool/JDC**: `server` and `sv2_clients` are `Some`, `sv1_clients` is `None`
/// - **tProxy**: `server` and `sv1_clients` are `Some`, `sv2_clients` is `None`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ToSchema)]
pub struct GlobalInfo {
    /// Server (upstream) summary - `None` if server monitoring is not enabled
    pub server: Option<ServerSummary>,
    /// Sv2 clients (downstream) summary - `None` if Sv2 client monitoring is not enabled (e.g.,
    /// tProxy)
    pub sv2_clients: Option<Sv2ClientsSummary>,
    /// Sv1 clients summary - `None` if Sv1 monitoring is not enabled (e.g., Pool/JDC)
    pub sv1_clients: Option<Sv1ClientsSummary>,
    /// Uptime in seconds since the application started
    pub uptime_secs: u64,
}
