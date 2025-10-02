use super::super::mining_pool::Downstream;
use super::pending_shares::PendingShare;
use super::super::stats::StatsMessage;
use bitcoin_hashes::sha256::Hash;
use ehash::calculate_ehash_amount;
use mining_sv2::MintQuoteNotification;
use mint_pool_messaging::{build_parsed_quote_request, MintQuoteResponse};
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
use shared_config::Sv2MessagingConfig;
use std::{convert::TryInto, sync::Arc};
use tokio::time::Instant as TokioInstant;
use tracing::{debug, error, info, warn};

/// Creates a mint quote request and sends it via TCP and Redis
fn submit_quote(
    m: SubmitSharesExtended<'_>,
    sv2_config: Option<&Sv2MessagingConfig>,
    pool: Arc<Mutex<super::Pool>>,
    minimum_difficulty: u32,
) -> Result<(), roles_logic_sv2::Error> {
    let header_hash = Hash::from_slice(m.hash.inner_as_ref())
        .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Invalid header hash: {e}")))?;

    let amount = calculate_ehash_amount(header_hash.to_byte_array(), minimum_difficulty);

    // Send stats update via channel - never blocks
    if let Ok((stats_handle, downstream_id)) = pool.safe_lock(|p| {
        let downstream_id = p.channel_to_downstream.get(&m.channel_id).copied();
        (p.stats_handle.clone(), downstream_id)
    }) {
        if let Some(id) = downstream_id {
            stats_handle.send_stats(StatsMessage::QuoteCreated { 
                downstream_id: id, 
                amount 
            });
        }
    }

    // Send via TCP if SV2 messaging is enabled
    if let Some(config) = sv2_config {
        if config.enabled {
            // Convert to static lifetime for the async task
            let m_static = m.into_static();
            let share_hash = m_static.hash.inner_as_ref().to_vec();
            let locking_key = m_static.locking_pubkey.clone();

            match pool.safe_lock(|p| p.mint_message_hub.clone()) {
                Ok(Some(hub)) => {
                    tokio::spawn(async move {
                        match build_parsed_quote_request(amount, &share_hash, locking_key) {
                            Ok(parsed) => {
                                if let Err(e) = hub.send_quote_request(parsed).await {
                                    error!("Failed to dispatch mint quote request via hub: {}", e);
                                } else {
                                    info!("Queued mint quote request via hub: share_hash={}", hex::encode(share_hash));
                                }
                            }
                            Err(e) => {
                                error!("Failed to build mint quote request: {}", e);
                            }
                        }
                    });
                }
                Ok(None) => {
                    warn!("SV2 messaging enabled but mint message hub unavailable; skipping quote dispatch");
                }
                Err(e) => {
                    error!("Failed to access pool for mint hub: {}", e);
                }
            }
        }
    }

    Ok(())
}


