use super::super::{mining_pool::Downstream, stats_client::StatsMessage};
use binary_sv2::Str0255;
use mining_sv2::MintQuoteNotification;
use mint_pool_messaging::MintQuoteResponseEvent;
use roles_logic_sv2::{
    errors::Error,
    handlers::mining::{ParseDownstreamMiningMessages, SendTo, SupportedChannelTypes},
    mining_sv2::*,
    parsers::Mining,
    routing_logic::NoRouting,
    selectors::NullDownstreamMiningSelector,
    template_distribution_sv2::SubmitSolution,
    utils::Mutex,
};
use std::{
    convert::{TryFrom, TryInto},
    sync::Arc,
};
use tracing::{debug, error, info, warn};

fn share_error_code(err: &roles_logic_sv2::Error) -> &'static str {
    use roles_logic_sv2::Error;

    match err {
        Error::ShareDoNotMatchAnyChannel
        | Error::NotFoundChannelId
        | Error::NoGroupIdOnExtendedChannel => SubmitSharesError::invalid_channel_error_code(),
        Error::ShareDoNotMatchAnyJob
        | Error::PrevHashRequireNonExistentJobId(_)
        | Error::JobNotUpdated(_, _)
        | Error::NoValidJob
        | Error::NoValidTranslatorJob
        | Error::NoTemplateForId
        | Error::NoValidTemplate(_)
        | Error::JDSMissingTransactions => SubmitSharesError::invalid_job_id_error_code(),
        Error::TargetError(_)
        | Error::HashrateError(_)
        | Error::ValueRemainingNotUpdated
        | Error::ImpossibleToCalculateMerkleRoot
        | Error::InvalidCoinbase => SubmitSharesError::difficulty_too_low_error_code(),
        _ => SubmitSharesError::stale_share_error_code(),
    }
}

fn build_submit_share_error(
    channel_id: u32,
    sequence_number: u32,
    err: &roles_logic_sv2::Error,
) -> SubmitSharesError<'static> {
    let code = share_error_code(err);
    let error_code =
        Str0255::try_from(String::from(code)).expect("predefined error codes must fit in Str0255");

    SubmitSharesError {
        channel_id,
        sequence_number,
        error_code,
    }
}

/// Handle mint quote response received from mint
/// This function sends an extension message to the downstream with the quote
pub async fn handle_mint_quote_response(
    pool: Arc<Mutex<super::Pool>>,
    event: MintQuoteResponseEvent,
) {
    let quote_id_str =
        std::str::from_utf8(event.response.quote_id.inner_as_ref()).unwrap_or("invalid_utf8");

    info!(
        "✅ Received mint quote response: quote_id={} share_hash={}",
        quote_id_str, event.share_hash
    );

    let Some(context) = event.context.clone() else {
        warn!(
            "No pending context available for mint quote response share_hash={}",
            event.share_hash
        );
        return;
    };

    let notification = MintQuoteNotification {
        channel_id: context.channel_id,
        sequence_number: context.sequence_number,
        share_hash: event.response.header_hash.clone(),
        quote_id: event.response.quote_id.clone(),
        amount: context.amount,
    };

    if let Err(e) = super::Pool::send_extension_message_to_downstream(
        pool.clone(),
        context.channel_id,
        notification,
    )
    .await
    {
        error!("Failed to send mint quote notification: {}", e);
    } else {
        info!(
            "Sent mint quote notification for channel {} seq {}",
            context.channel_id, context.sequence_number
        );
    }
}

impl ParseDownstreamMiningMessages<(), NullDownstreamMiningSelector, NoRouting> for Downstream {
    fn get_channel_type(&self) -> SupportedChannelTypes {
        SupportedChannelTypes::GroupAndExtended
    }

    fn is_work_selection_enabled(&self) -> bool {
        true
    }

    #[cfg(feature = "MG_reject_auth")]
    fn is_downstream_authorized(
        _self_mutex: Arc<Mutex<Self>>,
        _user_identity: &binary_sv2::Str0255,
    ) -> Result<bool, Error> {
        Ok(false)
    }

