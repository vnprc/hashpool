//! Message utilities for integrating TLV fields with Stratum V2 messages
//!
//! This module provides high-level functions to append and extract Cashu TLV fields
//! to/from serialized Stratum V2 messages. It handles the integration between the
//! core binary_sv2 message format and the extension TLV data.

extern crate alloc;
use alloc::vec::Vec;

use crate::tlv::{CashuTlvEncoder, CashuTlvParser, CashuExtensionFields, TlvError};

/// Append Cashu TLV fields to a serialized message
///
/// This function takes a message that has already been serialized using binary_sv2
/// and appends the Cashu extension TLV fields to the end of the message.
///
/// # Arguments
/// * `message_payload` - The serialized message bytes (after binary_sv2 encoding)
/// * `locking_pubkey` - Optional 33-byte compressed public key for Cashu mint
///
/// # Returns
/// The original message with TLV fields appended
///
/// # Example Message Flow
/// ```text
/// 1. Original struct: SubmitSharesExtended { job_id: 123, ... }
/// 2. binary_sv2 serialize: [0x01, 0x02, 0x03, ...]
/// 3. append_cashu_tlv_to_message() adds: [..., 0x00, 0x03, 0x01, 0x00, 0x21, <33 bytes>]
/// 4. Result: Complete message with TLV extension data
/// ```
pub fn append_cashu_tlv_to_message(
    message_payload: &mut Vec<u8>,
    locking_pubkey: Option<&[u8]>,
) -> Result<(), TlvError> {
    CashuTlvEncoder::append_to_message(message_payload, locking_pubkey)
}

/// Extract Cashu TLV fields from a received message
///
/// This function takes a complete message (including TLV fields) and extracts
/// the Cashu extension fields, leaving the original message intact for normal
/// binary_sv2 deserialization.
///
/// # Arguments
/// * `complete_message` - The full message bytes (core message + TLV fields)
/// * `core_message_size` - Size of the core message (without TLV fields)
///
/// # Returns
/// Extracted Cashu extension fields
///
/// # Example Usage
/// ```text
/// 1. Receive complete message: [0x01, 0x02, 0x03, ..., TLV_DATA]
/// 2. Deserialize core with binary_sv2: SubmitSharesExtended { job_id: 123, ... }
/// 3. extract_cashu_tlv_from_message() parses TLV_DATA
/// 4. Result: Both core struct AND extension fields available
/// ```
pub fn extract_cashu_tlv_from_message(
    complete_message: &[u8],
    core_message_size: usize,
) -> Result<CashuExtensionFields, TlvError> {
    CashuTlvParser::parse_from_message(complete_message, core_message_size)
}

/// Helper function to determine the core message size for TLV extraction
///
/// This function helps calculate where the core message ends and TLV data begins.
/// It can be used when the exact size isn't known ahead of time.
///
/// # Arguments
/// * `message_type_id` - The SRI message type identifier
/// * `message_bytes` - The complete message bytes
///
/// # Returns
/// Estimated size of the core message (before TLV fields)
///
/// # Note
/// This is a placeholder implementation. A complete implementation would:
/// 1. Use message type to determine fixed vs variable size
/// 2. For variable size messages, parse the length fields
/// 3. Calculate exact core message boundary
pub fn calculate_core_message_size(
    _message_type_id: u8,
    message_bytes: &[u8],
) -> Result<usize, TlvError> {
    // PLACEHOLDER: For development, assume no TLV fields means entire message is core
    // TODO: Implement proper message size calculation based on SRI message format
    
    // Simple heuristic: look for TLV header pattern
    // TLV starts with [0x00, 0x03, field_type, length_low, length_high]
    let tlv_pattern = [0x00, 0x03];
    
    for i in 0..message_bytes.len().saturating_sub(5) {
        if message_bytes[i..i+2] == tlv_pattern {
            return Ok(i);
        }
    }
    
    // No TLV found, entire message is core
    Ok(message_bytes.len())
}

/// Wrapper for SubmitSharesExtended message processing
///
/// High-level helper that handles the complete flow of appending TLV fields
/// to a SubmitSharesExtended message during transmission.
pub fn prepare_submit_shares_extended_with_cashu(
    core_message_bytes: Vec<u8>,
    locking_pubkey: Option<&[u8]>,
) -> Result<Vec<u8>, TlvError> {
    let mut message = core_message_bytes;
    append_cashu_tlv_to_message(&mut message, locking_pubkey)?;
    Ok(message)
}

/// Wrapper for receiving SubmitSharesExtended message processing  
///
/// High-level helper that handles the complete flow of extracting TLV fields
/// from a received SubmitSharesExtended message.
pub fn process_received_submit_shares_extended_with_cashu(
    complete_message: &[u8],
    core_message_size: usize,
) -> Result<CashuExtensionFields, TlvError> {
    extract_cashu_tlv_from_message(complete_message, core_message_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_append_and_extract_tlv() {
        // Simulate a core message
        let mut core_message = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let original_size = core_message.len();
        let locking_pubkey = vec![9u8; 33];

        // Append TLV fields
        append_cashu_tlv_to_message(&mut core_message, Some(&locking_pubkey)).unwrap();

        // Verify TLV was appended
        assert!(core_message.len() > original_size);

        // Extract TLV fields
        let extracted = extract_cashu_tlv_from_message(&core_message, original_size).unwrap();

        // Verify extraction
        assert_eq!(extracted.locking_pubkey, Some(locking_pubkey));
    }

    #[test]
    fn test_core_message_size_calculation() {
        // Message without TLV
        let message_no_tlv = vec![1, 2, 3, 4, 5];
        let size = calculate_core_message_size(0x20, &message_no_tlv).unwrap();
        assert_eq!(size, message_no_tlv.len());

        // Message with TLV (pattern: 0x00, 0x03)
        let mut message_with_tlv = vec![1, 2, 3, 4, 5];
        message_with_tlv.extend_from_slice(&[0x00, 0x03, 0x01, 0x00, 0x21]);
        message_with_tlv.extend_from_slice(&vec![6u8; 33]); // TLV data

        let size = calculate_core_message_size(0x20, &message_with_tlv).unwrap();
        assert_eq!(size, 5); // Core message ends before TLV
    }

    #[test]
    fn test_high_level_wrappers() {
        let core_message = vec![10, 20, 30, 40];
        let locking_pubkey = vec![50u8; 33];

        // Prepare message with TLV
        let complete_message = prepare_submit_shares_extended_with_cashu(
            core_message.clone(),
            Some(&locking_pubkey),
        ).unwrap();

        // Process received message
        let extracted = process_received_submit_shares_extended_with_cashu(
            &complete_message,
            core_message.len(),
        ).unwrap();

        assert_eq!(extracted.locking_pubkey, Some(locking_pubkey));
    }
}