use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct QuoteTracker {
    // Map share_hash -> quote_id for ecash minting
    quotes: Arc<Mutex<HashMap<Vec<u8>, String>>>,
}

impl QuoteTracker {
    pub fn new() -> Self {
        Self {
            quotes: Arc::new(Mutex::new(HashMap::new()))
        }
    }
    
    pub async fn store_quote(&self, share_hash: Vec<u8>, quote_id: String) {
        let mut quotes = self.quotes.lock().await;
        quotes.insert(share_hash, quote_id);
        
        // TODO this is toxic for low hashrate pools, think of something better or just remove it
        // Clean old entries if map gets too large
        if quotes.len() > 10000 {
            // Keep only recent 5000 entries (simple FIFO)
            let to_remove: Vec<_> = quotes.keys()
                .take(5000)
                .cloned()
                .collect();
            for key in to_remove {
                quotes.remove(&key);
            }
        }
    }
    
    pub async fn get_quote(&self, share_hash: &[u8]) -> Option<String> {
        let quotes = self.quotes.lock().await;
        quotes.get(share_hash).cloned()
    }
}