/// Handle mint quote response received from mint
/// This function sends an extension message to the downstream with the quote
pub async fn handle_mint_quote_response(
    pool: Arc<Mutex<super::Pool>>,
    response: MintQuoteResponse<'static>,
) {
    // Extract quote_id as string for logging
    let quote_id_str = std::str::from_utf8(response.quote_id.inner_as_ref())
        .unwrap_or("invalid_utf8");
    
    info!("✅ Received mint quote response: quote_id={}", quote_id_str);
    
    // Get the pending share manager and find the share by hash
    let header_hash = response.header_hash.inner_as_ref().to_vec();
    
    // Get the manager outside of the lock to avoid Send issues
    let manager = match pool.safe_lock(|p| p.pending_share_manager.clone()) {
        Ok(manager) => manager,
        Err(e) => {
            error!("Failed to access pending share manager: {}", e);
            return;
        }
    };
    
    let pending_share = manager.remove_pending_share(&header_hash).await;
    
    if let Some(share) = pending_share {
        // Create the extension message
        let notification = MintQuoteNotification {
            channel_id: share.channel_id,
            sequence_number: share.sequence_number,
            share_hash: response.header_hash.clone(),
            quote_id: response.quote_id.clone(),
            amount: share.amount,
        };
        
        // Send extension message to the downstream
        if let Err(e) = super::Pool::send_extension_message_to_downstream(
            pool.clone(),
            share.channel_id,
            notification,
        ).await {
            error!("Failed to send mint quote notification: {}", e);
        } else {
            info!("Sent mint quote notification for channel {} seq {}",
                  share.channel_id, share.sequence_number);
            // NOTE: quotes_redeemed should only be incremented when the translator's proof sweeper
            // actually mints tokens (changes quote state to ISSUED), not when quote is created
        }
    } else {
        debug!("No pending share found for hash: {:?}", header_hash);
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
                    debug!("Added channel mapping: channel_id {} -> downstream_id {}", success.channel_id, self.id);
                }) {
                    // Send stats update for new channel
                    if let Ok(stats_handle) = self.pool.safe_lock(|p| p.stats_handle.clone()) {
                        stats_handle.send_stats(StatsMessage::ChannelAdded { 
                            downstream_id: self.id, 
                            channel_id: success.channel_id 
                        });
                    }
                } else {
                    error!("Failed to add channel mapping for channel_id: {}", success.channel_id);
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
                            debug!("Added extended channel mapping: channel_id {} -> downstream_id {}", success.channel_id, self.id);
                        }) {
                            // Send stats update for new extended channel
                            if let Ok(stats_handle) = self.pool.safe_lock(|p| p.stats_handle.clone()) {
                                stats_handle.send_stats(StatsMessage::ChannelAdded { 
                                    downstream_id: self.id, 
                                    channel_id: success.channel_id 
                                });
                            }
                        } else {
                            error!("Failed to add extended channel mapping for channel_id: {}", success.channel_id);
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
        let fixed_low_target = roles_logic_sv2::utils::hash_rate_to_target(10_000_000_000_000.0, 10.0)?;
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
            Err(_) => todo!(),
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
                    if let Ok(stats_handle) = self.pool.safe_lock(|p| p.stats_handle.clone()) {
                        stats_handle.send_stats(StatsMessage::ShareSubmitted { 
                            downstream_id: self.id 
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

                    // Calculate work amount
                    let hash_bytes: [u8; 32] = m.hash.inner_as_ref().try_into()
                        .map_err(|_| Error::ExpectedLen32(m.hash.inner_as_ref().len()))?;
                    let minimum_difficulty = self.pool.safe_lock(|p| p.minimum_difficulty)
                        .map_err(|_| Error::PoisonLock(format!("Failed to lock pool")))?;
                    let amount = calculate_ehash_amount(hash_bytes, minimum_difficulty);
                    
                    // Track this share as pending for mint quote
                    let pending_share = PendingShare {
                        channel_id: m.channel_id,
                        sequence_number: m.sequence_number,
                        share_hash: m.hash.inner_as_ref().to_vec(),
                        locking_pubkey: m.locking_pubkey.inner_as_ref().to_vec(),
                        amount,
                        created_at: TokioInstant::now(),
                    };
                    
                    // Add to pending shares
                    if let Ok(manager) = self.pool.safe_lock(|p| p.pending_share_manager.clone()) {
                        let manager_clone = manager.clone();
                        let share_clone = pending_share.clone();
                        tokio::spawn(async move {
                            manager_clone.add_pending_share(share_clone).await;
                        });
                    }

                    submit_quote(
                        m.clone(), 
                        self.sv2_config.as_ref(),
                        self.pool.clone(),
                        minimum_difficulty
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
                    if let Ok(stats_handle) = self.pool.safe_lock(|p| p.stats_handle.clone()) {
                        stats_handle.send_stats(StatsMessage::ShareSubmitted { 
                            downstream_id: self.id 
                        });
                    }
                    
                    // Calculate ehash units
                    let hash_bytes: [u8; 32] = m.hash.inner_as_ref().try_into()
                        .map_err(|_| Error::ExpectedLen32(m.hash.inner_as_ref().len()))?;
                    let minimum_difficulty = self.pool.safe_lock(|p| p.minimum_difficulty)
                        .map_err(|_| Error::PoisonLock(format!("Failed to lock pool")))?;
                    let amount = calculate_ehash_amount(hash_bytes, minimum_difficulty);
                    
                    // Track this share as pending for mint quote
                    let pending_share = PendingShare {
                        channel_id: m.channel_id,
                        sequence_number: m.sequence_number,
                        share_hash: m.hash.inner_as_ref().to_vec(),
                        locking_pubkey: m.locking_pubkey.inner_as_ref().to_vec(),
                        amount,
                        created_at: TokioInstant::now(),
                    };
                    
                    // Add to pending shares
                    if let Ok(manager) = self.pool.safe_lock(|p| p.pending_share_manager.clone()) {
                        let manager_clone = manager.clone();
                        let share_clone = pending_share.clone();
                        tokio::spawn(async move {
                            manager_clone.add_pending_share(share_clone).await;
                        });
                    }
                    
                    submit_quote(
                        m.clone(), 
                        self.sv2_config.as_ref(),
                        self.pool.clone(),
                        minimum_difficulty
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
