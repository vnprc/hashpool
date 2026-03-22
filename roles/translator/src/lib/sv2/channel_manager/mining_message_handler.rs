use crate::{
    error::{self, TproxyError, TproxyErrorKind},
    is_aggregated,
    sv2::ChannelManager,
    utils::{proxy_extranonce_prefix_len, AggregatedState, AGGREGATED_CHANNEL_ID},
};
use stratum_apps::{
    stratum_core::{
        bitcoin::Target,
        channels_sv2::client::{extended::ExtendedChannel, group::GroupChannel},
        handlers_sv2::{HandleMiningMessagesFromServerAsync, SupportedChannelTypes},
        mining_sv2::{
            CloseChannel, ExtendedExtranonce, Extranonce, NewExtendedMiningJob, NewMiningJob,
            OpenExtendedMiningChannelSuccess, OpenMiningChannelError,
            OpenStandardMiningChannelSuccess, SetCustomMiningJobError, SetCustomMiningJobSuccess,
            SetExtranoncePrefix, SetGroupChannel, SetNewPrevHash, SetTarget, SubmitSharesError,
            SubmitSharesSuccess, UpdateChannelError,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
            MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
        },
        parsers_sv2::{Mining, Tlv},
    },
    utils::types::{DownstreamId, Hashrate},
};
use tracing::{debug, error, info, warn};

#[cfg_attr(not(test), hotpath::measure_all)]
impl HandleMiningMessagesFromServerAsync for ChannelManager {
    type Error = TproxyError<error::ChannelManager>;

    fn get_channel_type_for_server(&self, _server_id: Option<usize>) -> SupportedChannelTypes {
        SupportedChannelTypes::GroupAndExtended
    }

    fn is_work_selection_enabled_for_server(&self, _server_id: Option<usize>) -> bool {
        false
    }

    fn get_negotiated_extensions_with_server(
        &self,
        _server_id: Option<usize>,
    ) -> Result<Vec<u16>, Self::Error> {
        Ok(self
            .negotiated_extensions
            .super_safe_lock(|data| data.clone()))
    }

    async fn handle_open_standard_mining_channel_success(
        &mut self,
        _server_id: Option<usize>,
        m: OpenStandardMiningChannelSuccess<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        warn!("Received: {}", m);
        Err(TproxyError::log(TproxyErrorKind::UnexpectedMessage(
            0,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
        )))
    }

