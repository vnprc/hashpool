//! Message Type Detection for Ehash Extension
//!
//! This module provides functionality to detect SRI message types from raw bytes
//! to determine which messages need TLV processing.

use crate::{InterceptorResult, InterceptorError};

/// Message types that the extension cares about
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageType {
    /// SubmitSharesExtended - needs TLV processing
    SubmitSharesExtended,
    /// SubmitSharesSuccess - may contain extension data
    SubmitSharesSuccess,
    /// RequestExtensions - for negotiation
    RequestExtensions,
    /// Other message types - pass through unchanged
    Other,
}

/// Detector for identifying SRI message types from raw bytes
pub struct MessageTypeDetector;

impl MessageTypeDetector {
    /// Detect message type from raw bytes
    /// 
    /// SRI messages have the format:
    /// [extension_type: u16][msg_type: u8][msg_length: u24][payload...]
    pub fn detect_message_type(msg_bytes: &[u8]) -> InterceptorResult<MessageType> {
        if msg_bytes.len() < 6 {
            return Err(InterceptorError::InsufficientData);
        }
        
        // Parse SRI header
        let extension_type = u16::from_le_bytes([msg_bytes[0], msg_bytes[1]]);
        let msg_type = msg_bytes[2];
        
        // Check for mining protocol messages (extension_type = 0)
        if extension_type == 0 {
            match msg_type {
                // SubmitSharesExtended message type
                0x04 => Ok(MessageType::SubmitSharesExtended),
                // SubmitSharesSuccess message type  
                0x05 => Ok(MessageType::SubmitSharesSuccess),
                _ => Ok(MessageType::Other),
            }
        }
        // Check for common protocol messages
        else if extension_type == 1 {
            match msg_type {
                // RequestExtensions message type
                0x02 => Ok(MessageType::RequestExtensions),
                _ => Ok(MessageType::Other),
            }
        }
        else {
            Ok(MessageType::Other)
        }
    }
    
    /// Check if message type needs TLV processing
    pub fn needs_tlv_processing(msg_type: MessageType) -> bool {
        matches!(msg_type, MessageType::SubmitSharesExtended | MessageType::SubmitSharesSuccess)
    }
    
    /// Check if message is outgoing (translator to pool)
    pub fn is_outgoing_message(msg_type: MessageType) -> bool {
        matches!(msg_type, MessageType::SubmitSharesExtended)
    }
    
    /// Check if message is incoming (pool from translator)
    pub fn is_incoming_message(msg_type: MessageType) -> bool {
        matches!(msg_type, MessageType::SubmitSharesSuccess)
    }
    
    /// Get message type name for logging
    pub fn message_type_name(msg_type: MessageType) -> &'static str {
        match msg_type {
            MessageType::SubmitSharesExtended => "SubmitSharesExtended",
            MessageType::SubmitSharesSuccess => "SubmitSharesSuccess", 
            MessageType::RequestExtensions => "RequestExtensions",
            MessageType::Other => "Other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{vec, vec::Vec};
    
    #[test]
    fn test_detect_submit_shares_extended() {
        // Mock SRI message bytes: [ext_type: 0x0000][msg_type: 0x04][length: 0x000020][payload...]
        let msg_bytes = vec![0x00, 0x00, 0x04, 0x00, 0x00, 0x20, 0x00]; // 7 bytes minimum
        
        let msg_type = MessageTypeDetector::detect_message_type(&msg_bytes).unwrap();
        assert_eq!(msg_type, MessageType::SubmitSharesExtended);
        assert!(MessageTypeDetector::needs_tlv_processing(msg_type));
        assert!(MessageTypeDetector::is_outgoing_message(msg_type));
    }
    
    #[test]
    fn test_detect_submit_shares_success() {
        // Mock SRI message bytes: [ext_type: 0x0000][msg_type: 0x05][length: 0x000010][payload...]
        let msg_bytes = vec![0x00, 0x00, 0x05, 0x00, 0x00, 0x10];
        
        let msg_type = MessageTypeDetector::detect_message_type(&msg_bytes).unwrap();
        assert_eq!(msg_type, MessageType::SubmitSharesSuccess);
        assert!(MessageTypeDetector::needs_tlv_processing(msg_type));
        assert!(MessageTypeDetector::is_incoming_message(msg_type));
    }
    
    #[test]
    fn test_detect_other_message() {
        // Mock SRI message bytes: [ext_type: 0x0000][msg_type: 0xFF][length: 0x000010][payload...]
        let msg_bytes = vec![0x00, 0x00, 0xFF, 0x00, 0x00, 0x10];
        
        let msg_type = MessageTypeDetector::detect_message_type(&msg_bytes).unwrap();
        assert_eq!(msg_type, MessageType::Other);
        assert!(!MessageTypeDetector::needs_tlv_processing(msg_type));
    }
    
    #[test]
    fn test_insufficient_data() {
        let msg_bytes = vec![0x00, 0x00]; // Too short
        
        let result = MessageTypeDetector::detect_message_type(&msg_bytes);
        assert!(matches!(result, Err(InterceptorError::InsufficientData)));
    }
}