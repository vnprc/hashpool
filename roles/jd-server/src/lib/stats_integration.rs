use super::job_declarator::JobDeclarator;
use stats::stats_adapter::{JdsSnapshot, StatsSnapshotProvider};
use std::time::SystemTime;

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

impl StatsSnapshotProvider for JobDeclarator {
    type Snapshot = JdsSnapshot;

    fn get_snapshot(&self) -> JdsSnapshot {
        JdsSnapshot {
            listen_address: String::new(), // Will be filled from config
            timestamp: unix_timestamp(),
        }
    }
}
