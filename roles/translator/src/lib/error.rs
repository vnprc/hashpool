//! ## Translator Error Module
//!
//! Defines the custom error types used throughout the translator proxy.
//!
//! This module centralizes error handling by providing:
//! - A primary `Error` enum encompassing various error kinds from different sources (I/O, parsing,
//!   protocol logic, channels, configuration, etc.).
//! - A specific `ChannelSendError` enum for errors occurring during message sending over
//!   asynchronous channels.

use ext_config::ConfigError;
use std::{
    fmt::{self, Formatter},
    marker::PhantomData,
    sync::PoisonError,
};
use stratum_apps::{
    stratum_core::{
        binary_sv2,
        channels_sv2::client::error::GroupChannelError,
        framing_sv2,
        handlers_sv2::HandlerErrorType,
        noise_sv2,
        parsers_sv2::{self, ParserError, TlvError},
        sv1_api::server_to_client::SetDifficulty,
    },
    utils::types::{
        CanDisconnect, CanFallback, CanShutdown, ChannelId, DownstreamId, ExtensionType,
        MessageType,
    },
};
use tokio::sync::broadcast;

pub type TproxyResult<T, Owner> = Result<T, TproxyError<Owner>>;

#[derive(Debug)]
pub struct ChannelManager;

#[derive(Debug)]
pub struct Sv1Server;

#[derive(Debug)]
pub struct Upstream;

#[derive(Debug)]
pub struct Downstream;

#[derive(Debug)]
pub struct TproxyError<Owner> {
    pub kind: TproxyErrorKind,
    pub action: Action,
    _owner: PhantomData<Owner>,
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
    Log,
    Disconnect(DownstreamId),
    Fallback,
    Shutdown,
}

impl CanDisconnect for Downstream {}
impl CanDisconnect for Sv1Server {}
impl CanDisconnect for ChannelManager {}

impl CanFallback for Upstream {}
impl CanFallback for ChannelManager {}
impl CanFallback for Sv1Server {}

impl CanShutdown for ChannelManager {}
impl CanShutdown for Sv1Server {}
impl CanShutdown for Downstream {}
impl CanShutdown for Upstream {}

impl<O> TproxyError<O> {
    pub fn log<E: Into<TproxyErrorKind>>(kind: E) -> Self {
        Self {
            kind: kind.into(),
            action: Action::Log,
            _owner: PhantomData,
        }
    }
}

impl<O> TproxyError<O>
where
    O: CanDisconnect,
{
    pub fn disconnect<E: Into<TproxyErrorKind>>(kind: E, downstream_id: DownstreamId) -> Self {
        Self {
            kind: kind.into(),
            action: Action::Disconnect(downstream_id),
            _owner: PhantomData,
        }
    }
}

impl<O> TproxyError<O>
where
    O: CanFallback,
{
    pub fn fallback<E: Into<TproxyErrorKind>>(kind: E) -> Self {
        Self {
            kind: kind.into(),
            action: Action::Fallback,
            _owner: PhantomData,
        }
    }
}

impl<O> TproxyError<O>
where
    O: CanShutdown,
{
    pub fn shutdown<E: Into<TproxyErrorKind>>(kind: E) -> Self {
        Self {
            kind: kind.into(),
            action: Action::Shutdown,
            _owner: PhantomData,
        }
    }
}

impl<Owner> From<TproxyError<Owner>> for TproxyErrorKind {
    fn from(value: TproxyError<Owner>) -> Self {
        value.kind
    }
}

