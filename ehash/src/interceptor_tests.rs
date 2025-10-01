//! Unit tests for the message interceptor
//! These tests help debug and verify TLV operations

#[cfg(test)]
mod tests {
    use crate::interceptor::*;
    use const_sv2::MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED;
    use alloc::vec::Vec;
    use alloc::vec;
    
    // Helper to create a mock SV2 frame
    fn create_mock_frame(msg_type: u8, payload: Vec<u8>) -> Vec<u8> {
        let mut frame = Vec::new();
        
        // Extension type (2 bytes) - no extension
        frame.push(0x00);
        frame.push(0x00);
        
        // Message type (1 byte)
        frame.push(msg_type);
        
        // Payload length (3 bytes, little-endian)
        let len = payload.len() as u32;
        frame.push((len & 0xFF) as u8);
        frame.push(((len >> 8) & 0xFF) as u8);
        frame.push(((len >> 16) & 0xFF) as u8);
        
        // Payload
        frame.extend_from_slice(&payload);
        
        frame
    }
    
    // Helper to create a SubmitSharesExtended payload
    fn create_submit_shares_payload() -> Vec<u8> {
        let mut payload = Vec::new();
        
        // channel_id: u32 = 1
        payload.extend_from_slice(&1u32.to_le_bytes());
        
        // sequence_number: u32 = 0
        payload.extend_from_slice(&0u32.to_le_bytes());
        
        // job_id: u32 = 1
        payload.extend_from_slice(&1u32.to_le_bytes());
        
        // nonce: u32 = 12345
        payload.extend_from_slice(&12345u32.to_le_bytes());
        
        // ntime: u32 = 1234567890
        payload.extend_from_slice(&1234567890u32.to_le_bytes());
        
        // version: u32 = 0x20000000
        payload.extend_from_slice(&0x20000000u32.to_le_bytes());
        
        // extranonce length: u8 = 8
        payload.push(8);
        
        // extranonce: 8 bytes
        payload.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        
        payload
    }
    
    // Helper to append mock TLV data
    fn append_mock_tlv(frame: &mut Vec<u8>, locking_pubkey: &[u8]) {
        // Extension ID: 0x0003 (Cashu/ehash)
        frame.push(0x00);
        frame.push(0x03);
        
        // Field type: 0x0001 (locking_pubkey)
        frame.push(0x00);
        frame.push(0x01);
        
        // Length: 33 bytes for pubkey
        let len = locking_pubkey.len() as u16;
        frame.push((len & 0xFF) as u8);
        frame.push(((len >> 8) & 0xFF) as u8);
        
        // Data
        frame.extend_from_slice(locking_pubkey);
        
        // Update frame header length
        let new_payload_len = (frame.len() - 6) as u32;
        frame[3] = (new_payload_len & 0xFF) as u8;
        frame[4] = ((new_payload_len >> 8) & 0xFF) as u8;
        frame[5] = ((new_payload_len >> 16) & 0xFF) as u8;
    }
    
    #[test]
    fn test_interceptor_creation() {
        let interceptor = EhashMessageInterceptor::new();
        assert!(interceptor.is_extension_negotiated());
    }
    
    #[test]
    fn test_set_locking_pubkey() {
        let mut interceptor = EhashMessageInterceptor::new();
        let pubkey = [0x03; 33];
        interceptor.set_locking_pubkey(pubkey);
        assert_eq!(interceptor.get_locking_pubkey(), Some(pubkey));
    }
    
    #[test]
    fn test_intercept_outgoing_no_pubkey() {
        let interceptor = EhashMessageInterceptor::new();
        let mut frame = create_mock_frame(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            create_submit_shares_payload()
        );
        let original_len = frame.len();
        
        // Should not modify frame if no pubkey is set
        let result = interceptor.intercept_outgoing(&mut frame);
        assert!(result.is_ok());
        assert_eq!(frame.len(), original_len);
    }
    
    #[test]
    fn test_intercept_outgoing_with_pubkey() {
        let mut interceptor = EhashMessageInterceptor::new();
        let pubkey = [0x03; 33];
        interceptor.set_locking_pubkey(pubkey);
        
        let mut frame = create_mock_frame(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            create_submit_shares_payload()
        );
        let original_len = frame.len();
        
        let result = interceptor.intercept_outgoing(&mut frame);
        assert!(result.is_ok());
        
        // Frame should be extended with TLV data
        // TLV overhead: 2 (ext_id) + 2 (field_type) + 2 (length) + 33 (pubkey) = 39 bytes
        assert_eq!(frame.len(), original_len + 39);
    }
    
