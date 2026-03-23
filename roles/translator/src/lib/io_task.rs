use std::sync::Arc;

use async_channel::{Receiver, Sender};
use stratum_apps::{
    fallback_coordinator::FallbackCoordinator,
    network_helpers::noise_stream::{NoiseTcpReadHalf, NoiseTcpWriteHalf},
    stratum_core::framing_sv2::framing::Frame,
    task_manager::TaskManager,
    utils::types::{Message, Sv2Frame},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, trace, warn, Instrument as _};

#[cfg_attr(not(test), hotpath::measure)]
#[track_caller]
#[allow(clippy::too_many_arguments)]
pub fn spawn_io_tasks(
    task_manager: Arc<TaskManager>,
    mut reader: NoiseTcpReadHalf<Message>,
    mut writer: NoiseTcpWriteHalf<Message>,
    outbound_rx: Receiver<Sv2Frame>,
    inbound_tx: Sender<Sv2Frame>,
    cancellation_token: CancellationToken,
    fallback_coordinator: FallbackCoordinator,
) {
    let caller = std::panic::Location::caller();
    let inbound_tx_clone = inbound_tx.clone();
    let outbound_rx_clone = outbound_rx.clone();

    {
        let cancellation_token_clone = cancellation_token.clone();
        let fallback_coordinator_clone = fallback_coordinator.clone();
        task_manager.spawn(
            async move {
                // we just spawned a new task that's relevant to fallback coordination
                // so register it with the fallback coordinator
                let fallback_handler = fallback_coordinator_clone.register();

                // get the cancellation token that signals fallback
                let fallback_token = fallback_coordinator_clone.token();

                trace!("Reader task started");
                loop {
                    tokio::select! {
                        _ = cancellation_token_clone.cancelled() => {
                            trace!("Received app shutdown signal");
                            inbound_tx.close();
                            break;
                        }
                        _ = fallback_token.cancelled() => {
                            trace!("Received fallback signal");
                            inbound_tx.close();
                            break;
                        }
                        res = reader.read_frame() => {
                            match res {
                                Ok(frame) => {
                                    match frame {
                                        Frame::HandShake(frame) => {
                                            error!(?frame, "Received handshake frame");
                                            drop(frame);
                                            break;
                                        },
                                        Frame::Sv2(sv2_frame) => {
                                            trace!("Received inbound frame");
                                            if let Err(e) = inbound_tx.send(sv2_frame).await {
                                                inbound_tx.close();
                                                error!(error=?e, "Failed to forward inbound frame");
                                                break;
                                            }
                                        },
                                    }
                                }
                                Err(e) => {
                                    error!(error=?e, "Reader error");
                                    inbound_tx.close();
                                    break;
                                }
                            }
                        }
                    }
                }
                inbound_tx.close();
                outbound_rx_clone.close();
                drop(inbound_tx);
                drop(outbound_rx_clone);

                // signal fallback coordinator that this task has completed its cleanup
                fallback_handler.done();
                warn!("Reader task exited.");
            }
            .instrument(tracing::trace_span!(
                "reader_task",
                spawned_at = %format!("{}:{}", caller.file(), caller.line())
            )),
        );
    }

    {
        let fallback_coordinator_clone = fallback_coordinator.clone();
        task_manager.spawn(
            async move {
                // we just spawned a new task that's relevant to fallback coordination
                // so register it with the fallback coordinator
                let fallback_handler = fallback_coordinator_clone.register();

                // get the cancellation token that signals fallback
                let fallback_token = fallback_coordinator_clone.token();

                trace!("Writer task started");
                loop {
                    tokio::select! {
                        _ = cancellation_token.cancelled() => {
                            trace!("Received app shutdown signal");
                            inbound_tx_clone.close();
                            break;
                        }
                        _ = fallback_token.cancelled() => {
                            trace!("Received fallback signal");
                            inbound_tx_clone.close();
                            break;
                        }
                        res = outbound_rx.recv() => {
                            match res {
                                Ok(frame) => {
                                    trace!("Sending outbound frame");
                                    if let Err(e) = writer.write_frame(frame.into()).await {
                                        error!(error=?e, "Writer error");
                                        outbound_rx.close();
                                        break;
                                    }
                                }
                                Err(_) => {
                                    outbound_rx.close();
                                    warn!("Outbound channel closed");
                                    break;
                                }
                            }
                        }
                    }
                }
                outbound_rx.close();
                inbound_tx_clone.close();
                drop(outbound_rx);
                drop(inbound_tx_clone);

                // signal fallback coordinator that this task has completed its cleanup
                fallback_handler.done();
                warn!("Writer task exited.");
            }
            .instrument(tracing::trace_span!(
                "writer_task",
                spawned_at = %format!("{}:{}", caller.file(), caller.line())
            )),
        );
    }
}
