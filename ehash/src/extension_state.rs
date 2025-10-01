//! Extension State Management for Ehash Extension
//!
//! This module manages the state of the ehash extension including
//! negotiation status and connection-specific data.

use alloc::collections::BTreeMap;

/// Connection identifier type
pub type ConnectionId = u64;

/// State for a single connection
#[derive(Debug, Clone)]
pub struct ConnectionState {
    /// Whether ehash extension is negotiated for this connection
    pub extension_negotiated: bool,
    /// Current locking pubkey for shares from this connection
    pub locking_pubkey: Option<[u8; 33]>,
    /// Connection type (translator or pool)
    pub connection_type: ConnectionType,
}

/// Type of connection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionType {
    /// Translator connection (sends SubmitSharesExtended)
    Translator,
    /// Pool connection (receives SubmitSharesExtended)
    Pool,
}

/// Global extension state manager
pub struct ExtensionState {
    /// Per-connection state
    connections: BTreeMap<ConnectionId, ConnectionState>,
    /// Default locking pubkey for new connections
    default_locking_pubkey: Option<[u8; 33]>,
}

impl ExtensionState {
    /// Create new extension state manager
    pub fn new() -> Self {
        Self {
            connections: BTreeMap::new(),
            default_locking_pubkey: None,
        }
    }
    
    /// Add a new connection
    pub fn add_connection(&mut self, conn_id: ConnectionId, conn_type: ConnectionType) {
        let state = ConnectionState {
            extension_negotiated: true, // Hardcoded for now as per plan
            locking_pubkey: self.default_locking_pubkey,
            connection_type: conn_type,
        };
        self.connections.insert(conn_id, state);
    }
    
    /// Remove a connection
    pub fn remove_connection(&mut self, conn_id: ConnectionId) {
        self.connections.remove(&conn_id);
    }
    
    /// Check if extension is negotiated for a connection
    pub fn is_extension_negotiated(&self, conn_id: ConnectionId) -> bool {
        self.connections
            .get(&conn_id)
            .map(|state| state.extension_negotiated)
            .unwrap_or(false)
    }
    
    /// Set extension negotiation status for a connection
    pub fn set_extension_negotiated(&mut self, conn_id: ConnectionId, negotiated: bool) {
        if let Some(state) = self.connections.get_mut(&conn_id) {
            state.extension_negotiated = negotiated;
        }
    }
    
    /// Get locking pubkey for a connection
    pub fn get_locking_pubkey(&self, conn_id: ConnectionId) -> Option<[u8; 33]> {
        self.connections
            .get(&conn_id)
            .and_then(|state| state.locking_pubkey)
    }
    
    /// Set locking pubkey for a connection
    pub fn set_locking_pubkey(&mut self, conn_id: ConnectionId, pubkey: [u8; 33]) {
        if let Some(state) = self.connections.get_mut(&conn_id) {
            state.locking_pubkey = Some(pubkey);
        }
    }
    
    /// Set default locking pubkey for new connections
    pub fn set_default_locking_pubkey(&mut self, pubkey: [u8; 33]) {
        self.default_locking_pubkey = Some(pubkey);
    }
    
    /// Get connection type
    pub fn get_connection_type(&self, conn_id: ConnectionId) -> Option<ConnectionType> {
        self.connections
            .get(&conn_id)
            .map(|state| state.connection_type)
    }
    
    /// Get connection state
    pub fn get_connection_state(&self, conn_id: ConnectionId) -> Option<&ConnectionState> {
        self.connections.get(&conn_id)
    }
    
    /// Get mutable connection state
    pub fn get_connection_state_mut(&mut self, conn_id: ConnectionId) -> Option<&mut ConnectionState> {
        self.connections.get_mut(&conn_id)
    }
    
    /// List all active connections
    pub fn list_connections(&self) -> impl Iterator<Item = (ConnectionId, &ConnectionState)> {
        self.connections.iter().map(|(&id, state)| (id, state))
    }
    
    /// Count connections by type
    pub fn count_connections_by_type(&self, conn_type: ConnectionType) -> usize {
        self.connections
            .values()
            .filter(|state| state.connection_type == conn_type)
            .count()
    }
}

impl Default for ExtensionState {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionState {
    /// Create new connection state
    pub fn new(conn_type: ConnectionType) -> Self {
        Self {
            extension_negotiated: true, // Hardcoded for now
            locking_pubkey: None,
            connection_type: conn_type,
        }
    }
    
    /// Check if this connection can send shares (translator)
    pub fn can_send_shares(&self) -> bool {
        matches!(self.connection_type, ConnectionType::Translator) && self.extension_negotiated
    }
    
    /// Check if this connection can receive shares (pool)
    pub fn can_receive_shares(&self) -> bool {
        matches!(self.connection_type, ConnectionType::Pool) && self.extension_negotiated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_extension_state_basic() {
        let mut state = ExtensionState::new();
        
        // Add translator connection
        state.add_connection(1, ConnectionType::Translator);
        assert!(state.is_extension_negotiated(1));
        assert_eq!(state.get_connection_type(1), Some(ConnectionType::Translator));
        
        // Add pool connection
        state.add_connection(2, ConnectionType::Pool);
        assert!(state.is_extension_negotiated(2));
        assert_eq!(state.get_connection_type(2), Some(ConnectionType::Pool));
        
        // Count connections
        assert_eq!(state.count_connections_by_type(ConnectionType::Translator), 1);
        assert_eq!(state.count_connections_by_type(ConnectionType::Pool), 1);
    }
    
    #[test]
    fn test_locking_pubkey_management() {
        let mut state = ExtensionState::new();
        let pubkey = [1u8; 33];
        
        // Set default pubkey
        state.set_default_locking_pubkey(pubkey);
        
        // Add connection - should inherit default
        state.add_connection(1, ConnectionType::Translator);
        assert_eq!(state.get_locking_pubkey(1), Some(pubkey));
        
        // Override for specific connection
        let new_pubkey = [2u8; 33];
        state.set_locking_pubkey(1, new_pubkey);
        assert_eq!(state.get_locking_pubkey(1), Some(new_pubkey));
    }
    
    #[test]
    fn test_connection_capabilities() {
        let translator_state = ConnectionState::new(ConnectionType::Translator);
        assert!(translator_state.can_send_shares());
        assert!(!translator_state.can_receive_shares());
        
        let pool_state = ConnectionState::new(ConnectionType::Pool);
        assert!(!pool_state.can_send_shares());
        assert!(pool_state.can_receive_shares());
    }
}