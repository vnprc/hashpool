use std::{collections::HashMap, sync::Arc};
use tokio::{
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct PendingShare {
    pub channel_id: u32,
    pub sequence_number: u32,
    pub share_hash: Vec<u8>,
    pub locking_pubkey: Vec<u8>,
    pub amount: u64,
    pub created_at: Instant,
}

pub struct PendingShareManager {
    pending: Arc<Mutex<HashMap<Vec<u8>, PendingShare>>>,
}

impl PendingShareManager {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn add_pending_share(&self, share: PendingShare) {
        let mut pending = self.pending.lock().await;
        pending.insert(share.share_hash.clone(), share);
    }

    pub async fn remove_pending_share(&self, hash: &[u8]) -> Option<PendingShare> {
        let mut pending = self.pending.lock().await;
        pending.remove(hash)
    }

    pub async fn get_stale_shares(&self, timeout: Duration) -> Vec<PendingShare> {
        let mut pending = self.pending.lock().await;
        let now = Instant::now();

        let stale: Vec<_> = pending
            .iter()
            .filter(|(_, share)| now.duration_since(share.created_at) > timeout)
            .map(|(hash, share)| (hash.clone(), share.clone()))
            .collect();

        for (hash, _) in &stale {
            pending.remove(hash);
        }

        stale.into_iter().map(|(_, share)| share).collect()
    }
}
