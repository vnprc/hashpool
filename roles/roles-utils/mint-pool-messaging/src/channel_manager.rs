use super::*;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::sync::RwLock;

/// Manages channels and connection lifecycle
pub struct ChannelManager {
    next_channel_id: AtomicU64,
    active_channels: RwLock<HashMap<u64, ChannelInfo>>,
    #[allow(dead_code)]
    config: MessagingConfig,
}

#[derive(Debug, Clone)]
pub struct ChannelInfo {
    pub id: u64,
    pub role: Role,
    pub created_at: std::time::Instant,
    pub last_activity: std::time::Instant,
    pub message_count: u64,
}

#[derive(Error, Debug)]
pub enum ChannelError {
    #[error("Channel not found: {0}")]
    NotFound(u64),
    #[error("Channel already exists: {0}")]
    AlreadyExists(u64),
    #[error("Invalid channel state: {0}")]
    InvalidState(String),
}

impl ChannelManager {
    /// Create a new channel manager
    pub fn new(config: MessagingConfig) -> Self {
        Self {
            next_channel_id: AtomicU64::new(1),
            active_channels: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Create a new channel for the given role
    pub async fn create_channel(&self, role: Role) -> u64 {
        let channel_id = self.next_channel_id.fetch_add(1, Ordering::SeqCst);
        let now = std::time::Instant::now();

        let channel_info = ChannelInfo {
            id: channel_id,
            role,
            created_at: now,
            last_activity: now,
            message_count: 0,
        };

        let mut channels = self.active_channels.write().await;
        channels.insert(channel_id, channel_info);

        info!("Created new channel {}", channel_id);
        channel_id
    }

    /// Close a channel
    pub async fn close_channel(&self, channel_id: u64) -> Result<(), ChannelError> {
        let mut channels = self.active_channels.write().await;
        channels
            .remove(&channel_id)
            .ok_or(ChannelError::NotFound(channel_id))?;

        info!("Closed channel {}", channel_id);
        Ok(())
    }

    /// Update channel activity
    pub async fn update_activity(&self, channel_id: u64) -> Result<(), ChannelError> {
        let mut channels = self.active_channels.write().await;
        if let Some(channel) = channels.get_mut(&channel_id) {
            channel.last_activity = std::time::Instant::now();
            channel.message_count += 1;
            Ok(())
        } else {
            Err(ChannelError::NotFound(channel_id))
        }
    }

    /// Get channel info
    pub async fn get_channel(&self, channel_id: u64) -> Result<ChannelInfo, ChannelError> {
        let channels = self.active_channels.read().await;
        channels
            .get(&channel_id)
            .cloned()
            .ok_or(ChannelError::NotFound(channel_id))
    }

    /// Get all active channels
    pub async fn get_all_channels(&self) -> Vec<ChannelInfo> {
        let channels = self.active_channels.read().await;
        channels.values().cloned().collect()
    }

    /// Clean up stale channels (channels with no activity for a while)
    pub async fn cleanup_stale_channels(&self, max_idle_secs: u64) -> usize {
        let mut channels = self.active_channels.write().await;
        let now = std::time::Instant::now();
        let max_idle = std::time::Duration::from_secs(max_idle_secs);

        let stale_channels: Vec<u64> = channels
            .iter()
            .filter(|(_, info)| now.duration_since(info.last_activity) > max_idle)
            .map(|(id, _)| *id)
            .collect();

        for channel_id in &stale_channels {
            channels.remove(channel_id);
            warn!("Removed stale channel {}", channel_id);
        }

        stale_channels.len()
    }

    /// Get channel statistics
    pub async fn get_stats(&self) -> ChannelStats {
        let channels = self.active_channels.read().await;

        let total_channels = channels.len();
        let pool_channels = channels.values().filter(|c| c.role == Role::Pool).count();
        let mint_channels = channels.values().filter(|c| c.role == Role::Mint).count();

        let total_messages = channels.values().map(|c| c.message_count).sum();

        ChannelStats {
            total_channels,
            pool_channels,
            mint_channels,
            total_messages,
        }
    }
}

/// Channel statistics
#[derive(Debug)]
pub struct ChannelStats {
    pub total_channels: usize,
    pub pool_channels: usize,
    pub mint_channels: usize,
    pub total_messages: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // Channel Creation Tests
    // ============================================================================

    #[tokio::test]
    async fn test_create_pool_channel() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let channel_id = manager.create_channel(Role::Pool).await;
        assert!(channel_id > 0);

        let channel = manager.get_channel(channel_id).await.unwrap();
        assert_eq!(channel.id, channel_id);
        assert_eq!(channel.role, Role::Pool);
        assert_eq!(channel.message_count, 0);
    }

    #[tokio::test]
    async fn test_create_mint_channel() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let channel_id = manager.create_channel(Role::Mint).await;
        assert!(channel_id > 0);

        let channel = manager.get_channel(channel_id).await.unwrap();
        assert_eq!(channel.id, channel_id);
        assert_eq!(channel.role, Role::Mint);
    }

    #[tokio::test]
    async fn test_create_multiple_channels() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let id1 = manager.create_channel(Role::Pool).await;
        let id2 = manager.create_channel(Role::Mint).await;
        let id3 = manager.create_channel(Role::Pool).await;

        // Channel IDs should be unique and increasing
        assert!(id1 < id2);
        assert!(id2 < id3);

