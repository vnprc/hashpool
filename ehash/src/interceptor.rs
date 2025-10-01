//! Message Interceptor for Ehash Extension
//!
//! This module provides the core MessageInterceptor trait and implementation
//! for intercepting SRI messages at the byte level to add/extract TLV fields.

use alloc::vec::Vec;
use alloc::string::ToString;
use crate::{InterceptorResult, InterceptorError, ExtensionData};
use crate::{append_cashu_tlv_to_message, extract_cashu_tlv_from_message, calculate_core_message_size};
use const_sv2::MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED;

/// Trait for intercepting and modifying SRI messages at the byte level
pub trait MessageInterceptor {
    /// Intercept outgoing messages (translator to pool)
    /// Appends TLV fields to SubmitSharesExtended messages
    fn intercept_outgoing(&self, msg_bytes: &mut Vec<u8>) -> InterceptorResult<()>;
    
    /// Intercept incoming messages (pool from translator)
    /// Extracts TLV fields and returns core message + extension data
    fn intercept_incoming(&self, msg_bytes: &[u8]) -> InterceptorResult<(Vec<u8>, ExtensionData)>;
    
    /// Check if extension is negotiated for this connection
    fn is_extension_negotiated(&self) -> bool;
}

/// Ehash-specific implementation of MessageInterceptor
pub struct EhashMessageInterceptor {
    /// Extension negotiation state
    extension_negotiated: bool,
    /// Current locking pubkey for shares
    locking_pubkey: Option<[u8; 33]>,
}

impl EhashMessageInterceptor {
    /// Create new interceptor instance
    pub fn new() -> Self {
        Self {
            extension_negotiated: true, // Hardcoded for now as per plan
            locking_pubkey: None,
        }
    }
    
    /// Set the locking pubkey for future shares
    pub fn set_locking_pubkey(&mut self, pubkey: [u8; 33]) {
        self.locking_pubkey = Some(pubkey);
    }
    
    /// Get current locking pubkey
    pub fn get_locking_pubkey(&self) -> Option<[u8; 33]> {
        self.locking_pubkey
    }
}

impl Default for EhashMessageInterceptor {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageInterceptor for EhashMessageInterceptor {
    fn intercept_outgoing(&self, msg_bytes: &mut Vec<u8>) -> InterceptorResult<()> {
        if !self.extension_negotiated {
            return Err(InterceptorError::ExtensionNotNegotiated);
        }
        
        // Check if this is a SubmitSharesExtended message
        if msg_bytes.len() >= 6 {
            let msg_type = msg_bytes[2];
            if msg_type == MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED {
                // Only process if we have a locking pubkey to add
                if let Some(pubkey) = self.locking_pubkey {
                    tracing::debug!("📤 Appending TLV locking_pubkey to SubmitSharesExtended: {} bytes", pubkey.len());
                    
                    // Append TLV fields to message bytes
                    append_cashu_tlv_to_message(msg_bytes, Some(&pubkey))
                        .map_err(|e| InterceptorError::TlvError(e.to_string()))?;
                        
                    tracing::debug!("✅ TLV appended successfully, message now {} bytes", msg_bytes.len());
                } else {
                    tracing::warn!("No locking_pubkey set in interceptor for SubmitSharesExtended");
                }
            } else {
                tracing::debug!("Skipping TLV append for message type: 0x{:02x}", msg_type);
            }
        } else {
            tracing::warn!("Message too short for TLV processing: {} bytes", msg_bytes.len());
        }
        
        Ok(())
    }
    
    fn intercept_incoming(&self, msg_bytes: &[u8]) -> InterceptorResult<(Vec<u8>, ExtensionData)> {
        if !self.extension_negotiated {
            return Err(InterceptorError::ExtensionNotNegotiated);
        }
        
        tracing::debug!("📥 Pool intercepting incoming message: {} bytes", msg_bytes.len());
        
        // Check if this looks like a SubmitSharesExtended message
        if msg_bytes.len() >= 6 {
            let msg_type = msg_bytes[2];
            tracing::debug!("Incoming message type: 0x{:02x}", msg_type);
            
            if msg_type == MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED {
                // Calculate core message size and extract TLV fields
                let core_size = calculate_core_message_size(MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, msg_bytes)
                    .map_err(|e| InterceptorError::TlvError(e.to_string()))?;
                
                tracing::debug!("Core message size: {}, total size: {}", core_size, msg_bytes.len());
                
                if msg_bytes.len() > core_size {
                    tracing::debug!("Found {} bytes of potential TLV data", msg_bytes.len() - core_size);
                    
                    // Extract TLV fields from message
                    let ehash_fields = extract_cashu_tlv_from_message(msg_bytes, core_size)
                        .map_err(|e| InterceptorError::TlvError(e.to_string()))?;
                    
                    // Build corrected frame without TLV data
                    // The frame header needs its length field updated
                    let mut core_msg_bytes = msg_bytes[..core_size].to_vec();
                    
                    // Update the frame header's length field (bytes 3-5) to reflect the new payload size
                    // New payload length = core_size - 6 (header size)
                    let new_payload_len = (core_size - 6) as u32;
                    core_msg_bytes[3] = (new_payload_len & 0xFF) as u8;
                    core_msg_bytes[4] = ((new_payload_len >> 8) & 0xFF) as u8;
                    core_msg_bytes[5] = ((new_payload_len >> 16) & 0xFF) as u8;
                    
                    tracing::debug!("Updated frame header with new payload length: {}", new_payload_len);

                    let extension_data = ExtensionData {
                        ehash_fields,
                    };
                    
                    if let Some(ref pubkey) = extension_data.ehash_fields.locking_pubkey {
                        tracing::info!("✅ Extracted locking_pubkey from TLV: {} bytes", pubkey.len());
                    } else {
                        tracing::warn!("No locking_pubkey found in TLV fields");
                    }
                    
                    return Ok((core_msg_bytes, extension_data));
                } else {
                    tracing::debug!("No TLV data found (message size matches core size)");
                }
            }
        }
        
        // Return original message with empty extension data
        tracing::debug!("Returning original message with no TLV extraction");
        let extension_data = ExtensionData {
            ehash_fields: Default::default(),
        };
        
        Ok((msg_bytes.to_vec(), extension_data))
    }
    
    fn is_extension_negotiated(&self) -> bool {
        self.extension_negotiated
    }
}

#[cfg(test)]
#[path = "interceptor_tests.rs"]
mod tests;