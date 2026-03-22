use async_channel::{Receiver, Sender};
use stratum_apps::{
    stratum_core::parsers_sv2::{Mining, Tlv},
    utils::types::Sv2Frame,
};
use tracing::debug;

use crate::status::Status;

#[derive(Clone, Debug)]
pub struct ChannelState {
    pub upstream_sender: Sender<Sv2Frame>,
    pub upstream_receiver: Receiver<Sv2Frame>,
    pub sv1_server_sender: Sender<(Mining<'static>, Option<Vec<Tlv>>)>,
    pub sv1_server_receiver: Receiver<(Mining<'static>, Option<Vec<Tlv>>)>,
    pub status_sender: Sender<Status>,
}

#[cfg_attr(not(test), hotpath::measure_all)]
impl ChannelState {
    pub fn new(
        upstream_sender: Sender<Sv2Frame>,
        upstream_receiver: Receiver<Sv2Frame>,
        sv1_server_sender: Sender<(Mining<'static>, Option<Vec<Tlv>>)>,
        sv1_server_receiver: Receiver<(Mining<'static>, Option<Vec<Tlv>>)>,
        status_sender: Sender<Status>,
    ) -> Self {
        Self {
            upstream_sender,
            upstream_receiver,
            sv1_server_sender,
            sv1_server_receiver,
            status_sender,
        }
    }

    pub fn drop(&self) {
        debug!("Dropping channel manager channels");
        self.upstream_receiver.close();
        self.upstream_sender.close();
        self.sv1_server_receiver.close();
        self.sv1_server_sender.close();
    }
}
