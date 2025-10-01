//! Extension negotiation messages for Cashu integration
//! 
//! Implements the SRI extension negotiation protocol for Cashu support.

extern crate alloc;
use alloc::{vec, vec::Vec, string::String};

/// Extension 0x0001: Extension Negotiation
/// 
/// Message sent by client to request support for specific extensions
#[derive(Debug, Clone)]
pub struct RequestExtensions {
    /// List of extension IDs the client wants to use
    pub extension_types: Vec<u16>,
}

/// Response when all requested extensions are supported
#[derive(Debug, Clone)]
pub struct RequestExtensionsSuccess {
    /// List of extension IDs that the server supports from the request
    pub supported_extensions: Vec<u16>,
}

/// Response when some extensions are not supported or required extensions are missing
#[derive(Debug, Clone)]
pub struct RequestExtensionsError {
    /// Extensions from the request that are not supported
    pub unsupported_extensions: Vec<u16>,
    /// Extensions that are required but not requested
    pub required_extensions: Vec<u16>,
    /// Human-readable error message
    pub error_message: String,
}

/// Extension negotiation state for a connection
#[derive(Debug, Clone, Default)]
pub struct ExtensionState {
    /// Whether extension negotiation has been completed
    pub negotiated: bool,
    /// Set of extension IDs that both client and server support
    pub supported_extensions: Vec<u16>,
}

impl ExtensionState {
    /// Create a new extension state
    pub fn new() -> Self {
        Self {
            negotiated: false,
            supported_extensions: Vec::new(),
        }
    }

    /// Check if a specific extension is supported
    pub fn supports_extension(&self, extension_id: u16) -> bool {
        self.supported_extensions.contains(&extension_id)
    }

    /// Mark negotiation as complete with supported extensions
    pub fn complete_negotiation(&mut self, supported: Vec<u16>) {
        self.negotiated = true;
        self.supported_extensions = supported;
    }

    /// Check if Cashu extension is supported
    pub fn supports_cashu(&self) -> bool {
        self.supports_extension(crate::CASHU_EXTENSION_ID)
    }
}

/// Helper for creating extension negotiation messages
pub struct ExtensionNegotiator {
    /// Extensions this implementation supports
    supported_extensions: Vec<u16>,
}

impl ExtensionNegotiator {
    /// Create a new negotiator with Cashu extension support
    pub fn new_with_cashu() -> Self {
        Self {
            supported_extensions: vec![
                0x0001, // Extension Negotiation (required)
                crate::CASHU_EXTENSION_ID, // Cashu integration
            ],
        }
    }

    /// Process a RequestExtensions message and generate appropriate response
    pub fn process_request(
        &self, 
        request: &RequestExtensions
    ) -> Result<RequestExtensionsSuccess, RequestExtensionsError> {
        let mut supported = Vec::new();
        let mut unsupported = Vec::new();

        for &ext_id in &request.extension_types {
            if self.supported_extensions.contains(&ext_id) {
                supported.push(ext_id);
            } else {
                unsupported.push(ext_id);
            }
        }

        if unsupported.is_empty() {
            Ok(RequestExtensionsSuccess {
                supported_extensions: supported,
            })
        } else {
            Err(RequestExtensionsError {
                unsupported_extensions: unsupported,
                required_extensions: Vec::new(), // No required extensions for now
                error_message: "Some requested extensions are not supported".into(),
            })
        }
    }

    /// Create a RequestExtensions message for a client
    pub fn create_request(&self) -> RequestExtensions {
        RequestExtensions {
            extension_types: self.supported_extensions.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_extension_state() {
        let mut state = ExtensionState::new();
        assert!(!state.negotiated);
        assert!(!state.supports_cashu());

        state.complete_negotiation(vec![crate::CASHU_EXTENSION_ID]);
        assert!(state.negotiated);
        assert!(state.supports_cashu());
    }

    #[test]
    fn test_negotiator_success() {
        let negotiator = ExtensionNegotiator::new_with_cashu();
        let request = RequestExtensions {
            extension_types: vec![0x0001, crate::CASHU_EXTENSION_ID],
        };

        let result = negotiator.process_request(&request);
        assert!(result.is_ok());
        
        let success = result.unwrap();
        assert_eq!(success.supported_extensions.len(), 2);
        assert!(success.supported_extensions.contains(&crate::CASHU_EXTENSION_ID));
    }

    #[test]
    fn test_negotiator_partial_support() {
        let negotiator = ExtensionNegotiator::new_with_cashu();
        let request = RequestExtensions {
            extension_types: vec![0x0001, crate::CASHU_EXTENSION_ID, 0x9999], // 0x9999 unsupported
        };

        let result = negotiator.process_request(&request);
        assert!(result.is_err());
        
        let error = result.unwrap_err();
        assert_eq!(error.unsupported_extensions, vec![0x9999]);
    }
}