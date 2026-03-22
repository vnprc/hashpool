//! High-level networking utilities for SV2 connections
//!
//! This module provides connection management, encrypted streams, and protocol handling
//! for Stratum V2 applications. It includes support for:
//!
//! - Noise-encrypted connections ([`noise_connection`], [`noise_stream`])
//! - SV1 protocol connections ([`sv1_connection`]) - when `sv1` feature is enabled
//! - Hostname resolution ([`resolve_hostname`])
//!
//! Originally from the `network_helpers_sv2` crate.

pub mod noise_connection;
pub mod noise_stream;
pub mod resolve_hostname;

#[cfg(feature = "sv1")]
pub mod sv1_connection;

pub use resolve_hostname::{resolve_host, resolve_host_port, ResolveError};

use async_channel::{RecvError, SendError};
use std::{fmt, time::Duration};
use stratum_core::{
    binary_sv2::{Deserialize, GetSize, Serialize},
    codec_sv2::{Error as CodecError, HandshakeRole},
    noise_sv2::{Initiator, Responder},
};
use tokio::net::TcpStream;

use crate::{
    key_utils::{Secp256k1PublicKey, Secp256k1SecretKey},
    network_helpers::noise_stream::NoiseTcpStream,
};

/// Networking errors that can occur in SV2 connections
#[derive(Debug)]
pub enum Error {
    /// Invalid handshake message received from remote peer
    HandshakeRemoteInvalidMessage,
    /// Error from the codec layer
    CodecError(CodecError),
    /// Error receiving from async channel
    RecvError,
    /// Error sending to async channel
    SendError,
    /// Socket was closed, likely by the peer
    SocketClosed,
    /// Handshake timeout
    HandshakeTimeout,
    /// Invalid key provided to construct an Initiator or Responder
    InvalidKey,
    /// DNS resolution failed for a hostname
    DnsResolutionFailed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::HandshakeRemoteInvalidMessage => {
                write!(f, "Invalid handshake message received from remote peer")
            }

            Error::CodecError(e) => write!(f, "{}", e),

            Error::RecvError => write!(f, "Error receiving from async channel"),

            Error::SendError => write!(f, "Error sending to async channel"),

            Error::SocketClosed => write!(f, "Socket was closed (likely by the peer)"),

            Error::HandshakeTimeout => write!(f, "Handshake timeout"),

            Error::InvalidKey => write!(f, "Invalid key provided for handshake"),

            Error::DnsResolutionFailed(msg) => write!(f, "DNS resolution failed: {msg}"),
        }
    }
}

impl From<CodecError> for Error {
    fn from(e: CodecError) -> Self {
        Error::CodecError(e)
    }
}

impl From<RecvError> for Error {
    fn from(_: RecvError) -> Self {
        Error::RecvError
    }
}

impl<T> From<SendError<T>> for Error {
    fn from(_: SendError<T>) -> Self {
        Error::SendError
    }
}

impl From<ResolveError> for Error {
    fn from(e: ResolveError) -> Self {
        Error::DnsResolutionFailed(e.to_string())
    }
}

/// Default handshake timeout used by [`connect_with_noise`] and [`accept_noise_connection`].
/// Use [`noise_stream::NoiseTcpStream::new`] directly to override.
const NOISE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Connects to an upstream server as a Noise initiator, returning the split read/write halves.
///
/// The handshake timeout is opinionated and fixed at [`NOISE_HANDSHAKE_TIMEOUT`]. If you need a
/// custom timeout, use [`noise_stream::NoiseTcpStream::new`] directly.
///
/// Pass `Some(key)` to verify the server's authority public key, or `None` to skip
/// verification (encrypted but unauthenticated — use only on trusted networks).
pub async fn connect_with_noise<Message>(
    stream: TcpStream,
    authority_pub_key: Option<Secp256k1PublicKey>,
) -> Result<NoiseTcpStream<Message>, Error>
where
    Message: Serialize + Deserialize<'static> + GetSize + Send + 'static,
{
    let initiator = match authority_pub_key {
        Some(key) => Initiator::from_raw_k(key.into_bytes()).map_err(|_| Error::InvalidKey)?,
        None => Initiator::without_pk().map_err(|_| Error::InvalidKey)?,
    };
    let stream = noise_stream::NoiseTcpStream::new(
        stream,
        HandshakeRole::Initiator(initiator),
        NOISE_HANDSHAKE_TIMEOUT,
    )
    .await?;
    Ok(stream)
}

/// Accepts a downstream connection as a Noise responder, returning the split read/write halves.
///
/// The handshake timeout is opinionated and fixed at [`NOISE_HANDSHAKE_TIMEOUT`]. If you need a
/// custom timeout, use [`noise_stream::NoiseTcpStream::new`] directly.
///
/// `cert_validity` controls how long the generated Noise certificate is valid,
/// which is independent of the handshake timeout.
pub async fn accept_noise_connection<Message>(
    stream: TcpStream,
    pub_key: Secp256k1PublicKey,
    prv_key: Secp256k1SecretKey,
    cert_validity: u64,
) -> Result<NoiseTcpStream<Message>, Error>
where
    Message: Serialize + Deserialize<'static> + GetSize + Send + 'static,
{
    let responder = Responder::from_authority_kp(
        &pub_key.into_bytes(),
        &prv_key.into_bytes(),
        Duration::from_secs(cert_validity),
    )
    .map_err(|_| Error::InvalidKey)?;
    let stream = noise_stream::NoiseTcpStream::new(
        stream,
        HandshakeRole::Responder(responder),
        NOISE_HANDSHAKE_TIMEOUT,
    )
    .await?;
    Ok(stream)
}