    fn handle_open_standard_mining_channel(
        &mut self,
        incoming: OpenStandardMiningChannel,
        _m: Option<Arc<Mutex<()>>>,
    ) -> Result<SendTo<()>, Error> {
        let header_only = self.downstream_data.header_only;

        // Use a fixed hashrate to prevent DOS and ensure consistent difficulty
        // TODO: Move this to pool config file as 'fixed_minimum_hashrate'
        let fixed_low_hashrate = 10_000_000_000_000.0; // 10 TH/s - ~30 leading zeros

        let reposnses = self
            .channel_factory
            .safe_lock(|factory| {
                match factory.add_standard_channel(
                    incoming.request_id.as_u32(),
                    fixed_low_hashrate, // Use fixed rate instead of incoming.nominal_hash_rate
                    header_only,
                    self.id,
                ) {
                    Ok(msgs) => {
                        let mut res = vec![];
                        for msg in msgs {
                            res.push(msg.into_static());
                        }
                        Ok(res)
                    }
                    Err(e) => Err(e),
                }
            })
            .map_err(|e| roles_logic_sv2::Error::PoisonLock(e.to_string()))??;

        // Extract channel_id from the OpenStandardMiningChannelSuccess response and add to mapping
        let mut result = vec![];
        for response in reposnses {
            if let Mining::OpenStandardMiningChannelSuccess(ref success) = response {
                // Add mapping from channel_id to downstream_id
                if let Ok(_) = self.pool.safe_lock(|p| {
                    p.channel_to_downstream.insert(success.channel_id, self.id);
                    debug!(
                        "Added channel mapping: channel_id {} -> downstream_id {}",
                        success.channel_id, self.id
                    );
                }) {
                    // Send stats update for new channel
                    if let Ok(Some(stats_handle)) = self.pool.safe_lock(|p| p.stats_handle.clone()) {
                        stats_handle.send_stats(StatsMessage::ChannelOpened {
                            downstream_id: self.id,
                            channel_id: success.channel_id,
                        });
                    }
                } else {
                    error!(
                        "Failed to add channel mapping for channel_id: {}",
                        success.channel_id
                    );
                }
            }
            result.push(SendTo::Respond(response.into_static()))
        }
        Ok(SendTo::Multiple(result))
    }

    fn handle_open_extended_mining_channel(
        &mut self,
        m: OpenExtendedMiningChannel,
    ) -> Result<SendTo<()>, Error> {
        let request_id = m.request_id;
        // Use fixed hashrate for extended channels too
        // TODO: Move this to pool config file as 'fixed_minimum_hashrate'
        let hash_rate = 10_000_000_000_000.0; // 10 TH/s - consistent with standard channels
        let min_extranonce_size = m.min_extranonce_size;
        let messages_res = self
            .channel_factory
            .safe_lock(|s| s.new_extended_channel(request_id, hash_rate, min_extranonce_size))
            .map_err(|e| roles_logic_sv2::Error::PoisonLock(e.to_string()))?;
        match messages_res {
            Ok(messages) => {
                let mut result = vec![];
                for message in messages {
                    // Extract channel_id from OpenExtendedMiningChannelSuccess and add to mapping
                    if let Mining::OpenExtendedMiningChannelSuccess(ref success) = message {
                        // Add mapping from channel_id to downstream_id
                        if let Ok(_) = self.pool.safe_lock(|p| {
                            p.channel_to_downstream.insert(success.channel_id, self.id);
                            debug!(
                                "Added extended channel mapping: channel_id {} -> downstream_id {}",
                                success.channel_id, self.id
                            );
                        }) {
                            // Send stats update for new extended channel
                            if let Ok(Some(stats_handle)) =
                                self.pool.safe_lock(|p| p.stats_handle.clone())
                            {
                                stats_handle.send_stats(StatsMessage::ChannelOpened {
                                    downstream_id: self.id,
                                    channel_id: success.channel_id,
                                });
                            }
                        } else {
                            error!(
                                "Failed to add extended channel mapping for channel_id: {}",
                                success.channel_id
                            );
                        }
                    }
                    result.push(SendTo::Respond(message));
                }
                Ok(SendTo::Multiple(result))
            }
            Err(_) => Err(roles_logic_sv2::Error::ChannelIsNeitherExtendedNeitherInAPool),
        }
    }

