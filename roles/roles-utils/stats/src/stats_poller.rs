use crate::stats_adapter::StatsSnapshotProvider;
use crate::stats_client::StatsClient;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error};

/// Generic polling loop that works with any StatsSnapshotProvider
/// Polls every 5 seconds and sends snapshots to the stats service
pub async fn start_stats_polling<T>(
    provider: Arc<Mutex<T>>,
    client: StatsClient<T::Snapshot>,
) where
    T: StatsSnapshotProvider + Send + 'static,
    T::Snapshot: Send + 'static,
{
    let mut interval = tokio::time::interval(Duration::from_secs(5));

    loop {
        interval.tick().await;

        // Get snapshot via trait - no SRI coupling here
        let snapshot = {
            let guard = provider.lock().await;
            guard.get_snapshot()
        };

        debug!("Collected stats snapshot, sending to stats service");

        // Send to stats service
        if let Err(e) = client.send_snapshot(snapshot).await {
            error!("Failed to send stats snapshot: {}", e);
            // Continue polling even if send fails
        }
    }
}
