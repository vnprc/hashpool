//! Error types for TLV operations

use alloc::string::String;
use core::fmt;

/// Errors that can occur during TLV operations
#[derive(Debug, Clone, PartialEq)]
pub enum TlvError {
    /// Insufficient data to parse TLV field
    InsufficientData,
    /// Invalid TLV field format
    InvalidFormat,
    /// Unknown extension ID
    UnknownExtension(u16),
    /// Unknown field type for extension
    UnknownField { extension_id: u16, field_type: u16 },
    /// Field data has invalid length
    InvalidLength { expected: usize, actual: usize },
    /// Frame parsing error
    FrameError(String),
    /// Generic error with message
    Generic(String),
}

impl fmt::Display for TlvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlvError::InsufficientData => write!(f, "Insufficient data to parse TLV field"),
            TlvError::InvalidFormat => write!(f, "Invalid TLV field format"),
            TlvError::UnknownExtension(id) => write!(f, "Unknown extension ID: 0x{:04x}", id),
            TlvError::UnknownField { extension_id, field_type } => {
                write!(f, "Unknown field type 0x{:04x} for extension 0x{:04x}", field_type, extension_id)
            }
            TlvError::InvalidLength { expected, actual } => {
                write!(f, "Invalid field length: expected {}, got {}", expected, actual)
            }
            TlvError::FrameError(msg) => write!(f, "Frame error: {}", msg),
            TlvError::Generic(msg) => write!(f, "{}", msg),
        }
    }
}

/// Result type for TLV operations
pub type TlvResult<T> = Result<T, TlvError>;