    fn handle_update_channel(&mut self, m: UpdateChannel) -> Result<SendTo<()>, Error> {
        // Still track the reported hashrate for monitoring purposes
        let maximum_target =
            roles_logic_sv2::utils::hash_rate_to_target(m.nominal_hash_rate.into(), 10.0)?;
        self.channel_factory
            .safe_lock(|s| s.update_target_for_channel(m.channel_id, maximum_target.clone().into()))
            .unwrap_or_else(|_| {
                std::process::exit(1);
            });

        // TODO: Implement progressive fee structure based on share difficulty
        // Higher difficulty shares should receive lower fees to incentivize
        // miners to submit fewer, higher-quality shares. This reduces network
        // overhead and allows for better pool scalability.
        //
        // Example fee structure:
        // - Difficulty < 1K: 3% fee
        // - Difficulty 1K-10K: 2% fee
        // - Difficulty 10K-100K: 1% fee
        // - Difficulty > 100K: 0.5% fee

        // Use a fixed higher difficulty to prevent DOS - approximately 30 leading zeros
        // TODO: Move this to pool config file as 'fixed_minimum_hashrate'
        let fixed_low_target =
            roles_logic_sv2::utils::hash_rate_to_target(10_000_000_000_000.0, 10.0)?;
        let set_target = SetTarget {
            channel_id: m.channel_id,
            maximum_target: fixed_low_target,
        };
        Ok(SendTo::Respond(Mining::SetTarget(set_target)))
    }

