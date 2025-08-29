use std::fmt;

#[derive(Debug)]
pub enum MintError {
    Config(String),
    Database(String),
    Network(String),
    Sv2Connection(String),
    QuoteProcessing(String),
    Custom(String),
}

impl fmt::Display for MintError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MintError::Config(msg) => write!(f, "Configuration error: {}", msg),
            MintError::Database(msg) => write!(f, "Database error: {}", msg),
            MintError::Network(msg) => write!(f, "Network error: {}", msg),
            MintError::Sv2Connection(msg) => write!(f, "SV2 connection error: {}", msg),
            MintError::QuoteProcessing(msg) => write!(f, "Quote processing error: {}", msg),
            MintError::Custom(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for MintError {}

impl From<anyhow::Error> for MintError {
    fn from(err: anyhow::Error) -> Self {
        MintError::Custom(err.to_string())
    }
}