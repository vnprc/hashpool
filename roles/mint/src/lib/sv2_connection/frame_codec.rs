#![allow(dead_code)]
//! SV2 Frame Codec for serializing and deserializing mint-pool messages
//!
//! This module handles encoding/decoding of:
//! - MintQuoteRequest (0x80)
//! - MintQuoteResponse (0x81)
//! - MintQuoteError (0x82)
//!
//! Using the binary_sv2 codec infrastructure

use anyhow::Result;
use binary_sv2::{Encodable, EncodableField, GetSize};
use mint_quote_sv2::{MintQuoteError, MintQuoteRequest, MintQuoteResponse};
use tracing::info;

/// Message wrapper enum for all mint-pool messages
/// Implements GetSize and EncodableField for use with Connection::new()
#[derive(Clone, Debug)]
pub enum MintPoolMessage<'a> {
    MintQuoteRequest(MintQuoteRequest<'a>),
    MintQuoteResponse(MintQuoteResponse<'a>),
    MintQuoteError(MintQuoteError<'a>),
}

/// GetSize implementation for MintPoolMessage
/// Delegates to inner message types for size calculation
impl<'a> GetSize for MintPoolMessage<'a> {
    fn get_size(&self) -> usize {
        match self {
            Self::MintQuoteRequest(msg) => msg.get_size(),
            Self::MintQuoteResponse(msg) => msg.get_size(),
            Self::MintQuoteError(msg) => msg.get_size(),
        }
    }
}

/// Encodable trait implementation via EncodableField conversion
/// This is automatically derived from the From impl below via blanket impl
impl<'a> From<MintPoolMessage<'a>> for EncodableField<'a> {
    fn from(m: MintPoolMessage<'a>) -> Self {
        match m {
            MintPoolMessage::MintQuoteRequest(msg) => msg.into(),
            MintPoolMessage::MintQuoteResponse(msg) => msg.into(),
            MintPoolMessage::MintQuoteError(msg) => msg.into(),
        }
    }
}

/// Frame type identifiers
#[allow(dead_code)]
pub mod frame_types {
    pub const MINT_QUOTE_REQUEST: u8 = 0x80;
    pub const MINT_QUOTE_RESPONSE: u8 = 0x81;
    pub const MINT_QUOTE_ERROR: u8 = 0x82;
}

/// Encode a MintQuoteResponse into SV2 frame bytes
#[allow(dead_code)]
pub fn encode_mint_quote_response(response: &MintQuoteResponse<'_>) -> Result<Vec<u8>> {
    // Encode using MintQuoteResponse's Encodable trait
    let mut buffer = Vec::new();
    response
        .clone()
        .to_bytes(&mut buffer)
        .map_err(|e| anyhow::anyhow!("Failed to encode MintQuoteResponse: {:?}", e))?;

    // Prepend message type byte (0x81)
    let mut frame_bytes = vec![frame_types::MINT_QUOTE_RESPONSE];
    frame_bytes.extend_from_slice(&buffer);

    info!(
        "Encoded MintQuoteResponse frame ({} bytes)",
        frame_bytes.len()
    );
    Ok(frame_bytes)
}

/// Encode a MintQuoteError into SV2 frame bytes
pub fn encode_mint_quote_error(error_code: u32, error_message: &str) -> Result<Vec<u8>> {
    // Validate error message length (Str0255 requires 1-255 bytes)
    if error_message.is_empty() || error_message.len() > 255 {
        return Err(anyhow::anyhow!(
            "Error message must be 1-255 bytes (got {} bytes)",
            error_message.len()
        ));
    }

    // Encode as binary: error_code (4 bytes, little-endian) + message length (1 byte) + message
    let mut frame_bytes = vec![frame_types::MINT_QUOTE_ERROR];
    frame_bytes.extend_from_slice(&error_code.to_le_bytes());
    frame_bytes.push(error_message.len() as u8);
    frame_bytes.extend_from_slice(error_message.as_bytes());

    info!("Encoded MintQuoteError frame ({} bytes)", frame_bytes.len());
    Ok(frame_bytes)
}

/// Decode frame bytes into MintQuoteResponse
/// NOTE: Not currently used - responses are sent from pool, not received and decoded
pub fn decode_mint_quote_response(_frame_bytes: &[u8]) -> Result<MintQuoteResponse<'static>> {
    // Requires frame header (message type) to be already consumed by caller
    // Parse frame payload to reconstruct MintQuoteResponse
    // This would use binary_sv2 Decodable trait if needed for bidirectional communication

    anyhow::bail!("MintQuoteResponse decoding not implemented - not currently needed")
}

/// Decode frame bytes into MintQuoteError
/// NOTE: Not currently used - errors are sent from mint, not received and decoded
pub fn decode_mint_quote_error(frame_bytes: &[u8]) -> Result<(u32, String)> {
    if frame_bytes.len() < 4 {
        anyhow::bail!("MintQuoteError frame too short");
    }

    let error_code = u32::from_le_bytes([
        frame_bytes[0],
        frame_bytes[1],
        frame_bytes[2],
        frame_bytes[3],
    ]);

    let message = String::from_utf8_lossy(&frame_bytes[4..]).into_owned();

    Ok((error_code, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_type_constants() {
        assert_eq!(frame_types::MINT_QUOTE_REQUEST, 0x80);
        assert_eq!(frame_types::MINT_QUOTE_RESPONSE, 0x81);
        assert_eq!(frame_types::MINT_QUOTE_ERROR, 0x82);
    }

    #[test]
    fn test_encode_quote_error() {
        let result = encode_mint_quote_error(1, "test error");
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert_eq!(bytes[0], frame_types::MINT_QUOTE_ERROR);
    }

    #[test]
    fn test_decode_quote_error() {
        let mut frame_bytes = vec![1u8, 0, 0, 0]; // error code = 1
        frame_bytes.extend_from_slice(b"test error");

        let result = decode_mint_quote_error(&frame_bytes);
        assert!(result.is_ok());
        let (code, msg) = result.unwrap();
        assert_eq!(code, 1);
        assert_eq!(msg, "test error");
    }
}