#[derive(Debug)]
pub enum TproxyErrorKind {
    /// Generic SV1 protocol error
    SV1Error,
    /// Error from the network helpers library
    NetworkHelpersError(stratum_apps::network_helpers::Error),
    /// Error from roles logic parser library
    ParserError(ParserError),
    /// Errors on bad CLI argument input.
    BadCliArgs,
    /// Errors on bad `serde_json` serialize/deserialize.
    BadSerdeJson(serde_json::Error),
    /// Errors on bad `config` TOML deserialize.
    BadConfigDeserialize(ConfigError),
    /// Errors from `binary_sv2` crate.
    BinarySv2(binary_sv2::Error),
    /// Errors on bad noise handshake.
    CodecNoise(noise_sv2::Error),
    /// Errors from `framing_sv2` crate.
    FramingSv2(framing_sv2::Error),
    /// Errors on bad `TcpStream` connection.
    Io(std::io::Error),
    /// Errors on bad `String` to `int` conversion.
    ParseInt(std::num::ParseIntError),
    /// Mutex poison lock error
    PoisonLock,
    /// Channel receiver error
    ChannelErrorReceiver(async_channel::RecvError),
    /// Channel sender error
    ChannelErrorSender,
    /// Broadcast channel receiver error
    BroadcastChannelErrorReceiver(broadcast::error::RecvError),
    /// Tokio channel receiver error
    TokioChannelErrorRecv(tokio::sync::broadcast::error::RecvError),
    /// Error converting SetDifficulty to Message
    SetDifficultyToMessage(SetDifficulty),
    /// Received an unexpected message type
    UnexpectedMessage(ExtensionType, MessageType),
    /// Job not found during share validation
    JobNotFound,
    /// Invalid merkle root during share validation
    InvalidMerkleRoot,
    /// Pending channel not found for the given request ID
    PendingChannelNotFound(u32),
    /// Server does not support required extensions
    RequiredExtensionsNotSupported(Vec<u16>),
    /// Server requires extensions that the translator doesn't support
    ServerRequiresUnsupportedExtensions(Vec<u16>),
    /// Represents a generic channel send failure, described by a string.
    General(String),
    /// Error bubbling up from translator-core library
    TranslatorCore(stratum_apps::stratum_core::stratum_translation::error::StratumTranslationError),
    /// Downstream mapped to request id not found
    DownstreamNotFound(u32),
    /// Error about TLV encoding/decoding
    TlvError(parsers_sv2::TlvError),
    /// Setup connection error
    SetupConnectionError,
    /// Open mining channel error
    OpenMiningChannelError,
    /// Could not initiate subsystem
    CouldNotInitiateSystem,
    /// Channel not found
    DownstreamNotFoundWithChannelId(ChannelId),
    /// Channel not found
    ChannelNotFound,
    /// Failed to process SetNewPrevHash message
    FailedToProcessSetNewPrevHash,
    /// Failed to process NewExtendedMiningJob message
    FailedToProcessNewExtendedMiningJob,
    /// Failed to add channel id to group channel
    FailedToAddChannelIdToGroupChannel(GroupChannelError),
    /// Aggregated channel was closed
    AggregatedChannelClosed,
    /// Invalid key
    InvalidKey,
}

impl std::error::Error for TproxyErrorKind {}

impl fmt::Display for TproxyErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use TproxyErrorKind::*;
        match self {
            General(e) => write!(f, "{e}"),
            BadCliArgs => write!(f, "Bad CLI arg input"),
            BadSerdeJson(ref e) => write!(f, "Bad serde json: `{e:?}`"),
            BadConfigDeserialize(ref e) => write!(f, "Bad `config` TOML deserialize: `{e:?}`"),
            BinarySv2(ref e) => write!(f, "Binary SV2 error: `{e:?}`"),
            CodecNoise(ref e) => write!(f, "Noise error: `{e:?}"),
            FramingSv2(ref e) => write!(f, "Framing SV2 error: `{e:?}`"),
            Io(ref e) => write!(f, "I/O error: `{e:?}"),
            ParseInt(ref e) => write!(f, "Bad convert from `String` to `int`: `{e:?}`"),
            PoisonLock => write!(f, "Poison Lock error"),
            ChannelErrorReceiver(ref e) => write!(f, "Channel receive error: `{e:?}`"),
            BroadcastChannelErrorReceiver(ref e) => {
                write!(f, "Broadcast channel receive error: {e:?}")
            }
            ChannelErrorSender => write!(f, "Sender error"),
            TokioChannelErrorRecv(ref e) => write!(f, "Channel receive error: `{e:?}`"),
            SetDifficultyToMessage(ref e) => {
                write!(f, "Error converting SetDifficulty to Message: `{e:?}`")
            }
            UnexpectedMessage(extension_type, message_type) => {
                write!(
                    f,
                    "Received a message type that was not expected: {extension_type}, {message_type}"
                )
            }
            JobNotFound => write!(f, "Job not found during share validation"),
            InvalidMerkleRoot => write!(f, "Invalid merkle root during share validation"),
            PendingChannelNotFound(request_id) => {
                write!(f, "No pending channel found for request_id: {}", request_id)
            }
            RequiredExtensionsNotSupported(extensions) => {
                write!(
                    f,
                    "Server does not support required extensions: {:?}",
                    extensions
                )
            }
            ServerRequiresUnsupportedExtensions(extensions) => {
                write!(
                    f,
                    "Server requires extensions that we don't support: {:?}",
                    extensions
                )
            }
            SV1Error => write!(f, "Sv1 error"),
            TranslatorCore(ref e) => write!(f, "Translator core error: {e:?}"),
            NetworkHelpersError(ref e) => write!(f, "Network helpers error: {e:?}"),
            ParserError(ref e) => write!(f, "Roles logic parser error: {e:?}"),
            DownstreamNotFound(request_id) => write!(
                f,
                "Downstream id associated to request id: {request_id} not found"
            ),
            TlvError(ref e) => write!(f, "TLV error: {e:?}"),
            OpenMiningChannelError => write!(f, "failed to open mining channel"),
            SetupConnectionError => write!(f, "failed to setup connection with upstream"),
            CouldNotInitiateSystem => write!(f, "Could not initiate subsystem"),
            DownstreamNotFoundWithChannelId(channel_id) => {
                write!(f, "Downstream not found with channel id: {channel_id}")
            }
            ChannelNotFound => write!(f, "Channel not found"),
            FailedToProcessSetNewPrevHash => write!(f, "Failed to process SetNewPrevHash message"),
            FailedToProcessNewExtendedMiningJob => {
                write!(f, "Failed to process NewExtendedMiningJob message")
            }
            FailedToAddChannelIdToGroupChannel(ref e) => {
                write!(f, "Failed to add channel id to group channel: {e:?}")
            }
            AggregatedChannelClosed => write!(f, "Aggregated channel was closed"),
            InvalidKey => write!(f, "Invalid key used during noise handshake"),
        }
    }
}