    fn handle_submit_shares_standard(
        &mut self,
        m: SubmitSharesStandard,
    ) -> Result<SendTo<()>, Error> {
        let res = self
            .channel_factory
            .safe_lock(|cf| cf.on_submit_shares_standard(m.clone()))
            .map_err(|e| roles_logic_sv2::Error::PoisonLock(e.to_string()))?;
        match res {
            Ok(res) => match res  {
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::SendErrorDownstream(m) => {
                    Ok(SendTo::Respond(Mining::SubmitSharesError(m)))
                }
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::SendSubmitShareUpstream(_) => unreachable!(),
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::RelaySubmitShareUpstream => unreachable!(),
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::ShareMeetBitcoinTarget((share,t_id,coinbase,_)) => {
                    if let Some(template_id) = t_id {
                        let solution = SubmitSolution {
                            template_id,
                            version: share.get_version(),
                            header_timestamp: share.get_n_time(),
                            header_nonce: share.get_nonce(),
                            coinbase_tx: coinbase.try_into()?,
                        };
                        // TODO we can block everything with the below (looks like this will infinite loop??)
                        while self.solution_sender.try_send(solution.clone()).is_err() {};
                    }
                    let success = SubmitSharesSuccess {
                        channel_id: m.channel_id,
                        last_sequence_number: m.sequence_number,
                        new_submits_accepted_count: 1,
                        new_shares_sum: 0,
                        // initialize to all zeros, will be updated later
                        hash: [0u8; 32].into(),
                    };

                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))

                },
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::ShareMeetDownstreamTarget => {
                 let success = SubmitSharesSuccess {
                        channel_id: m.channel_id,
                        last_sequence_number: m.sequence_number,
                        new_submits_accepted_count: 1,
                        new_shares_sum: 0,
                        // initialize to all zeros, will be updated later
                        hash: [0u8; 32].into(),
                    };
                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
                },
            },
            Err(err) => {
                warn!(
                    ?err,
                    channel_id = m.channel_id,
                    sequence_number = m.sequence_number,
                    "Rejecting submit_shares_standard due to channel factory error"
                );
                let submit_error = build_submit_share_error(m.channel_id, m.sequence_number, &err);
                Ok(SendTo::Respond(Mining::SubmitSharesError(submit_error)))
            }
        }
    }

    fn handle_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtended<'_>,
    ) -> Result<SendTo<()>, Error> {
        let res = self
            .channel_factory
            .safe_lock(|cf| cf.on_submit_shares_extended(m.clone()))
            .map_err(|e| roles_logic_sv2::Error::PoisonLock(e.to_string()))?;
        match res {
            Ok(res) => match res  {
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::SendErrorDownstream(m) => {
                    Ok(SendTo::Respond(Mining::SubmitSharesError(m)))
                }
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::SendSubmitShareUpstream(_) => unreachable!(),
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::RelaySubmitShareUpstream => unreachable!(),
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::ShareMeetBitcoinTarget((share,t_id,coinbase,_)) => {
                    // Send share submitted stats - never blocks
                    if let Ok(Some(stats_handle)) = self.pool.safe_lock(|p| p.stats_handle.clone()) {
                        stats_handle.send_stats(StatsMessage::ShareSubmitted {
                            downstream_id: self.id,
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64,
                        });
                    }

                    if let Some(template_id) = t_id {
                        let solution = SubmitSolution {
                            template_id,
                            version: share.get_version(),
                            header_timestamp: share.get_n_time(),
                            header_nonce: share.get_nonce(),
                            coinbase_tx: coinbase.try_into()?,
                        };
                        // TODO we can block everything with the below (looks like this will infinite loop??)
                        while self.solution_sender.try_send(solution.clone()).is_err() {};
                    }

                    // Submit quote via dispatcher
                    self.quote_dispatcher.submit_quote(
                        m.hash.inner_as_ref(),
                        m.locking_pubkey.clone().into_static(),
                        m.channel_id,
                        m.sequence_number,
                    )?;

                    let success = SubmitSharesSuccess {
                        channel_id: m.channel_id,
                        last_sequence_number: m.sequence_number,
                        new_submits_accepted_count: 1,
                        new_shares_sum: 0,
                        // TODO is this ownership hack fixable?
                        hash: m.hash.inner_as_ref().to_owned().try_into()?,
                    };

                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))

                },
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::ShareMeetDownstreamTarget => {
                    // Send share submitted stats - never blocks
                    if let Ok(Some(stats_handle)) = self.pool.safe_lock(|p| p.stats_handle.clone()) {
                        stats_handle.send_stats(StatsMessage::ShareSubmitted {
                            downstream_id: self.id,
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64,
                        });
                    }

                    // Submit quote via dispatcher
                    self.quote_dispatcher.submit_quote(
                        m.hash.inner_as_ref(),
                        m.locking_pubkey.clone().into_static(),
                        m.channel_id,
                        m.sequence_number,
                    )?;

                    let success = SubmitSharesSuccess {
                        channel_id: m.channel_id,
                        last_sequence_number: m.sequence_number,
                        new_submits_accepted_count: 1,
                        new_shares_sum: 0,
                        // TODO is this ownership hack fixable?
                        hash: m.hash.inner_as_ref().to_owned().try_into()?,
                    };
                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
                },
            },
            Err(err) => {
                warn!(
                    ?err,
                    channel_id = m.channel_id,
                    sequence_number = m.sequence_number,
                    "Rejecting submit_shares_extended due to channel factory error"
                );
                let submit_error =
                    build_submit_share_error(m.channel_id, m.sequence_number, &err);
                Ok(SendTo::Respond(Mining::SubmitSharesError(submit_error)))
            }
        }
    }

    fn handle_set_custom_mining_job(&mut self, m: SetCustomMiningJob) -> Result<SendTo<()>, Error> {
        let m = SetCustomMiningJobSuccess {
            channel_id: m.channel_id,
            request_id: m.request_id,
            job_id: self
                .channel_factory
                .safe_lock(|cf| cf.on_new_set_custom_mining_job(m.into_static()).job_id)
                .unwrap(),
        };
        Ok(SendTo::Respond(Mining::SetCustomMiningJobSuccess(m)))
    }
}
