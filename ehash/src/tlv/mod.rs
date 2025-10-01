//! Core TLV (Type-Length-Value) Infrastructure
//!
//! This module provides a generic, well-tested foundation for handling TLV extensions
//! in Stratum V2 messages. It supports multiple extension types and provides type-safe
//! operations for encoding/decoding TLV fields.

pub mod core;
pub mod frame_parser;
pub mod extension;
pub mod error;

// Re-export main types
pub use core::*;
pub use frame_parser::*;
pub use extension::*;
pub use error::*;