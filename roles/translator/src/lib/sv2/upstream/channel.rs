use async_channel::{Receiver, Sender};
use stratum_apps::utils::types::Sv2Frame;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct UpstreamChannelState {
    /// Receiver for the SV2 Upstream role
    pub upstream_receiver: Receiver<Sv2Frame>,
    /// Sender for the SV2 Upstream role
    pub upstream_sender: Sender<Sv2Frame>,
    /// Sender for the ChannelManager to send SV2 frames
    pub channel_manager_sender: Sender<Sv2Frame>,
    /// Receiver for the ChannelManager to receive SV2 frames
    pub channel_manager_receiver: Receiver<Sv2Frame>,
}

#[cfg_attr(not(test), hotpath::measure_all)]
impl UpstreamChannelState {
    pub fn new(
        upstream_receiver: Receiver<Sv2Frame>,
        upstream_sender: Sender<Sv2Frame>,
        channel_manager_sender: Sender<Sv2Frame>,
        channel_manager_receiver: Receiver<Sv2Frame>,
    ) -> Self {
        Self {
            upstream_receiver,
            upstream_sender,
            channel_manager_sender,
            channel_manager_receiver,
        }
    }

    pub fn drop(&self) {
        debug!("Closing all upstream channels");
        self.upstream_receiver.close();
        self.upstream_receiver.close();
    }
}
