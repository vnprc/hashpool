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