    #[test]
    fn test_intercept_incoming_no_tlv() {
        let interceptor = EhashMessageInterceptor::new();
        let frame = create_mock_frame(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            create_submit_shares_payload()
        );
        
        let result = interceptor.intercept_incoming(&frame);
        assert!(result.is_ok());
        
        let (core_bytes, extension_data) = result.unwrap();
        assert_eq!(core_bytes.len(), frame.len());
        assert!(extension_data.ehash_fields.locking_pubkey.is_none());
    }
    
    #[test]
    fn test_intercept_incoming_with_tlv() {
        let interceptor = EhashMessageInterceptor::new();
        let mut frame = create_mock_frame(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            create_submit_shares_payload()
        );
        
        let pubkey = [0x02; 33];
        append_mock_tlv(&mut frame, &pubkey);
        
        let result = interceptor.intercept_incoming(&frame);
        assert!(result.is_ok());
        
        let (core_bytes, extension_data) = result.unwrap();
        
        // Core bytes should be smaller than original frame
        assert!(core_bytes.len() < frame.len());
        
        // Should extract the pubkey
        assert_eq!(
            extension_data.ehash_fields.locking_pubkey,
            Some(pubkey.to_vec())
        );
        
        // Core frame should have updated length in header
        let core_payload_len = ((core_bytes[5] as u32) << 16) 
            | ((core_bytes[4] as u32) << 8) 
            | (core_bytes[3] as u32);
        assert_eq!(core_payload_len as usize, core_bytes.len() - 6);
    }
    
    #[test]
    fn test_intercept_wrong_message_type() {
        let interceptor = EhashMessageInterceptor::new();
        let mut frame = create_mock_frame(
            0x15, // Different message type
            vec![0; 20]
        );
        
        // Should not process non-SubmitSharesExtended messages
        let result = interceptor.intercept_incoming(&frame);
        assert!(result.is_ok());
        
        let (core_bytes, extension_data) = result.unwrap();
        assert_eq!(core_bytes, frame);
        assert!(extension_data.ehash_fields.locking_pubkey.is_none());
    }
    
    #[test]
    fn test_frame_length_update() {
        // Test that frame header length is correctly updated
        let mut frame = create_mock_frame(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            create_submit_shares_payload()
        );
        
        let original_payload_len = frame.len() - 6;
        
        // Manually add TLV data
        let tlv_data = vec![0; 39];
        frame.extend_from_slice(&tlv_data);
        
        // Update header length
        let new_len = (frame.len() - 6) as u32;
        frame[3] = (new_len & 0xFF) as u8;
        frame[4] = ((new_len >> 8) & 0xFF) as u8;
        frame[5] = ((new_len >> 16) & 0xFF) as u8;
        
        // Verify length was updated correctly
        let header_len = ((frame[5] as u32) << 16) 
            | ((frame[4] as u32) << 8) 
            | (frame[3] as u32);
        assert_eq!(header_len as usize, original_payload_len + 39);
    }
    
    #[test]
    fn test_roundtrip_with_tlv() {
        let mut interceptor = EhashMessageInterceptor::new();
        let pubkey = [0x03; 33];
        interceptor.set_locking_pubkey(pubkey);
        
        // Create original frame
        let mut frame = create_mock_frame(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            create_submit_shares_payload()
        );
        let original_core = frame.clone();
        
        // Outgoing: add TLV
        interceptor.intercept_outgoing(&mut frame).unwrap();
        assert!(frame.len() > original_core.len());
        
        // Incoming: extract TLV
        let (core_bytes, extension_data) = interceptor.intercept_incoming(&frame).unwrap();
        
        // Should recover original pubkey
        assert_eq!(
            extension_data.ehash_fields.locking_pubkey,
            Some(pubkey.to_vec())
        );
        
        // Core bytes should match original (with updated header)
        assert_eq!(core_bytes.len(), original_core.len());
        assert_eq!(&core_bytes[0..3], &original_core[0..3]); // Extension type and msg type
        // Payload should match
        assert_eq!(&core_bytes[6..], &original_core[6..]);
    }
}