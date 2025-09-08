use tokio::time::Instant;
use mining_sv2::SubmitSharesExtended;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct PendingShare {
    pub channel_id: u32,
    pub sequence_number: u32,
    pub share_hash: Vec<u8>,
    pub share_data: SubmitSharesExtended<'static>,
    pub created_at: Instant,
}

#[derive(Debug)]
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
        tracing::debug!("Adding pending share: hash={}, channel_id={}", 
                       hex::encode(&share.share_hash), share.channel_id);
        pending.insert(share.share_hash.clone(), share);
    }
    
    pub async fn remove_pending_share(&self, hash: &[u8]) -> Option<PendingShare> {
        let mut pending = self.pending.lock().await;
        let result = pending.remove(hash);
        if result.is_some() {
            tracing::debug!("Removed pending share: hash={}", hex::encode(hash));
        }
        result
    }
    
    pub async fn get_stale_shares(&self, timeout: Duration) -> Vec<PendingShare> {
        let mut pending = self.pending.lock().await;
        let now = Instant::now();
        
        let stale_hashes: Vec<Vec<u8>> = pending
            .iter()
            .filter(|(_, share)| now.duration_since(share.created_at) > timeout)
            .map(|(hash, _)| hash.clone())
            .collect();
            
        let stale_shares: Vec<PendingShare> = stale_hashes
            .into_iter()
            .filter_map(|hash| pending.remove(&hash))
            .collect();
            
        if !stale_shares.is_empty() {
            tracing::warn!("Found {} stale shares", stale_shares.len());
        }
        
        stale_shares
    }
    
    pub async fn get_pending_count(&self) -> usize {
        let pending = self.pending.lock().await;
        pending.len()
    }
    
    #[cfg(test)]
    pub async fn get_all_pending(&self) -> HashMap<Vec<u8>, PendingShare> {
        let pending = self.pending.lock().await;
        pending.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mining_sv2::Extranonce;
    
    fn create_test_share(channel_id: u32, sequence_number: u32, hash_suffix: u8) -> PendingShare {
        let mut hash_bytes = [0u8; 32];
        hash_bytes[31] = hash_suffix; // Make each hash unique
        
        let share_data = SubmitSharesExtended {
            channel_id,
            sequence_number,
            job_id: 1,
            nonce: 12345,
            ntime: 1234567890,
            version: 0x20000000,
            extranonce: Extranonce::new(32).unwrap().into_b032(),
            hash: hash_bytes.into(),
            locking_pubkey: [2u8; 33].into(),
        };
        
        PendingShare {
            channel_id,
            sequence_number,
            share_hash: hash_bytes.to_vec(),
            share_data,
            created_at: Instant::now(),
        }
    }
    
    #[tokio::test]
    async fn test_add_and_remove_pending_share() {
        let manager = PendingShareManager::new();
        let share = create_test_share(1, 100, 1);
        let hash = share.share_hash.clone();
        
        // Add share
        manager.add_pending_share(share).await;
        assert_eq!(manager.get_pending_count().await, 1);
        
        // Remove share
        let removed = manager.remove_pending_share(&hash).await;
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().channel_id, 1);
        assert_eq!(manager.get_pending_count().await, 0);
        
        // Try to remove non-existent share
        let not_found = manager.remove_pending_share(&hash).await;
        assert!(not_found.is_none());
    }
    
    #[tokio::test]
    async fn test_stale_shares_detection() {
        let manager = PendingShareManager::new();
        
        // Add a fresh share
        let fresh_share = create_test_share(1, 100, 1);
        manager.add_pending_share(fresh_share).await;
        
        // Add a stale share by creating one in the past
        let mut stale_share = create_test_share(2, 200, 2);
        stale_share.created_at = Instant::now() - Duration::from_secs(20);
        manager.add_pending_share(stale_share).await;
        
        assert_eq!(manager.get_pending_count().await, 2);
        
        // Get stale shares with 10 second timeout
        let stale = manager.get_stale_shares(Duration::from_secs(10)).await;
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].channel_id, 2);
        
        // Should have 1 share left (the fresh one)
        assert_eq!(manager.get_pending_count().await, 1);
    }
    
    #[tokio::test]
    async fn test_multiple_shares_same_channel() {
        let manager = PendingShareManager::new();
        
        // Add multiple shares from same channel
        let share1 = create_test_share(1, 100, 1);
        let share2 = create_test_share(1, 101, 2);
        let share3 = create_test_share(2, 200, 3);
        
        manager.add_pending_share(share1).await;
        manager.add_pending_share(share2).await;
        manager.add_pending_share(share3).await;
        
        assert_eq!(manager.get_pending_count().await, 3);
        
        let all_pending = manager.get_all_pending().await;
        let channel1_shares: Vec<_> = all_pending.values()
            .filter(|share| share.channel_id == 1)
            .collect();
        assert_eq!(channel1_shares.len(), 2);
    }
}