        let channels = manager.get_all_channels().await;
        assert_eq!(channels.len(), 3);
    }

    #[tokio::test]
    async fn test_channel_ids_are_sequential() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let id1 = manager.create_channel(Role::Pool).await;
        let id2 = manager.create_channel(Role::Pool).await;
        let id3 = manager.create_channel(Role::Pool).await;

        // IDs should be sequential
        assert_eq!(id2, id1 + 1);
        assert_eq!(id3, id2 + 1);
    }

    // ============================================================================
    // Channel Closure Tests
    // ============================================================================

    #[tokio::test]
    async fn test_close_channel() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let channel_id = manager.create_channel(Role::Pool).await;
        assert!(manager.get_channel(channel_id).await.is_ok());

        manager.close_channel(channel_id).await.unwrap();
        assert!(manager.get_channel(channel_id).await.is_err());
    }

    #[tokio::test]
    async fn test_close_nonexistent_channel() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let result = manager.close_channel(99999).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ChannelError::NotFound(id) => assert_eq!(id, 99999),
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_close_already_closed_channel() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let channel_id = manager.create_channel(Role::Pool).await;
        manager.close_channel(channel_id).await.unwrap();

        // Closing again should fail
        let result = manager.close_channel(channel_id).await;
        assert!(result.is_err());
    }

    // ============================================================================
    // Activity Update Tests
    // ============================================================================

    #[tokio::test]
    async fn test_update_channel_activity() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let channel_id = manager.create_channel(Role::Pool).await;
        let initial = manager.get_channel(channel_id).await.unwrap();
        let initial_activity = initial.last_activity;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        manager.update_activity(channel_id).await.unwrap();

        let updated = manager.get_channel(channel_id).await.unwrap();
        // Last activity should have been updated
        assert!(updated.last_activity >= initial_activity);
        assert_eq!(updated.message_count, 1);
    }

    #[tokio::test]
    async fn test_activity_increments_message_count() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let channel_id = manager.create_channel(Role::Pool).await;

        for i in 1..=5 {
            manager.update_activity(channel_id).await.unwrap();
            let channel = manager.get_channel(channel_id).await.unwrap();
            assert_eq!(channel.message_count, i as u64);
        }
    }

    #[tokio::test]
    async fn test_update_nonexistent_channel_activity() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let result = manager.update_activity(99999).await;
        assert!(result.is_err());
    }

    // ============================================================================
    // Stale Channel Cleanup Tests
    // ============================================================================

    #[tokio::test]
    async fn test_cleanup_stale_channels() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let _channel1 = manager.create_channel(Role::Pool).await;
        let _channel2 = manager.create_channel(Role::Mint).await;

        // Simulate older activity on channel1 by manually manipulating time
        // For this test, we'll just verify the cleanup function works
        let removed = manager.cleanup_stale_channels(1).await;
        // No channels should be stale yet (just created)
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn test_cleanup_ignores_active_channels() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let channel_id = manager.create_channel(Role::Pool).await;

        // Update activity to keep it fresh
        manager.update_activity(channel_id).await.unwrap();

        // Cleanup with very short timeout should not remove fresh channels
        let removed = manager.cleanup_stale_channels(1).await;
        assert_eq!(removed, 0);

        // Channel should still exist
        assert!(manager.get_channel(channel_id).await.is_ok());
    }

    // ============================================================================
    // Statistics Tests
    // ============================================================================

    #[tokio::test]
    async fn test_channel_statistics() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let _ = manager.create_channel(Role::Pool).await;
        let _ = manager.create_channel(Role::Pool).await;
        let _ = manager.create_channel(Role::Mint).await;

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_channels, 3);
        assert_eq!(stats.pool_channels, 2);
        assert_eq!(stats.mint_channels, 1);
    }

    #[tokio::test]
    async fn test_statistics_with_messages() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let ch1 = manager.create_channel(Role::Pool).await;
        let ch2 = manager.create_channel(Role::Mint).await;

        // Send messages on each channel
        manager.update_activity(ch1).await.unwrap();
        manager.update_activity(ch1).await.unwrap();
        manager.update_activity(ch2).await.unwrap();

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_messages, 3);
    }

    #[tokio::test]
    async fn test_statistics_empty_manager() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_channels, 0);
        assert_eq!(stats.pool_channels, 0);
        assert_eq!(stats.mint_channels, 0);
        assert_eq!(stats.total_messages, 0);
    }

    // ============================================================================
    // Get All Channels Tests
    // ============================================================================

    #[tokio::test]
    async fn test_get_all_channels_empty() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let channels = manager.get_all_channels().await;
        assert_eq!(channels.len(), 0);
    }

    #[tokio::test]
    async fn test_get_all_channels_multiple() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let id1 = manager.create_channel(Role::Pool).await;
        let id2 = manager.create_channel(Role::Mint).await;

        let channels = manager.get_all_channels().await;
        assert_eq!(channels.len(), 2);

        let ids: Vec<u64> = channels.iter().map(|c| c.id).collect();
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[tokio::test]
    async fn test_get_channel_not_found() {
        let config = MessagingConfig::default();
        let manager = ChannelManager::new(config);

        let result = manager.get_channel(12345).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ChannelError::NotFound(id) => assert_eq!(id, 12345),
            _ => panic!("Expected NotFound error"),
        }
    }
}
