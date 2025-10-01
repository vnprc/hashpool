//! TLV (Type-Length-Value) encoding and decoding for Cashu extension fields

extern crate alloc;
use alloc::vec::Vec;

use derive_more::Display;

use crate::{CASHU_EXTENSION_ID, FIELD_TYPE_LOCKING_PUBKEY};

/// Error types for TLV operations
#[derive(Debug, Display)]
pub enum TlvError {
    #[display("Invalid TLV type field")]
    InvalidType,
    #[display("Invalid TLV length")]
    InvalidLength,
    #[display("Insufficient data for TLV field")]
    InsufficientData,
    #[display("Invalid field type for Cashu extension")]
    InvalidFieldType,
    #[display("Serialization error")]
    SerializationError,
}

/// A single TLV field
#[derive(Debug, Clone)]
pub struct TlvField {
    /// Extension type (first 2 bytes of type field)
    pub extension_type: u16,
    /// Field type within extension (3rd byte of type field)
    pub field_type: u8,
    /// Field value
    pub value: Vec<u8>,
}

impl TlvField {
    /// Create a new TLV field
    pub fn new(extension_type: u16, field_type: u8, value: Vec<u8>) -> Self {
        Self {
            extension_type,
            field_type,
            value,
        }
    }

    /// Encode the TLV field to bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::new();
        
        // Type field: 3 bytes (U16 extension_type + U8 field_type)
        encoded.extend_from_slice(&self.extension_type.to_le_bytes());
        encoded.push(self.field_type);
        
        // Length field: 2 bytes (U16)
        let length = self.value.len() as u16;
        encoded.extend_from_slice(&length.to_le_bytes());
        
        // Value field
        encoded.extend_from_slice(&self.value);
        
        encoded
    }

    /// Decode a TLV field from bytes
    pub fn decode(data: &[u8]) -> Result<(Self, usize), TlvError> {
        if data.len() < 5 {
            return Err(TlvError::InsufficientData);
        }

        // Parse type field (3 bytes)
        let extension_type = u16::from_le_bytes([data[0], data[1]]);
        let field_type = data[2];

        // Parse length field (2 bytes)
        let length = u16::from_le_bytes([data[3], data[4]]) as usize;

        // Check if we have enough data for the value
        if data.len() < 5 + length {
            return Err(TlvError::InsufficientData);
        }

        // Extract value
        let value = data[5..5 + length].to_vec();

        Ok((
            TlvField {
                extension_type,
                field_type,
                value,
            },
            5 + length,
        ))
    }
}

/// Cashu extension fields extracted from TLV
#[derive(Debug, Clone, Default)]
pub struct CashuExtensionFields {
    /// Locking pubkey (33 bytes compressed)
    pub locking_pubkey: Option<Vec<u8>>,
}

/// TLV encoder for Cashu extension fields
pub struct CashuTlvEncoder;

impl CashuTlvEncoder {
    /// Encode Cashu fields as TLV and append to message payload
    pub fn append_to_message(
        payload: &mut Vec<u8>,
        locking_pubkey: Option<&[u8]>,
    ) -> Result<(), TlvError> {
        // Add locking_pubkey TLV field if present
        if let Some(pubkey) = locking_pubkey {
            if pubkey.len() != 33 {
                return Err(TlvError::InvalidLength);
            }
            let field = TlvField::new(
                CASHU_EXTENSION_ID,
                FIELD_TYPE_LOCKING_PUBKEY,
                pubkey.to_vec(),
            );
            payload.extend_from_slice(&field.encode());
        }

        Ok(())
    }

    /// Create TLV fields for Cashu extension
    pub fn create_tlv_fields(
        locking_pubkey: &[u8],
    ) -> Result<Vec<TlvField>, TlvError> {
        let mut fields = Vec::new();

        // Validate and add locking_pubkey
        if locking_pubkey.len() != 33 {
            return Err(TlvError::InvalidLength);
        }
        fields.push(TlvField::new(
            CASHU_EXTENSION_ID,
            FIELD_TYPE_LOCKING_PUBKEY,
            locking_pubkey.to_vec(),
        ));

        Ok(fields)
    }
}

/// TLV parser for extracting Cashu extension fields
pub struct CashuTlvParser;

impl CashuTlvParser {
    /// Parse TLV fields from the end of a message payload
    pub fn parse_from_message(payload: &[u8], base_message_size: usize) -> Result<CashuExtensionFields, TlvError> {
        if payload.len() < base_message_size {
            return Ok(CashuExtensionFields::default());
        }

        let tlv_data = &payload[base_message_size..];
        Self::parse_tlv_fields(tlv_data)
    }

    /// Parse all TLV fields and extract Cashu extension fields
    pub fn parse_tlv_fields(data: &[u8]) -> Result<CashuExtensionFields, TlvError> {
        let mut fields = CashuExtensionFields::default();
        let mut offset = 0;

        while offset < data.len() {
            let (field, consumed) = TlvField::decode(&data[offset..])?;
            offset += consumed;

            // Only process Cashu extension fields
            if field.extension_type == CASHU_EXTENSION_ID {
                match field.field_type {
                    FIELD_TYPE_LOCKING_PUBKEY => {
                        if field.value.len() == 33 {
                            fields.locking_pubkey = Some(field.value);
                        }
                    }
                    _ => {
                        // Unknown field type within Cashu extension, ignore
                    }
                }
            }
            // Ignore TLV fields from other extensions
        }

        Ok(fields)
    }

    /// Extract TLV fields for a specific extension
    pub fn extract_extension_fields(data: &[u8], extension_id: u16) -> Result<Vec<TlvField>, TlvError> {
        let mut fields = Vec::new();
        let mut offset = 0;

        while offset < data.len() {
            let (field, consumed) = TlvField::decode(&data[offset..])?;
            offset += consumed;

            if field.extension_type == extension_id {
                fields.push(field);
            }
        }

        Ok(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_tlv_field_encode_decode() {
        let original = TlvField::new(
            CASHU_EXTENSION_ID,
            FIELD_TYPE_LOCKING_PUBKEY,
            vec![1u8; 33],
        );

        let encoded = original.encode();
        let (decoded, consumed) = TlvField::decode(&encoded).unwrap();

        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.extension_type, original.extension_type);
        assert_eq!(decoded.field_type, original.field_type);
        assert_eq!(decoded.value, original.value);
    }

    #[test]
    fn test_cashu_fields_encoding() {
        let mut payload = vec![1, 2, 3, 4]; // Base message
        let locking_pubkey = vec![5u8; 33];

        CashuTlvEncoder::append_to_message(
            &mut payload,
            Some(&locking_pubkey),
        ).unwrap();

        // Check that TLV fields were appended
        assert!(payload.len() > 4);

        // Parse the TLV fields
        let tlv_data = &payload[4..];
        let fields = CashuTlvParser::parse_tlv_fields(tlv_data).unwrap();

        assert_eq!(fields.locking_pubkey, Some(locking_pubkey));
    }

    #[test]
    fn test_empty_tlv_parsing() {
        let fields = CashuTlvParser::parse_tlv_fields(&[]).unwrap();
        assert!(fields.locking_pubkey.is_none());
    }
}