use super::super::mining_pool::Downstream;
use bitcoin_hashes::sha256::Hash;
use mining_sv2::cashu::{calculate_work, BlindedMessageSet};
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
use shared_config::RedisConfig;
use std::{convert::{TryFrom, TryInto}, sync::Arc};
use tracing::error;

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
        let reposnses = self
            .channel_factory
            .safe_lock(|factory| {
                match factory.add_standard_channel(
                    incoming.request_id.as_u32(),
                    incoming.nominal_hash_rate,
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
        let mut result = vec![];
        for response in reposnses {
            result.push(SendTo::Respond(response.into_static()))
        }
        Ok(SendTo::Multiple(result))
    }

    fn handle_open_extended_mining_channel(
        &mut self,
        m: OpenExtendedMiningChannel,
    ) -> Result<SendTo<()>, Error> {
        let request_id = m.request_id;
        let hash_rate = m.nominal_hash_rate;
        let min_extranonce_size = m.min_extranonce_size;
        let messages_res = self
            .channel_factory
            .safe_lock(|s| s.new_extended_channel(request_id, hash_rate, min_extranonce_size))
            .map_err(|e| roles_logic_sv2::Error::PoisonLock(e.to_string()))?;
        match messages_res {
            Ok(messages) => {
                let messages = messages.into_iter().map(SendTo::Respond).collect();
                Ok(SendTo::Multiple(messages))
            }
            Err(_) => Err(roles_logic_sv2::Error::ChannelIsNeitherExtendedNeitherInAPool),
        }
    }

    fn handle_update_channel(&mut self, m: UpdateChannel) -> Result<SendTo<()>, Error> {
        let maximum_target =
            roles_logic_sv2::utils::hash_rate_to_target(m.nominal_hash_rate.into(), 10.0)?;
        self.channel_factory
            .safe_lock(|s| s.update_target_for_channel(m.channel_id, maximum_target.clone().into()))
            .unwrap_or_else(|_| {
                std::process::exit(1);
            });
        let set_target = SetTarget {
            channel_id: m.channel_id,
            maximum_target,
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
            Err(_) => todo!(),
        }
    }

    fn handle_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtended,
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

                    let header_hash = Hash::from_slice(m.hash.inner_as_ref())
                        .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Invalid header hash: {e}")))?;

                    let quote_request = cdk::nuts::nutXX::MintQuoteMiningShareRequest {
                        amount: calculate_work(header_hash.to_byte_array()).into(),
                        unit: cdk::nuts::CurrencyUnit::Custom("HASH".to_string()),
                        header_hash,
                        description: None,
                        pubkey: None,
                    };

                    let blinded_message_set = BlindedMessageSet::try_from(m.blinded_messages.clone())
                    .expect("Failed to convert Sv2BlindedMessageSetWire to BlindedMessageSet");

                    let blinded_message_vec: Vec<cdk::nuts::BlindedMessage> = blinded_message_set.items.iter()
                        .filter_map(|item| item.clone())
                        .collect();

                    let json = mining_sv2::cashu::format_quote_event_json(&quote_request, &blinded_message_vec);
                    // TODO ensure future resolves
                    tokio::spawn(enqueue_quote_event(self.redis_config.clone(), json));
    

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
                    let header_hash = Hash::from_slice(m.hash.inner_as_ref())
                        .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Invalid header hash: {e}")))?;
                    let amount = calculate_work(header_hash.to_byte_array());

                    let quote_request = cdk::nuts::nutXX::MintQuoteMiningShareRequest {
                        amount: amount.into(),
                        unit: cdk::nuts::CurrencyUnit::Custom("HASH".to_string()),
                        header_hash: Hash::from_slice(m.hash.inner_as_ref())
                            .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Invalid header hash: {e}")))?,
                        description: None,
                        pubkey: None,
                    };

                    let blinded_message_set = BlindedMessageSet::try_from(m.blinded_messages.clone())
                    .expect("Failed to convert Sv2BlindedMessageSetWire to BlindedMessageSet");

                    let blinded_message_vec: Vec<cdk::nuts::BlindedMessage> = blinded_message_set.items.iter()
                        .filter_map(|item| item.clone())
                        .collect();

                    let json = mining_sv2::cashu::format_quote_event_json(&quote_request, &blinded_message_vec);
                    // TODO ensure future resolves
                    tokio::spawn(enqueue_quote_event(self.redis_config.clone(), json));

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
            Err(e) => {
                error!("{:?}",e);
                todo!();
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

async fn enqueue_quote_event(redis_config: RedisConfig, payload: String) {
    let client = redis::Client::open(redis_config.url).expect("Invalid Redis URL");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("Failed to connect to Redis");

    let _: () = redis::cmd("RPUSH")
        .arg(redis_config.create_quote_prefix)
        .arg(payload)
        .query_async(&mut conn)
        .await
        .expect("Failed to push to Redis");
}
