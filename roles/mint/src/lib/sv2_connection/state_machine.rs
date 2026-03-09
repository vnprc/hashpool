#![allow(dead_code)]
//! Connection state machine for managing mint-pool SV2 connection lifecycle
//!
//! States:
//! - Disconnected: Not connected to pool
//! - Connecting: TCP connection established, awaiting Noise handshake
//! - Connected: Noise handshake complete, awaiting SetupConnection response
//! - Ready: SetupConnection accepted, ready to process quotes
//! - Error: Connection error state

use std::fmt;
use tracing::info;

/// Connection state enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected to pool
    Disconnected,
    /// TCP connection established, Noise handshake in progress
    Connecting,
    /// Noise handshake complete, SetupConnection sent
    SetupInProgress,
    /// Connection fully established and ready for operation
    Ready,
    /// Connection encountered an error
    Error,
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionState::Disconnected => write!(f, "Disconnected"),
            ConnectionState::Connecting => write!(f, "Connecting"),
            ConnectionState::SetupInProgress => write!(f, "SetupInProgress"),
            ConnectionState::Ready => write!(f, "Ready"),
            ConnectionState::Error => write!(f, "Error"),
        }
    }
}

/// Connection state machine
pub struct ConnectionStateMachine {
    current_state: ConnectionState,
    last_error: Option<String>,
}

impl ConnectionStateMachine {
    /// Create a new connection state machine
    pub fn new() -> Self {
        Self {
            current_state: ConnectionState::Disconnected,
            last_error: None,
        }
    }

    /// Get current connection state
    pub fn current_state(&self) -> ConnectionState {
        self.current_state
    }

    /// Get last error message if in error state
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Transition to Connecting state (after TCP connection established)
    pub fn tcp_connected(&mut self) -> Result<(), String> {
        match self.current_state {
            ConnectionState::Disconnected => {
                self.current_state = ConnectionState::Connecting;
                self.last_error = None;
                info!("✅ State transition: Disconnected → Connecting");
                Ok(())
            }
            other => Err(format!("Cannot connect from state: {}", other)),
        }
    }

    /// Transition to SetupInProgress state (after Noise handshake)
    pub fn noise_handshake_complete(&mut self) -> Result<(), String> {
        match self.current_state {
            ConnectionState::Connecting => {
                self.current_state = ConnectionState::SetupInProgress;
                self.last_error = None;
                info!("✅ State transition: Connecting → SetupInProgress");
                Ok(())
            }
            other => Err(format!("Cannot complete handshake from state: {}", other)),
        }
    }

    /// Transition to Ready state (after SetupConnection response)
    pub fn setup_connection_accepted(&mut self) -> Result<(), String> {
        match self.current_state {
            ConnectionState::SetupInProgress => {
                self.current_state = ConnectionState::Ready;
                self.last_error = None;
                info!("✅ State transition: SetupInProgress → Ready");
                Ok(())
            }
            other => Err(format!("Cannot accept setup from state: {}", other)),
        }
    }

    /// Transition to Error state
    pub fn error(&mut self, message: String) {
        let previous = self.current_state;
        self.current_state = ConnectionState::Error;
        self.last_error = Some(message.clone());
        info!("❌ State transition: {} → Error ({})", previous, message);
    }

    /// Transition back to Disconnected (for reconnection)
    pub fn reset(&mut self) {
        let previous = self.current_state;
        self.current_state = ConnectionState::Disconnected;
        self.last_error = None;
        info!("🔄 State transition: {} → Disconnected (reset)", previous);
    }

    /// Check if connection is ready for quote processing
    pub fn is_ready(&self) -> bool {
        self.current_state == ConnectionState::Ready
    }

    /// Check if connection is in a recoverable error state
    pub fn is_recoverable_error(&self) -> bool {
        matches!(self.current_state, ConnectionState::Error)
    }
}

impl Default for ConnectionStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let sm = ConnectionStateMachine::new();
        assert_eq!(sm.current_state(), ConnectionState::Disconnected);
        assert!(!sm.is_ready());
    }

    #[test]
    fn test_state_transitions() {
        let mut sm = ConnectionStateMachine::new();

        // Valid transitions
        assert!(sm.tcp_connected().is_ok());
        assert_eq!(sm.current_state(), ConnectionState::Connecting);

        assert!(sm.noise_handshake_complete().is_ok());
        assert_eq!(sm.current_state(), ConnectionState::SetupInProgress);

        assert!(sm.setup_connection_accepted().is_ok());
        assert_eq!(sm.current_state(), ConnectionState::Ready);
        assert!(sm.is_ready());
    }

    #[test]
    fn test_invalid_transitions() {
        let mut sm = ConnectionStateMachine::new();

        // Can't go directly to SetupInProgress
        assert!(sm.noise_handshake_complete().is_err());

        // Can't go directly to Ready
        assert!(sm.setup_connection_accepted().is_err());
    }

    #[test]
    fn test_error_state() {
        let mut sm = ConnectionStateMachine::new();
        sm.tcp_connected().unwrap();

        let error_msg = "Connection timeout".to_string();
        sm.error(error_msg.clone());

        assert_eq!(sm.current_state(), ConnectionState::Error);
        assert!(sm.is_recoverable_error());
        assert_eq!(sm.last_error(), Some("Connection timeout"));
    }

    #[test]
    fn test_reset() {
        let mut sm = ConnectionStateMachine::new();
        sm.tcp_connected().unwrap();
        sm.noise_handshake_complete().unwrap();
        sm.setup_connection_accepted().unwrap();

        assert!(sm.is_ready());

        sm.reset();
        assert_eq!(sm.current_state(), ConnectionState::Disconnected);
        assert!(!sm.is_ready());
    }
}