impl From<binary_sv2::Error> for TproxyErrorKind {
    fn from(e: binary_sv2::Error) -> Self {
        TproxyErrorKind::BinarySv2(e)
    }
}

impl From<noise_sv2::Error> for TproxyErrorKind {
    fn from(e: noise_sv2::Error) -> Self {
        TproxyErrorKind::CodecNoise(e)
    }
}

impl From<framing_sv2::Error> for TproxyErrorKind {
    fn from(e: framing_sv2::Error) -> Self {
        TproxyErrorKind::FramingSv2(e)
    }
}

impl From<std::io::Error> for TproxyErrorKind {
    fn from(e: std::io::Error) -> Self {
        TproxyErrorKind::Io(e)
    }
}

impl From<std::num::ParseIntError> for TproxyErrorKind {
    fn from(e: std::num::ParseIntError) -> Self {
        TproxyErrorKind::ParseInt(e)
    }
}

impl From<serde_json::Error> for TproxyErrorKind {
    fn from(e: serde_json::Error) -> Self {
        TproxyErrorKind::BadSerdeJson(e)
    }
}

impl From<ConfigError> for TproxyErrorKind {
    fn from(e: ConfigError) -> Self {
        TproxyErrorKind::BadConfigDeserialize(e)
    }
}

impl From<async_channel::RecvError> for TproxyErrorKind {
    fn from(e: async_channel::RecvError) -> Self {
        TproxyErrorKind::ChannelErrorReceiver(e)
    }
}

impl From<tokio::sync::broadcast::error::RecvError> for TproxyErrorKind {
    fn from(e: tokio::sync::broadcast::error::RecvError) -> Self {
        TproxyErrorKind::TokioChannelErrorRecv(e)
    }
}

//*** LOCK ERRORS ***
impl<T> From<PoisonError<T>> for TproxyErrorKind {
    fn from(_e: PoisonError<T>) -> Self {
        TproxyErrorKind::PoisonLock
    }
}

impl From<SetDifficulty> for TproxyErrorKind {
    fn from(e: SetDifficulty) -> Self {
        TproxyErrorKind::SetDifficultyToMessage(e)
    }
}

impl<'a> From<stratum_apps::stratum_core::sv1_api::error::Error<'a>> for TproxyErrorKind {
    fn from(_: stratum_apps::stratum_core::sv1_api::error::Error<'a>) -> Self {
        TproxyErrorKind::SV1Error
    }
}

impl From<stratum_apps::network_helpers::Error> for TproxyErrorKind {
    fn from(value: stratum_apps::network_helpers::Error) -> Self {
        TproxyErrorKind::NetworkHelpersError(value)
    }
}

impl From<stratum_apps::stratum_core::stratum_translation::error::StratumTranslationError>
    for TproxyErrorKind
{
    fn from(
        e: stratum_apps::stratum_core::stratum_translation::error::StratumTranslationError,
    ) -> Self {
        TproxyErrorKind::TranslatorCore(e)
    }
}

impl From<ParserError> for TproxyErrorKind {
    fn from(value: ParserError) -> Self {
        TproxyErrorKind::ParserError(value)
    }
}

impl From<TlvError> for TproxyErrorKind {
    fn from(value: TlvError) -> Self {
        TproxyErrorKind::TlvError(value)
    }
}

impl HandlerErrorType for TproxyErrorKind {
    fn parse_error(error: ParserError) -> Self {
        TproxyErrorKind::ParserError(error)
    }

    fn unexpected_message(extension_type: ExtensionType, message_type: MessageType) -> Self {
        TproxyErrorKind::UnexpectedMessage(extension_type, message_type)
    }
}

impl<Owner> HandlerErrorType for TproxyError<Owner> {
    fn parse_error(error: ParserError) -> Self {
        Self {
            kind: TproxyErrorKind::ParserError(error),
            action: Action::Log,
            _owner: PhantomData,
        }
    }

    fn unexpected_message(extension_type: ExtensionType, message_type: MessageType) -> Self {
        Self {
            kind: TproxyErrorKind::UnexpectedMessage(extension_type, message_type),
            action: Action::Log,
            _owner: PhantomData,
        }
    }
}

impl<Owner> std::fmt::Display for TproxyError<Owner> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "[{:?}/{:?}]", self.kind, self.action)
    }
}
