//! ## Status Reporting System
//!
//! This module provides a centralized way for components of the Translator to report
//! health updates, shutdown reasons, or fatal errors to the main runtime loop.
//!
//! Each task wraps its report in a [`Status`] and sends it over an async channel,
//! tagged with a [`Sender`] variant that identifies the source subsystem.

use stratum_apps::utils::types::DownstreamId;
use tracing::{debug, warn};

use crate::error::{Action, TproxyError, TproxyErrorKind};

/// Identifies the component that originated a [`Status`] update.
///
/// Each variant contains a channel to the main coordinator, and optionally a component ID
/// (e.g. a downstream connection ID).
#[derive(Debug, Clone)]
pub enum StatusSender {
    /// A specific downstream connection.
    Downstream {
        downstream_id: DownstreamId,
        tx: async_channel::Sender<Status>,
    },
    /// The SV1 server listener.
    Sv1Server(async_channel::Sender<Status>),
    /// The SV2 <-> SV1 bridge manager.
    ChannelManager(async_channel::Sender<Status>),
    /// The upstream SV2 connection handler.
    Upstream(async_channel::Sender<Status>),
}

impl StatusSender {
    /// Sends a [`Status`] update.
    #[cfg_attr(not(test), hotpath::measure)]
    pub async fn send(&self, status: Status) -> Result<(), async_channel::SendError<Status>> {
        match self {
            Self::Downstream { downstream_id, tx } => {
                debug!(
                    "Sending status from Downstream [{}]: {:?}",
                    downstream_id, status.state
                );
                tx.send(status).await
            }
            Self::Sv1Server(tx) => {
                debug!("Sending status from Sv1Server: {:?}", status.state);
                tx.send(status).await
            }
            Self::ChannelManager(tx) => {
                debug!("Sending status from ChannelManager: {:?}", status.state);
                tx.send(status).await
            }
            Self::Upstream(tx) => {
                debug!("Sending status from Upstream: {:?}", status.state);
                tx.send(status).await
            }
        }
    }
}

/// The type of event or error being reported by a component.
#[derive(Debug)]
pub enum State {
    /// Downstream task exited or encountered an unrecoverable error.
    DownstreamShutdown {
        downstream_id: DownstreamId,
        reason: TproxyErrorKind,
    },
    /// SV1 server listener exited unexpectedly.
    Sv1ServerShutdown(TproxyErrorKind),
    /// Channel manager shut down (SV2 bridge manager).
    ChannelManagerShutdown(TproxyErrorKind),
    /// Upstream SV2 connection closed or failed.
    UpstreamShutdown(TproxyErrorKind),
}

/// A message reporting the current [`State`] of a component.
#[derive(Debug)]
pub struct Status {
    pub state: State,
}

#[cfg_attr(not(test), hotpath::measure)]
async fn send_status<O>(sender: &StatusSender, error: TproxyError<O>) -> bool {
    use Action::*;

    match error.action {
        Log => {
            warn!("Log-only error from {:?}: {:?}", sender, error.kind);
            false
        }

        Disconnect(downstream_id) => {
            let state = State::DownstreamShutdown {
                downstream_id,
                reason: error.kind,
            };

            if let Err(e) = sender.send(Status { state }).await {
                tracing::error!(
                    "Failed to send downstream shutdown status from {:?}: {:?}",
                    sender,
                    e
                );
                std::process::abort();
            }
            matches!(sender, StatusSender::Downstream { .. })
        }

        Fallback => {
            let state = State::UpstreamShutdown(error.kind);

            if let Err(e) = sender.send(Status { state }).await {
                tracing::error!("Failed to send fallback status from {:?}: {:?}", sender, e);
                std::process::abort();
            }
            matches!(sender, StatusSender::Upstream { .. })
        }

        Shutdown => {
            let state = match sender {
                StatusSender::ChannelManager(_) => {
                    warn!(
                        "Channel Manager shutdown requested due to error: {:?}",
                        error.kind
                    );
                    State::ChannelManagerShutdown(error.kind)
                }
                StatusSender::Sv1Server(_) => {
                    warn!(
                        "Sv1Server shutdown requested due to error: {:?}",
                        error.kind
                    );
                    State::Sv1ServerShutdown(error.kind)
                }
                _ => State::ChannelManagerShutdown(error.kind),
            };

            if let Err(e) = sender.send(Status { state }).await {
                tracing::error!("Failed to send shutdown status from {:?}: {:?}", sender, e);
                std::process::abort();
            }
            true
        }
    }
}

#[cfg_attr(not(test), hotpath::measure)]
pub async fn handle_error<O>(sender: &StatusSender, e: TproxyError<O>) -> bool {
    send_status(sender, e).await
}