    async fn handle_open_extended_mining_channel_success(
        &mut self,
        _server_id: Option<usize>,
        m: OpenExtendedMiningChannelSuccess<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        // Retrieve the pending channel request data.
        // Both aggregated and non-aggregated modes store data in pending_downstream_channels, keyed
        // by request_id, so the lookup is identical for both.
        let (user_identity, nominal_hashrate, downstream_extranonce_len) = self
            .pending_downstream_channels
            .remove(&(m.request_id as DownstreamId))
            .ok_or_else(|| {
                error!("No pending channel found for request_id: {}", m.request_id);
                TproxyError::log(TproxyErrorKind::PendingChannelNotFound(m.request_id))
            })?
            .1;

        let success = {
            info!(
                "Received: {}, user_identity: {}, nominal_hashrate: {}",
                m, user_identity, nominal_hashrate
            );

            let full_extranonce_size = m.extranonce_size as usize + m.extranonce_prefix.len();

            // add the channel to the group channel
            match self.group_channels.get_mut(&m.group_channel_id) {
                Some(mut group_channel) => {
                    group_channel
                        .add_channel_id(m.channel_id, full_extranonce_size)
                        .map_err(|e| {
                            error!("Failed to add channel id to group channel: {:?}", e);
                            TproxyError::fallback(
                                TproxyErrorKind::FailedToAddChannelIdToGroupChannel(e),
                            )
                        })?;
                }
                None => {
                    let mut group_channel = GroupChannel::new(m.group_channel_id);
                    group_channel
                        .add_channel_id(m.channel_id, full_extranonce_size)
                        .map_err(|e| {
                            error!("Failed to add channel id to group channel: {:?}", e);
                            TproxyError::fallback(
                                TproxyErrorKind::FailedToAddChannelIdToGroupChannel(e),
                            )
                        })?;
                    self.group_channels
                        .insert(m.group_channel_id, group_channel);
                }
            }

            let extranonce_prefix = m.extranonce_prefix.clone().into_static().to_vec();
            let target = Target::from_le_bytes(m.target.clone().inner_as_ref().try_into().unwrap());
            let version_rolling = true; // we assume this is always true on extended channels
            let extended_channel = ExtendedChannel::new(
                m.channel_id,
                user_identity.clone(),
                extranonce_prefix.clone(),
                target,
                nominal_hashrate,
                version_rolling,
                m.extranonce_size,
            );

            // If we are in aggregated mode, we need to create a new extranonce prefix and
            // insert the extended channel into the map
            if is_aggregated() {
                // Store the upstream extended channel under AGGREGATED_CHANNEL_ID
                self.extended_channels
                    .insert(AGGREGATED_CHANNEL_ID, extended_channel.clone());

                let upstream_extranonce_prefix: Extranonce = m.extranonce_prefix.clone().into();
                let translator_proxy_extranonce_prefix_len = proxy_extranonce_prefix_len(
                    m.extranonce_size.into(),
                    downstream_extranonce_len,
                );

                // range 0 is the extranonce_prefix from upstream
                // range 1 is the extranonce_prefix added by the tproxy
                // range 2 is the extranonce_size used by the miner for rolling
                let range_0 = 0..extranonce_prefix.len();
                let range1 = range_0.end..range_0.end + translator_proxy_extranonce_prefix_len;
                let range2 = range1.end..range1.end + downstream_extranonce_len;
                debug!(
                    "\n\nrange_0: {:?}, range1: {:?}, range2: {:?}\n\n",
                    range_0, range1, range2
                );
                let extended_extranonce_factory = ExtendedExtranonce::from_upstream_extranonce(
                    upstream_extranonce_prefix,
                    range_0,
                    range1,
                    range2,
                )
                .expect("Failed to create ExtendedExtranonce from upstream extranonce");
                self.extranonce_factories
                    .insert(AGGREGATED_CHANNEL_ID, extended_extranonce_factory);

                let mut factory = self
                    .extranonce_factories
                    .get_mut(&AGGREGATED_CHANNEL_ID)
                    .expect("extranonce_prefix_factory should be set after creation");
                let new_extranonce_size = factory.get_range2_len() as u16;
                let new_extranonce_prefix = factory
                    .next_prefix_extended(new_extranonce_size as usize)
                    .expect("next_prefix_extended should return a value for valid input")
                    .into_b032();
                let new_downstream_extended_channel = ExtendedChannel::new(
                    m.channel_id,
                    user_identity.clone(),
                    new_extranonce_prefix.clone().into_static().to_vec(),
                    target,
                    nominal_hashrate,
                    true,
                    new_extranonce_size,
                );
                self.extended_channels
                    .insert(m.channel_id, new_downstream_extended_channel);
                self.aggregated_channel_state
                    .set(AggregatedState::Connected);
                let new_open_extended_mining_channel_success = OpenExtendedMiningChannelSuccess {
                    request_id: m.request_id,
                    channel_id: m.channel_id,
                    extranonce_prefix: new_extranonce_prefix,
                    extranonce_size: new_extranonce_size,
                    target: m.target.clone(),
                    group_channel_id: m.group_channel_id,
                };
                Ok::<OpenExtendedMiningChannelSuccess<'static>, Self::Error>(
                    new_open_extended_mining_channel_success.into_static(),
                )
            } else {
                // Non-aggregated mode: check if we need to adjust extranonce size
                if m.extranonce_size as usize != downstream_extranonce_len {
                    // We need to create an extranonce factory to ensure proper extranonce2_size
                    let upstream_extranonce_prefix: Extranonce = m.extranonce_prefix.clone().into();
                    let translator_proxy_extranonce_prefix_len = proxy_extranonce_prefix_len(
                        m.extranonce_size.into(),
                        downstream_extranonce_len,
                    );

                    // range 0 is the extranonce1 from upstream
                    // range 1 is the extranonce1 added by the tproxy
                    // range 2 is the extranonce2 used by the miner for rolling
                    let range_0 = 0..extranonce_prefix.len();
                    let range1 = range_0.end..range_0.end + translator_proxy_extranonce_prefix_len;
                    let range2 = range1.end..range1.end + downstream_extranonce_len;
                    debug!(
                        "\n\nrange_0: {:?}, range1: {:?}, range2: {:?}\n\n",
                        range_0, range1, range2
                    );
                    // Create the factory - this should succeed if configuration is valid
                    let extended_extranonce_factory = ExtendedExtranonce::from_upstream_extranonce(
                            upstream_extranonce_prefix,
                            range_0,
                            range1,
                            range2,
                        )
                        .expect("Failed to create ExtendedExtranonce factory - likely extranonce size configuration issue");
                    // Store the factory for this specific channel
                    let mut factory = extended_extranonce_factory;
                    let new_extranonce_prefix = factory
                        .next_prefix_extended(downstream_extranonce_len)
                        .expect("Failed to generate extranonce prefix")
                        .into_b032();
                    // Create channel with the configured extranonce size
                    let new_downstream_extended_channel = ExtendedChannel::new(
                        m.channel_id,
                        user_identity.clone(),
                        new_extranonce_prefix.clone().into_static().to_vec(),
                        target,
                        nominal_hashrate,
                        true,
                        downstream_extranonce_len as u16,
                    );
                    self.extended_channels
                        .insert(m.channel_id, new_downstream_extended_channel);

                    self.extranonce_factories.insert(m.channel_id, factory);

                    let new_open_extended_mining_channel_success =
                        OpenExtendedMiningChannelSuccess {
                            request_id: m.request_id,
                            channel_id: m.channel_id,
                            extranonce_prefix: new_extranonce_prefix,
                            extranonce_size: downstream_extranonce_len as u16,
                            target: m.target.clone(),
                            group_channel_id: m.group_channel_id,
                        };
                    Ok::<OpenExtendedMiningChannelSuccess<'static>, Self::Error>(
                        new_open_extended_mining_channel_success.into_static(),
                    )
                } else {
                    // Extranonce size matches, use as-is
                    self.extended_channels
                        .insert(m.channel_id, extended_channel);
                    Ok::<OpenExtendedMiningChannelSuccess<'static>, Self::Error>(m.into_static())
                }
            }
        }?;

        self.channel_state
            .sv1_server_sender
            .send((
                Mining::OpenExtendedMiningChannelSuccess(success.clone()),
                None,
            ))
            .await
            .map_err(|e| {
                error!("Failed to send OpenExtendedMiningChannelSuccess: {:?}", e);
                TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender)
            })?;

        // In aggregated mode, serve any downstream requests that were buffered in
        // pending_channels while the upstream channel was being established (Pending state).
        if is_aggregated() {
            let pending_requests: Vec<(u32, String, Hashrate, usize)> = self
                .pending_downstream_channels
                .iter()
                .map(|r| {
                    (
                        *r.key() as u32,
                        r.value().0.clone(),
                        r.value().1,
                        r.value().2,
                    )
                })
                .collect();
            self.pending_downstream_channels.clear();

            for (req_id, user_identity, hashrate, min_extranonce_size) in pending_requests {
                self.handle_downstream_channel_request_in_aggregated_mode(
                    req_id,
                    user_identity,
                    hashrate,
                    min_extranonce_size,
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn handle_open_mining_channel_error(
        &mut self,
        _server_id: Option<usize>,
        m: OpenMiningChannelError<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        warn!("Received: {}", m);
        Err(TproxyError::fallback(
            TproxyErrorKind::OpenMiningChannelError,
        ))
    }

    async fn handle_update_channel_error(
        &mut self,
        _server_id: Option<usize>,
        m: UpdateChannelError<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        warn!("Received: {}", m);
        Ok(())
    }

    async fn handle_close_channel(
        &mut self,
        _server_id: Option<usize>,
        m: CloseChannel<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received: {}", m);
        // are we working in aggregated mode?
        if is_aggregated() {
            // even if aggregated channel_id != m.channel_id, we should trigger fallback
            // because why would a sane server send a CloseChannel message to a different
            // channel?
            return Err(TproxyError::fallback(
                TproxyErrorKind::AggregatedChannelClosed,
            ));
        }

        let group_channel = self.group_channels.remove(&m.channel_id);

        // we're not in aggregated mode
        // was the message sent to a group channel?
        if let Some((_, group_channel)) = group_channel {
            for channel_id in group_channel.get_channel_ids() {
                self.extended_channels.remove(channel_id);
            }
        // if the message was not sent to a group channel, and we're not working in
        // aggregated mode,
        } else if self.extended_channels.contains_key(&m.channel_id) {
            // remove the channel from the extended channels map
            self.extended_channels.remove(&m.channel_id);

            // remove the channel from any group channels that contain it
            for mut group_channel in self.group_channels.iter_mut() {
                if group_channel.get_channel_ids().contains(&m.channel_id) {
                    group_channel.remove_channel_id(m.channel_id);
                }
            }
        } else {
            error!(
                "Channel Id not found: {}, ignoring CloseChannel message",
                m.channel_id
            );
            return Err(TproxyError::log(TproxyErrorKind::ChannelNotFound));
        }

        Ok(())
    }

    async fn handle_set_extranonce_prefix(
        &mut self,
        _server_id: Option<usize>,
        m: SetExtranoncePrefix<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        warn!("Received: {}", m);
        warn!("⚠️ Cannot process SetExtranoncePrefix since set_extranonce is not supported for majority of sv1 clients. Ignoring.");
        Ok(())
    }

    async fn handle_submit_shares_success(
        &mut self,
        _server_id: Option<usize>,
        m: SubmitSharesSuccess,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received: {} ✅", m);

        // In aggregated mode, the Pool responds with the upstream channel ID, but the
        // channel is stored under AGGREGATED_CHANNEL_ID in the DashMap.
        // In non-aggregated mode, m.channel_id matches the DashMap key directly.
        let key = if is_aggregated() {
            AGGREGATED_CHANNEL_ID
        } else {
            m.channel_id
        };

        if let Some(mut ch) = self.extended_channels.get_mut(&key) {
            ch.on_share_acknowledgement(m.new_submits_accepted_count, m.new_shares_sum as f64);
        }

        Ok(())
    }

    async fn handle_submit_shares_error(
        &mut self,
        _server_id: Option<usize>,
        m: SubmitSharesError<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        warn!("Received: {} ❌", m);
        Ok(())
    }

    async fn handle_new_mining_job(
        &mut self,
        _server_id: Option<usize>,
        m: NewMiningJob<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        warn!("Received: {}", m);
        warn!("⚠️ Cannot process NewMiningJob since Translator Proxy supports only extended mining jobs. Ignoring.");
        Ok(())
    }

    async fn handle_new_extended_mining_job(
        &mut self,
        _server_id: Option<usize>,
        m: NewExtendedMiningJob<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received: {}", m);
        let m_static = m.clone().into_static();

        // we update the channel states and keep track of the messages that need to be sent to the
        // SV1Server
        let new_extended_mining_job_messages_sv1_server = {
            let mut new_extended_mining_job_messages = Vec::new();

            // are we in aggregated mode?
            if is_aggregated() {
                // Validate that the message is for the aggregated channel or its group
                let aggregated_channel_id = self
                    .extended_channels
                    .get(&AGGREGATED_CHANNEL_ID)
                    .ok_or(TproxyError::fallback(TproxyErrorKind::ChannelNotFound))?
                    .get_channel_id();

                // here, we are assuming that since we are in aggregated mode, there should
                // be only one single group channel and the
                // aggregated channel must belong to it
                let group_channel = self.group_channels.iter().next();
                let Some(group_channel) = group_channel else {
                    error!("Aggregated channel does not belong to any group channel");
                    return Err(TproxyError::fallback(TproxyErrorKind::ChannelNotFound));
                };

                let group_channel_id = group_channel.get_group_channel_id();

                // was the message sent to the aggregated channel?
                if aggregated_channel_id == m_static.channel_id
                    || group_channel_id == m_static.channel_id
                {
                    // update all extended channel states
                    for mut extended_channel in self.extended_channels.iter_mut() {
                        extended_channel
                            .on_new_extended_mining_job(m_static.clone())
                            .map_err(|e| {
                                error!("Failed to process new extended mining job: {:?}", e);
                                TproxyError::fallback(
                                    TproxyErrorKind::FailedToProcessNewExtendedMiningJob,
                                )
                            })?;
                    }

                    // only send this message to the SV1Server if it's not a future job
                    if !m_static.is_future() {
                        let mut new_extended_mining_job_message = m_static.clone();
                        new_extended_mining_job_message.channel_id = AGGREGATED_CHANNEL_ID; // this is done so that every aggregated downstream
                                                                                            // will receive the NewExtendedMiningJob message
                        new_extended_mining_job_messages.push(new_extended_mining_job_message);
                    }
                } else {
                    // we got a nonsense channel id, we should log an error and ignore the
                    // message
                    error!(
                        "Channel not found: {}, ignoring NewExtendedMiningJob message",
                        m_static.channel_id
                    );
                    return Err(TproxyError::log(TproxyErrorKind::ChannelNotFound));
                }
            // we're not in aggregated mode
            // was the message sent to a group channel?
            } else if let Some(mut group_channel) = self.group_channels.get_mut(&m.channel_id) {
                // update group channel state
                group_channel.on_new_extended_mining_job(m_static.clone());

                // process the message for each individual channel on the group
                for channel_id in group_channel.get_channel_ids() {
                    let mut channel = self
                        .extended_channels
                        .get_mut(channel_id)
                        .ok_or(TproxyError::fallback(TproxyErrorKind::ChannelNotFound))?;

                    let mut job = m_static.clone();
                    job.channel_id = *channel_id;

                    // update each channel state
                    channel
                        .on_new_extended_mining_job(job.clone())
                        .map_err(|e| {
                            error!("Failed to process new extended mining job: {:?}", e);
                            TproxyError::fallback(
                                TproxyErrorKind::FailedToProcessNewExtendedMiningJob,
                            )
                        })?;

                    // only send this message to the SV1Server if it's not a future job
                    if !job.is_future() {
                        new_extended_mining_job_messages.push(job);
                    }
                }
            // if the message was not sent to a group channel, we need to check if we're
            // working in aggregated mode
            } else {
                let Some(mut channel) = self.extended_channels.get_mut(&m_static.channel_id) else {
                    // we got a nonsense channel id, we should log an error and ignore the
                    // message
                    error!(
                        "Channel not found: {}, ignoring NewExtendedMiningJob message",
                        m_static.channel_id
                    );
                    return Err(TproxyError::log(TproxyErrorKind::ChannelNotFound));
                };

                // update channel state
                channel
                    .on_new_extended_mining_job(m_static.clone())
                    .map_err(|e| {
                        error!("Failed to process new extended mining job: {:?}", e);
                        TproxyError::fallback(TproxyErrorKind::FailedToProcessNewExtendedMiningJob)
                    })?;

                // only send this message to the SV1Server if it's not a future job
                if !m_static.is_future() {
                    let new_extended_mining_job_message = m_static.clone();
                    new_extended_mining_job_messages.push(new_extended_mining_job_message);
                }
            }
            Ok::<Vec<NewExtendedMiningJob<'static>>, Self::Error>(new_extended_mining_job_messages)
        }?;

        // now we need to send the NewExtendedMiningJob message(s) to the SV1Server
        for message in new_extended_mining_job_messages_sv1_server {
            self.channel_state
                .sv1_server_sender
                .send((Mining::NewExtendedMiningJob(message), None))
                .await
                .map_err(|e| {
                    error!("Failed to send immediate NewExtendedMiningJob: {:?}", e);
                    TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender)
                })?;
        }
        Ok(())
    }

    async fn handle_set_new_prev_hash(
        &mut self,
        _server_id: Option<usize>,
        m: SetNewPrevHash<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received: {}", m);
        let mut m_static = m.clone().into_static();

        // we update the channel states and keep track of the messages that need to be sent to the
        // SV1Server
        let (set_new_prev_hash_messages_sv1_server, new_extended_mining_job_messages_sv1_server) =
            {
                let mut set_new_prev_hash_messages = Vec::new();
                let mut new_extended_mining_job_messages = Vec::new();

                if is_aggregated() {
                    // Validate that the message is for the aggregated channel or its group
                    let aggregated_channel_id = self
                        .extended_channels
                        .get(&AGGREGATED_CHANNEL_ID)
                        .ok_or(TproxyError::fallback(TproxyErrorKind::ChannelNotFound))?
                        .get_channel_id();

                    // does aggregated channel belong to some group channel?
                    // here, we are assuming that since we are in aggregated mode, there
                    // should be only one single group channel
                    // and the aggregated channel must belong to it
                    let group_channel = self.group_channels.iter().next();
                    let Some(group_channel) = group_channel else {
                        error!("Aggregated channel does not belong to any group channel");
                        return Err(TproxyError::fallback(TproxyErrorKind::ChannelNotFound));
                    };

                    let group_channel_id = group_channel.get_group_channel_id();

                    // was the message sent to the aggregated channel?
                    if aggregated_channel_id == m.channel_id || group_channel_id == m.channel_id {
                        // update all extended channel states
                        for mut extended_channel in self.extended_channels.iter_mut() {
                            extended_channel
                                .on_set_new_prev_hash(m_static.clone())
                                .map_err(|e| {
                                    error!("Failed to set new prev hash: {:?}", e);
                                    TproxyError::fallback(
                                        TproxyErrorKind::FailedToProcessSetNewPrevHash,
                                    )
                                })?;
                        }

                        // make sure the SetNewPrevHash message is sent to the aggregated
                        // channel
                        m_static.channel_id = AGGREGATED_CHANNEL_ID;
                        set_new_prev_hash_messages.push(m_static.clone());

                        // for the aggregated channel, send one NewExtendedMiningJob message
                        // to the SV1Server (get active job after updating all channels)
                        let mut new_extended_mining_job_message = self
                            .extended_channels
                            .get(&AGGREGATED_CHANNEL_ID)
                            .expect("aggregated channel must exist")
                            .get_active_job()
                            .expect("active job must exist")
                            .clone();
                        new_extended_mining_job_message.0.channel_id = AGGREGATED_CHANNEL_ID;
                        new_extended_mining_job_messages.push(new_extended_mining_job_message.0);
                    } else {
                        // we got a nonsense channel id, we should log an error and ignore
                        // the message
                        warn!(
                            "Channel not found: {}, ignoring SetNewPrevHash message",
                            m_static.channel_id
                        );
                        return Err(TproxyError::log(TproxyErrorKind::ChannelNotFound));
                    }
                // we are not in aggregated mode.. was the message sent to a group channel?
                } else if let Some(mut group_channel) = self.group_channels.get_mut(&m.channel_id) {
                    // update group channel state
                    group_channel
                        .on_set_new_prev_hash(m_static.clone())
                        .map_err(|e| {
                            error!("Failed to set new prev hash: {:?}", e);
                            TproxyError::fallback(TproxyErrorKind::FailedToProcessSetNewPrevHash)
                        })?;

                    // there's no aggregated channel, so we need to process the message for each
                    // individual channel on the group
                    for channel_id in group_channel.get_channel_ids() {
                        let mut channel = self
                            .extended_channels
                            .get_mut(channel_id)
                            .ok_or(TproxyError::fallback(TproxyErrorKind::ChannelNotFound))?;

                        channel
                            .on_set_new_prev_hash(m_static.clone())
                            .map_err(|e| {
                                error!("Failed to set new prev hash: {:?}", e);
                                TproxyError::fallback(
                                    TproxyErrorKind::FailedToProcessSetNewPrevHash,
                                )
                            })?;

                        // for each extended channel, send one SetNewPrevHash message to the
                        // SV1Server
                        let mut set_new_prev_hash_message = m_static.clone();
                        set_new_prev_hash_message.channel_id = *channel_id;
                        set_new_prev_hash_messages.push(set_new_prev_hash_message);

                        // for each extended channel, send one NewExtendedMiningJob message to
                        // the SV1Server
                        let new_extended_mining_job_message = channel
                            .get_active_job()
                            .expect("active job must exist")
                            .clone();
                        new_extended_mining_job_messages.push(new_extended_mining_job_message.0);
                    }
                // if the message was not sent to a group channel, and we're not in aggregated
                // mode, we need to process the message for a specific channel
                } else {
                    let Some(mut channel) = self.extended_channels.get_mut(&m_static.channel_id)
                    else {
                        // we got a nonsense channel id, we should log an error and ignore the
                        // message
                        warn!(
                            "Channel not found: {}, ignoring SetNewPrevHash message",
                            m_static.channel_id
                        );
                        return Err(TproxyError::log(TproxyErrorKind::ChannelNotFound));
                    };

                    // update channel state
                    channel
                        .on_set_new_prev_hash(m_static.clone())
                        .map_err(|e| {
                            error!("Failed to set new prev hash: {:?}", e);
                            TproxyError::fallback(TproxyErrorKind::FailedToProcessSetNewPrevHash)
                        })?;

                    // make sure the SetNewPrevHash message is sent to the channel
                    set_new_prev_hash_messages.push(m_static.clone());

                    // for the channel, send one NewExtendedMiningJob message to the SV1Server
                    let new_extended_mining_job_message = channel
                        .get_active_job()
                        .expect("active job must exist")
                        .clone();
                    new_extended_mining_job_messages.push(new_extended_mining_job_message.0);
                }
                Ok::<
                    (
                        Vec<SetNewPrevHash<'static>>,
                        Vec<NewExtendedMiningJob<'static>>,
                    ),
                    Self::Error,
                >((set_new_prev_hash_messages, new_extended_mining_job_messages))
            }?;

        // we need to send the SetNewPrevHash message(s) to the SV1Server
        for message in set_new_prev_hash_messages_sv1_server {
            self.channel_state
                .sv1_server_sender
                .send((Mining::SetNewPrevHash(message), None))
                .await
                .map_err(|e| {
                    error!("Failed to send SetNewPrevHash: {:?}", e);
                    TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender)
                })?;
        }

        // we need to send the NewExtendedMiningJob message(s) to the SV1Server
        for message in new_extended_mining_job_messages_sv1_server {
            self.channel_state
                .sv1_server_sender
                .send((Mining::NewExtendedMiningJob(message), None))
                .await
                .map_err(|e| {
                    error!("Failed to send NewExtendedMiningJob: {:?}", e);
                    TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender)
                })?;
        }

        Ok(())
    }

    async fn handle_set_custom_mining_job_success(
        &mut self,
        _server_id: Option<usize>,
        m: SetCustomMiningJobSuccess,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        warn!("Received: {}", m);
        warn!("⚠️ Cannot process SetCustomMiningJobSuccess since Translator Proxy does not support custom mining jobs. Ignoring.");
        Err(TproxyError::log(TproxyErrorKind::UnexpectedMessage(
            0,
            MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
        )))
    }

    async fn handle_set_custom_mining_job_error(
        &mut self,
        _server_id: Option<usize>,
        m: SetCustomMiningJobError<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        warn!("Received: {}", m);
        warn!("⚠️ Cannot process SetCustomMiningJobError since Translator Proxy does not support custom mining jobs. Ignoring.");
        Err(TproxyError::log(TproxyErrorKind::UnexpectedMessage(
            0,
            MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR,
        )))
    }

    async fn handle_set_target(
        &mut self,
        _server_id: Option<usize>,
        m: SetTarget<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received: {}", m);

        let m_static = m.clone().into_static();

        // Update the channel targets in the channel manager
        let set_target_messages_sv1_server = {
            let mut set_target_messages = Vec::new();

            // are in aggregated mode?
            if is_aggregated() {
                let aggregated_channel_id = self
                    .extended_channels
                    .get(&AGGREGATED_CHANNEL_ID)
                    .ok_or(TproxyError::fallback(TproxyErrorKind::ChannelNotFound))?
                    .get_channel_id();

                // does aggregated channel belong to some group channel?
                // here, we are assuming that since we are in aggregated mode, there should
                // be only one single group channel and the
                // aggregated channel must belong to it
                let group_channel = self.group_channels.iter().next();
                let Some(group_channel) = group_channel else {
                    error!("Aggregated channel does not belong to any group channel");
                    return Err(TproxyError::fallback(TproxyErrorKind::ChannelNotFound));
                };

                let group_channel_id = group_channel.get_group_channel_id();

                // was the message sent to the aggregated channel?
                if aggregated_channel_id == m.channel_id || group_channel_id == m.channel_id {
                    // Update target for all extended channels (including AGGREGATED_CHANNEL_ID)
                    self.extended_channels.iter_mut().for_each(|mut channel| {
                        channel.set_target(Target::from_le_bytes(
                            m.maximum_target
                                .inner_as_ref()
                                .try_into()
                                .expect("target deserialization should never fail"),
                        ));
                    });

                    let mut message = m_static.clone();
                    message.channel_id = AGGREGATED_CHANNEL_ID;
                    set_target_messages.push(message);
                } else {
                    // we got a nonsense channel id, we should log an error and ignore the
                    // message
                    warn!(
                        "Channel not found: {}, ignoring SetTarget message",
                        m_static.channel_id
                    );
                    return Err(TproxyError::log(TproxyErrorKind::ChannelNotFound));
                }

            // we are not in aggregated mode... was the message sent to a group channel?
            } else if let Some(group_channel) = self.group_channels.get(&m.channel_id) {
                // process the message for each individual channel on the group
                for channel_id in group_channel.get_channel_ids() {
                    let mut channel = self
                        .extended_channels
                        .get_mut(channel_id)
                        .ok_or(TproxyError::fallback(TproxyErrorKind::ChannelNotFound))?;

                    channel.set_target(Target::from_le_bytes(
                        m.maximum_target
                            .inner_as_ref()
                            .try_into()
                            .expect("target deserialization should never fail"),
                    ));

                    let mut message = m_static.clone();
                    message.channel_id = *channel_id;
                    set_target_messages.push(message);
                }
            // if the message was not sent to a group channel, and we're not in aggregated
            // mode, we need to process the message for a specific channel
            } else {
                let Some(mut channel) = self.extended_channels.get_mut(&m.channel_id) else {
                    // we got a nonsense channel id, we should log an error and ignore the
                    // message
                    warn!(
                        "Channel not found: {}, ignoring SetTarget message",
                        m_static.channel_id
                    );
                    return Err(TproxyError::log(TproxyErrorKind::ChannelNotFound));
                };

                channel.set_target(Target::from_le_bytes(
                    m.maximum_target
                        .inner_as_ref()
                        .try_into()
                        .expect("target deserialization should never fail"),
                ));

                set_target_messages.push(m_static.clone());
            }

            Ok::<Vec<SetTarget<'static>>, Self::Error>(set_target_messages)
        }?;

        // now we need to send the SetTarget message(s) to the SV1Server
        for message in set_target_messages_sv1_server {
            self.channel_state
                .sv1_server_sender
                .send((Mining::SetTarget(message), None))
                .await
                .map_err(|e| {
                    error!("Failed to send SetTarget: {:?}", e);
                    TproxyError::shutdown(TproxyErrorKind::ChannelErrorSender)
                })?;
        }

        Ok(())
    }

    async fn handle_set_group_channel(
        &mut self,
        _server_id: Option<usize>,
        m: SetGroupChannel<'_>,
        _tlv_fields: Option<&[Tlv]>,
    ) -> Result<(), Self::Error> {
        info!("Received: {}", m);

        // remove every channel from any group channels that end up empty
        let mut group_channels_to_remove = Vec::new();

        // check every group channel if it contains any of the channels in the new group
        // channel
        for mut channel in self.group_channels.iter_mut() {
            let group_channel_id = *channel.key();
            let group_channel = channel.value_mut();

            let channel_ids_to_remove = m.channel_ids.clone().into_inner();
            for channel_id in channel_ids_to_remove {
                group_channel.remove_channel_id(channel_id);
            }

            if group_channel.get_channel_ids().is_empty() {
                group_channels_to_remove.push(group_channel_id);
            }
        }

        // Now remove the empty group channels
        for group_channel_id in group_channels_to_remove {
            self.group_channels.remove(&group_channel_id);
        }

        // does the group channel already exist?
        match self.group_channels.get_mut(&m.group_channel_id) {
            // if yes, clean up any channels that are no longer in the new group channel
            Some(mut group_channel) => {
                let current_channel_ids = group_channel.get_channel_ids().clone();
                let new_channel_ids = m.channel_ids.clone().into_inner();

                // Remove channels that are no longer in the new list
                for channel_id in &current_channel_ids {
                    if !new_channel_ids.contains(channel_id) {
                        group_channel.remove_channel_id(*channel_id);
                    }
                }

                // Add all channels from the message (inner HashSet ingores duplicates)
                for channel_id in new_channel_ids {
                    let extended_channel = self
                        .extended_channels
                        .get(&channel_id)
                        .ok_or_else(|| TproxyError::fallback(TproxyErrorKind::ChannelNotFound))?;

                    let full_extranonce_size = extended_channel.get_full_extranonce_size();
                    group_channel
                        .add_channel_id(channel_id, full_extranonce_size)
                        .map_err(|e| {
                            error!("Failed to add channel id to group channel: {:?}", e);
                            TproxyError::fallback(
                                TproxyErrorKind::FailedToAddChannelIdToGroupChannel(e),
                            )
                        })?;
                }
            }
            // if no, create a new group channel, and add all the channels to it
            None => {
                let mut group_channel = GroupChannel::new(m.group_channel_id);

                // Add all channels to the newly created group channel
                for channel_id in m.channel_ids.clone().into_inner() {
                    let extended_channel = self
                        .extended_channels
                        .get(&channel_id)
                        .ok_or_else(|| TproxyError::fallback(TproxyErrorKind::ChannelNotFound))?;

                    let full_extranonce_size = extended_channel.get_full_extranonce_size();

                    group_channel
                        .add_channel_id(channel_id, full_extranonce_size)
                        .map_err(|e| {
                            error!("Failed to add channel id to group channel: {:?}", e);
                            TproxyError::fallback(
                                TproxyErrorKind::FailedToAddChannelIdToGroupChannel(e),
                            )
                        })?;
                }

                self.group_channels
                    .insert(m.group_channel_id, group_channel);
            }
        }

        Ok(())
    }
}
