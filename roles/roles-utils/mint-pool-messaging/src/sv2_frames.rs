//! Helpers for serializing mint-quote messages into full SV2 frames.
//!
//! These utilities make sure the SV2 header is correctly populated so both
//! pool and mint sides can rely on the standard framing implementation without
//! manually crafting byte arrays.

use binary_sv2::to_bytes;
use mint_quote_sv2::{
    CHANNEL_BIT_MINT_QUOTE_ERROR, CHANNEL_BIT_MINT_QUOTE_REQUEST, CHANNEL_BIT_MINT_QUOTE_RESPONSE,
    MESSAGE_TYPE_MINT_QUOTE_ERROR, MESSAGE_TYPE_MINT_QUOTE_REQUEST,
    MESSAGE_TYPE_MINT_QUOTE_RESPONSE, SV2_MINT_QUOTE_PROTOCOL_DISCRIMINANT,
};
use mint_quote_sv2::{MintQuoteError, MintQuoteRequest, MintQuoteResponse};

use crate::{MessagingError, MessagingResult};

/// Build a noise-ready SV2 frame for a mint quote request.
pub fn quote_request_frame_bytes(request: &MintQuoteRequest<'static>) -> MessagingResult<Vec<u8>> {
    let payload = to_bytes(request.clone()).map_err(|e| {
        MessagingError::Encoding(format!("failed to encode MintQuoteRequest: {e:?}"))
    })?;
    build_frame_bytes(
        payload,
        MESSAGE_TYPE_MINT_QUOTE_REQUEST,
        CHANNEL_BIT_MINT_QUOTE_REQUEST,
    )
}

/// Build a noise-ready SV2 frame for a mint quote response.
pub fn quote_response_frame_bytes(
    response: &MintQuoteResponse<'static>,
) -> MessagingResult<Vec<u8>> {
    let payload = to_bytes(response.clone()).map_err(|e| {
        MessagingError::Encoding(format!("failed to encode MintQuoteResponse: {e:?}"))
    })?;
    build_frame_bytes(
        payload,
        MESSAGE_TYPE_MINT_QUOTE_RESPONSE,
        CHANNEL_BIT_MINT_QUOTE_RESPONSE,
    )
}

/// Build a noise-ready SV2 frame for a mint quote error.
pub fn quote_error_frame_bytes(error: &MintQuoteError<'static>) -> MessagingResult<Vec<u8>> {
    let payload = to_bytes(error.clone())
        .map_err(|e| MessagingError::Encoding(format!("failed to encode MintQuoteError: {e:?}")))?;
    build_frame_bytes(
        payload,
        MESSAGE_TYPE_MINT_QUOTE_ERROR,
        CHANNEL_BIT_MINT_QUOTE_ERROR,
    )
}

fn build_frame_bytes(
    payload: Vec<u8>,
    message_type: u8,
    channel_msg: bool,
) -> MessagingResult<Vec<u8>> {
    let mut extension_type = SV2_MINT_QUOTE_PROTOCOL_DISCRIMINANT as u16;
    if channel_msg {
        extension_type |= 0x8000;
    }

    let mut frame = Vec::with_capacity(6 + payload.len());
    frame.extend_from_slice(&extension_type.to_le_bytes());
    frame.push(message_type);
    let len_bytes = (payload.len() as u32).to_le_bytes();
    frame.extend_from_slice(&len_bytes[..3]);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_sv2::Str0255;
    use std::convert::TryFrom;

    // Helper to create test MintQuoteRequest - using private fields would require the actual setup
    // These tests verify that frame generation functions don't panic and return valid data

    // ============================================================================
    // SV2 Frame Encoding Tests
    // ============================================================================

    #[test]
    fn test_quote_error_frame_encoding_basic() {
        // Test that error frame encoding works without panicking
        let error = MintQuoteError {
            error_code: 0x01,
            error_message: Str0255::try_from("Invalid request".to_string()).unwrap(),
        };

        let frame = quote_error_frame_bytes(&error).unwrap();
        // Should generate valid frame bytes
        assert!(!frame.is_empty());
        assert!(frame.len() >= 6);
        // Byte 2 should contain the message type
        assert_eq!(frame[2], MESSAGE_TYPE_MINT_QUOTE_ERROR);
    }

    #[test]
    fn test_quote_error_frame_various_error_codes() {
        // Test that various error codes are handled
        for error_code in [0x00, 0x01, 0x02, 0xFF] {
            let error = MintQuoteError {
                error_code,
                error_message: Str0255::try_from("error".to_string()).unwrap(),
            };

            let frame = quote_error_frame_bytes(&error).unwrap();
            assert!(!frame.is_empty());
            assert!(frame.len() >= 6);
        }
    }

    #[test]
    fn test_quote_response_frame_encoding_basic() {
        // Test that response frame encoding works
        let response = MintQuoteResponse {
            quote_id: Str0255::try_from("QUOTE-123".to_string()).unwrap(),
            header_hash: [0x33u8; 32].into(),
        };

        let frame = quote_response_frame_bytes(&response).unwrap();
        assert!(!frame.is_empty());
        assert!(frame.len() >= 6);
        // Byte 2 should contain the message type
        assert_eq!(frame[2], MESSAGE_TYPE_MINT_QUOTE_RESPONSE);
    }

    #[test]
    fn test_quote_response_large_quote_id() {
        // Test that long quote IDs are handled
        let long_id = "quote-with-a-very-long-identifier-1234567890".to_string();
        let response = MintQuoteResponse {
            quote_id: Str0255::try_from(long_id).unwrap(),
            header_hash: [0x44u8; 32].into(),
        };

        let frame = quote_response_frame_bytes(&response).unwrap();
        assert!(!frame.is_empty());
    }

    #[test]
    fn test_frame_structure_has_header() {
        // Verify frames have the expected header size
        let error = MintQuoteError {
            error_code: 0x05,
            error_message: Str0255::try_from("test".to_string()).unwrap(),
        };

        let frame = quote_error_frame_bytes(&error).unwrap();
        // Header is: extension_type(2) + message_type(1) + length(3) = 6 bytes minimum
        assert!(frame.len() >= 6, "Frame should have at least 6-byte header");
    }

    #[test]
    fn test_frame_length_field_correct() {
        // Verify the length field is correct
        let error = MintQuoteError {
            error_code: 0x20,
            error_message: Str0255::try_from("message".to_string()).unwrap(),
        };

        let frame = quote_error_frame_bytes(&error).unwrap();
        // Extract length from bytes 3-5
        let mut len_bytes = [0u8; 4];
        len_bytes[..3].copy_from_slice(&frame[3..6]);
        let payload_length = u32::from_le_bytes(len_bytes) as usize;
        // Payload length should equal frame length - header(6)
        assert_eq!(payload_length, frame.len() - 6);
    }
}
