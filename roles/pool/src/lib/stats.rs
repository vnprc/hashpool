use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub enum StatsMessage {
    ShareSubmitted { downstream_id: u32 },
    QuoteCreated { downstream_id: u32, amount: u64 },
    ChannelAdded { downstream_id: u32, channel_id: u32 },
    ChannelRemoved { downstream_id: u32, channel_id: u32 },
    DownstreamConnected { downstream_id: u32, is_work_selection_enabled: bool },
    DownstreamDisconnected { downstream_id: u32 },
}

#[derive(Debug)]
pub enum StatsQuery {
    GetDownstreamStats(u32, oneshot::Sender<Option<DownstreamStats>>),
    GetAllDownstreams(oneshot::Sender<Vec<(u32, DownstreamStats)>>),
}

#[derive(Debug, Clone)]
pub struct DownstreamStats {
    pub shares_submitted: u64,
    pub quotes_created: u64,
    pub ehash_mined: u64,
    pub channels: Vec<u32>,
    pub last_share_time: Option<Instant>,
    pub connected_at: Instant,
    pub is_work_selection_enabled: bool,
}

impl Default for DownstreamStats {
    fn default() -> Self {
        Self {
            shares_submitted: 0,
            quotes_created: 0,
            ehash_mined: 0,
            channels: Vec::new(),
            last_share_time: None,
            connected_at: Instant::now(),
            is_work_selection_enabled: false,
        }
    }
}

pub struct StatsManager {
    stats_rx: mpsc::UnboundedReceiver<StatsMessage>,
    query_rx: mpsc::UnboundedReceiver<StatsQuery>,
    downstream_stats: HashMap<u32, DownstreamStats>,
}

impl StatsManager {
    pub fn new() -> (Self, StatsHandle) {
        let (stats_tx, stats_rx) = mpsc::unbounded_channel();
        let (query_tx, query_rx) = mpsc::unbounded_channel();
        
        let manager = Self {
            stats_rx,
            query_rx,
            downstream_stats: HashMap::new(),
        };
        
        let handle = StatsHandle {
            stats_tx,
            query_tx,
        };
        
        (manager, handle)
    }
    
    pub async fn run(&mut self) {
        loop {
            tokio::select! {
                Some(msg) = self.stats_rx.recv() => {
                    self.handle_stats_message(msg).await;
                }
                Some(query) = self.query_rx.recv() => {
                    self.handle_query(query).await;
                }
                else => break,
            }
        }
    }
    
    async fn handle_stats_message(&mut self, msg: StatsMessage) {
        match msg {
            StatsMessage::ShareSubmitted { downstream_id } => {
                if let Some(stats) = self.downstream_stats.get_mut(&downstream_id) {
                    stats.shares_submitted += 1;
                    stats.last_share_time = Some(Instant::now());
                }
            }
            StatsMessage::QuoteCreated { downstream_id, amount } => {
                if let Some(stats) = self.downstream_stats.get_mut(&downstream_id) {
                    stats.quotes_created += 1;
                    stats.ehash_mined += amount;
                }
            }
            StatsMessage::ChannelAdded { downstream_id, channel_id } => {
                if let Some(stats) = self.downstream_stats.get_mut(&downstream_id) {
                    if !stats.channels.contains(&channel_id) {
                        stats.channels.push(channel_id);
                    }
                }
            }
            StatsMessage::ChannelRemoved { downstream_id, channel_id } => {
                if let Some(stats) = self.downstream_stats.get_mut(&downstream_id) {
                    stats.channels.retain(|&id| id != channel_id);
                }
            }
            StatsMessage::DownstreamConnected { downstream_id, is_work_selection_enabled } => {
                self.downstream_stats.insert(downstream_id, DownstreamStats {
                    connected_at: Instant::now(),
                    is_work_selection_enabled,
                    ..Default::default()
                });
            }
            StatsMessage::DownstreamDisconnected { downstream_id } => {
                self.downstream_stats.remove(&downstream_id);
            }
        }
    }
    
    async fn handle_query(&self, query: StatsQuery) {
        match query {
            StatsQuery::GetDownstreamStats(downstream_id, response_tx) => {
                let stats = self.downstream_stats.get(&downstream_id).cloned();
                let _ = response_tx.send(stats);
            }
            StatsQuery::GetAllDownstreams(response_tx) => {
                let all_stats: Vec<(u32, DownstreamStats)> = self.downstream_stats
                    .iter()
                    .map(|(&id, stats)| (id, stats.clone()))
                    .collect();
                let _ = response_tx.send(all_stats);
            }
        }
    }
}

#[derive(Clone)]
pub struct StatsHandle {
    pub stats_tx: mpsc::UnboundedSender<StatsMessage>,
    pub query_tx: mpsc::UnboundedSender<StatsQuery>,
}

impl StatsHandle {
    pub fn send_stats(&self, msg: StatsMessage) {
        // Never blocks - if channel is full, we just drop the stat update
        let _ = self.stats_tx.send(msg);
    }
    
    pub async fn get_downstream_stats(&self, downstream_id: u32) -> Option<DownstreamStats> {
        let (tx, rx) = oneshot::channel();
        self.query_tx.send(StatsQuery::GetDownstreamStats(downstream_id, tx)).ok()?;
        rx.await.ok().flatten()
    }
    
    pub async fn get_all_downstream_stats(&self) -> Vec<(u32, DownstreamStats)> {
        let (tx, rx) = oneshot::channel();
        if self.query_tx.send(StatsQuery::GetAllDownstreams(tx)).is_ok() {
            rx.await.unwrap_or_default()
        } else {
            Vec::new()
        }
    }
}