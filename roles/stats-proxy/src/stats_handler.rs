use std::sync::Arc;
use tracing::debug;
use serde::{Deserialize, Serialize};

use crate::db::StatsDatabase;

pub struct StatsHandler {
    db: Arc<StatsDatabase>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StatsMessage {
    ShareSubmitted { downstream_id: u32, timestamp: u64 },
    QuoteCreated { downstream_id: u32, amount: u64, timestamp: u64 },
    ChannelOpened { downstream_id: u32, channel_id: u32 },
    ChannelClosed { downstream_id: u32, channel_id: u32 },
    DownstreamConnected { downstream_id: u32, flags: u32, #[serde(default)] name: String },
    DownstreamDisconnected { downstream_id: u32 },
    HashrateUpdate { downstream_id: u32, hashrate: f64, timestamp: u64 },
    BalanceUpdate { balance: u64, timestamp: u64 },
}

impl StatsHandler {
    pub fn new(db: Arc<StatsDatabase>) -> Self {
        Self { db }
    }

    pub async fn handle_message(&self, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        // Parse JSON message
        let msg: StatsMessage = serde_json::from_slice(data)?;

        match msg {
            StatsMessage::ShareSubmitted { downstream_id, timestamp } => {
                debug!("Share submitted: downstream_id={}, timestamp={}", downstream_id, timestamp);
                self.db.record_share(downstream_id, timestamp)?;
            }
            StatsMessage::QuoteCreated { downstream_id, amount, timestamp } => {
                debug!("Quote created: downstream_id={}, amount={}, timestamp={}", downstream_id, amount, timestamp);
                self.db.record_quote(downstream_id, amount, timestamp)?;
            }
            StatsMessage::ChannelOpened { downstream_id, channel_id } => {
                debug!("Channel opened: downstream_id={}, channel_id={}", downstream_id, channel_id);
                self.db.record_channel_opened(downstream_id, channel_id)?;
            }
            StatsMessage::ChannelClosed { downstream_id, channel_id } => {
                debug!("Channel closed: downstream_id={}, channel_id={}", downstream_id, channel_id);
                self.db.record_channel_closed(downstream_id, channel_id)?;
            }
            StatsMessage::DownstreamConnected { downstream_id, flags, name } => {
                debug!("Downstream connected: downstream_id={}, flags={}, name={}", downstream_id, flags, name);
                self.db.record_downstream_connected(downstream_id, flags, name)?;
            }
            StatsMessage::DownstreamDisconnected { downstream_id } => {
                debug!("Downstream disconnected: downstream_id={}", downstream_id);
                self.db.record_downstream_disconnected(downstream_id)?;
            }
            StatsMessage::HashrateUpdate { downstream_id, hashrate, timestamp } => {
                debug!("Hashrate update: downstream_id={}, hashrate={}, timestamp={}", downstream_id, hashrate, timestamp);
                self.db.record_hashrate(downstream_id, hashrate, timestamp)?;
            }
            StatsMessage::BalanceUpdate { balance, timestamp } => {
                debug!("Balance update: balance={}, timestamp={}", balance, timestamp);
                self.db.update_balance(balance)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_share_submitted_json_encoding() {
        let msg = StatsMessage::ShareSubmitted {
            downstream_id: 42,
            timestamp: 1234567890,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"ShareSubmitted"#));
        assert!(json.contains(r#""downstream_id":42"#));
        assert!(json.contains(r#""timestamp":1234567890"#));
    }

    #[test]
    fn test_share_submitted_json_decoding() {
        let json = r#"{"type":"ShareSubmitted","downstream_id":42,"timestamp":1234567890}"#;
        let msg: StatsMessage = serde_json::from_str(json).unwrap();

        match msg {
            StatsMessage::ShareSubmitted { downstream_id, timestamp } => {
                assert_eq!(downstream_id, 42);
                assert_eq!(timestamp, 1234567890);
            }
            _ => panic!("Expected ShareSubmitted variant"),
        }
    }

    #[test]
    fn test_quote_created_json_encoding() {
        let msg = StatsMessage::QuoteCreated {
            downstream_id: 7,
            amount: 5000,
            timestamp: 9876543210,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"QuoteCreated"#));
        assert!(json.contains(r#""downstream_id":7"#));
        assert!(json.contains(r#""amount":5000"#));
        assert!(json.contains(r#""timestamp":9876543210"#));
    }

    #[test]
    fn test_quote_created_json_decoding() {
        let json = r#"{"type":"QuoteCreated","downstream_id":7,"amount":5000,"timestamp":9876543210}"#;
        let msg: StatsMessage = serde_json::from_str(json).unwrap();

        match msg {
            StatsMessage::QuoteCreated { downstream_id, amount, timestamp } => {
                assert_eq!(downstream_id, 7);
                assert_eq!(amount, 5000);
                assert_eq!(timestamp, 9876543210);
            }
            _ => panic!("Expected QuoteCreated variant"),
        }
    }

    #[test]
    fn test_channel_opened_json_roundtrip() {
        let msg = StatsMessage::ChannelOpened {
            downstream_id: 10,
            channel_id: 200,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: StatsMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            StatsMessage::ChannelOpened { downstream_id, channel_id } => {
                assert_eq!(downstream_id, 10);
                assert_eq!(channel_id, 200);
            }
            _ => panic!("Expected ChannelOpened variant"),
        }
    }

    #[test]
    fn test_channel_closed_json_roundtrip() {
        let msg = StatsMessage::ChannelClosed {
            downstream_id: 15,
            channel_id: 300,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: StatsMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            StatsMessage::ChannelClosed { downstream_id, channel_id } => {
                assert_eq!(downstream_id, 15);
                assert_eq!(channel_id, 300);
            }
            _ => panic!("Expected ChannelClosed variant"),
        }
    }

    #[test]
    fn test_downstream_connected_json_roundtrip() {
        let msg = StatsMessage::DownstreamConnected {
            downstream_id: 20,
            flags: 1,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: StatsMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            StatsMessage::DownstreamConnected { downstream_id, flags } => {
                assert_eq!(downstream_id, 20);
                assert_eq!(flags, 1);
            }
            _ => panic!("Expected DownstreamConnected variant"),
        }
    }

    #[test]
    fn test_downstream_disconnected_json_roundtrip() {
        let msg = StatsMessage::DownstreamDisconnected {
            downstream_id: 25,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: StatsMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            StatsMessage::DownstreamDisconnected { downstream_id } => {
                assert_eq!(downstream_id, 25);
            }
            _ => panic!("Expected DownstreamDisconnected variant"),
        }
    }

    #[test]
    fn test_message_as_bytes() {
        let msg = StatsMessage::ShareSubmitted {
            downstream_id: 1,
            timestamp: 1000,
        };

        let bytes = serde_json::to_vec(&msg).unwrap();
        let decoded: StatsMessage = serde_json::from_slice(&bytes).unwrap();

        match decoded {
            StatsMessage::ShareSubmitted { downstream_id, timestamp } => {
                assert_eq!(downstream_id, 1);
                assert_eq!(timestamp, 1000);
            }
            _ => panic!("Expected ShareSubmitted variant"),
        }
    }

    #[test]
    fn test_invalid_json_returns_error() {
        let invalid_json = b"not valid json";
        let result: Result<StatsMessage, _> = serde_json::from_slice(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_type_field_returns_error() {
        let json = r#"{"downstream_id":42,"timestamp":1234567890}"#;
        let result: Result<StatsMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_type_returns_error() {
        let json = r#"{"type":"UnknownType","downstream_id":42}"#;
        let result: Result<StatsMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_all_message_types_unique() {
        let messages = vec![
            StatsMessage::ShareSubmitted { downstream_id: 1, timestamp: 1000 },
            StatsMessage::QuoteCreated { downstream_id: 1, amount: 100, timestamp: 1000 },
            StatsMessage::ChannelOpened { downstream_id: 1, channel_id: 10 },
            StatsMessage::ChannelClosed { downstream_id: 1, channel_id: 10 },
            StatsMessage::DownstreamConnected { downstream_id: 1, flags: 0 },
            StatsMessage::DownstreamDisconnected { downstream_id: 1 },
        ];

        let mut type_names = std::collections::HashSet::new();
        for msg in messages {
            let json = serde_json::to_string(&msg).unwrap();
            let json_value: serde_json::Value = serde_json::from_str(&json).unwrap();
            let type_name = json_value["type"].as_str().unwrap();
            assert!(type_names.insert(type_name.to_string()), "Duplicate type name: {}", type_name);
        }

        assert_eq!(type_names.len(), 6);
    }
}
