use core::fmt;

use miniscript::bitcoin::{address, hex};

/// Error enum
#[derive(Debug)]
pub enum Error {
    /// Error parsing a Bitcoin address
    Address(address::ParseError),
    /// Error parsing a raw descriptor as hex.
    Hex(hex::HexToBytesError),
    /// Invalid `output_script_value` for script type. It must be a valid public key/script
    InvalidOutputScript,
    /// Unknown script type in config
    UnknownOutputScriptType,
    /// Error from the `miniscript` crate.
    Miniscript(miniscript::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use Error::*;
        match self {
            Address(ref e) => write!(f, "Bitcoin address: {e}"),
            Hex(ref e) => write!(f, "Decoding hex-formatted script: {e}"),
            UnknownOutputScriptType => write!(f, "Unknown script type in config"),
            InvalidOutputScript => write!(f, "Invalid output_script_value for your script type. It must be a valid public key/script"),
            Miniscript(ref e) => write!(f, "Miniscript: {e}"),
        }
    }
}

impl From<address::ParseError> for Error {
    fn from(e: address::ParseError) -> Self {
        Error::Address(e)
    }
}

impl From<hex::HexToBytesError> for Error {
    fn from(e: hex::HexToBytesError) -> Self {
        Error::Hex(e)
    }
}

impl From<miniscript::Error> for Error {
    fn from(e: miniscript::Error) -> Self {
        Error::Miniscript(e)
    }
}
