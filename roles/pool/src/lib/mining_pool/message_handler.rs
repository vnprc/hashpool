use super::super::mining_pool::Downstream;
use super::pending_shares::PendingShare;
use bitcoin_hashes::sha256::Hash;
use mining_sv2::cashu::calculate_work;
use mining_sv2::MintQuoteNotification;
use mint_pool_messaging::{MintQuoteRequest, MintQuoteResponse};
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
use tokio::time::Instant;
use tracing::{error, info, debug};

/// Creates a mint quote request and sends it via TCP and Redis
fn submit_quote(
    m: SubmitSharesExtended<'_>,
    sv2_config: Option<&Sv2MessagingConfig>,
    pool: Arc<Mutex<super::Pool>>,
) -> Result<(), roles_logic_sv2::Error> {
    let header_hash = Hash::from_slice(m.hash.inner_as_ref())
        .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Invalid header hash: {e}")))?;
    
    let amount = calculate_work(header_hash.to_byte_array());
    
    // Send via TCP if SV2 messaging is enabled
    if let Some(config) = sv2_config {
        if config.enabled {
            // Convert to static lifetime for the async task
            let m_static = m.into_static();
            let pool_clone = pool.clone();
            
            tokio::spawn(async move {
                match send_sv2_mint_quote_tcp(pool_clone, m_static, amount).await {
                    Ok(_) => {
                        info!("Successfully sent mint quote via SV2 TCP");
                    }
                    Err(e) => {
                        error!("Failed to send mint quote via SV2 TCP: {}", e);
                    }
                }
            });
        }
    }
    
    Ok(())
}

/// Send extension message to specific downstream
async fn send_extension_message_to_downstream(
    _pool: Arc<Mutex<super::Pool>>,
    channel_id: u32,
    notification: MintQuoteNotification<'static>,
) -> Result<(), Box<dyn std::error::Error>> {
    // For now, we'll log the notification but skip the actual sending
    // This is a simplified implementation for Phase 1
    info!("Would send MintQuoteNotification to channel {}: quote_id={:?}, amount={}", 
          channel_id,
          String::from_utf8_lossy(notification.quote_id.inner_as_ref()),
          notification.amount);
    
    // TODO: Implement proper extension message framing and sending
    // This requires proper SV2 frame construction with extension type
    
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
        if let Err(e) = send_extension_message_to_downstream(
            pool.clone(),
            share.channel_id,
            notification,
        ).await {
            error!("Failed to send mint quote notification: {}", e);
        } else {
            info!("Sent mint quote notification for channel {} seq {}",
                  share.channel_id, share.sequence_number);
        }
    } else {
        debug!("No pending share found for hash: {:?}", header_hash);
    }
}

/// Send a mint quote request via SV2 TCP connection
/// Create a mint quote request from a submitted share
fn create_mint_quote_request(
    m: &SubmitSharesExtended<'static>,
    amount: u64,
) -> Result<MintQuoteRequest<'static>, Box<dyn std::error::Error + Send + Sync>> {
    use binary_sv2::{Str0255, U256, CompressedPubKey, Sv2Option};
    use std::convert::TryInto;
    
    // Create SV2 mint quote request
    let unit_str = "HASH".as_bytes().to_vec();
    let unit: Str0255 = unit_str.try_into()
        .map_err(|e| format!("Failed to create unit string: {:?}", e))?;
    
    let header_hash_bytes = m.hash.inner_as_ref().to_vec();
    let header_hash: U256 = header_hash_bytes.try_into()
        .map_err(|e| format!("Failed to create header hash: {:?}", e))?;
    
    let locking_key: CompressedPubKey = m.locking_pubkey.clone();
    
    // Create optional description (empty for now)
    let description: Sv2Option<Str0255> = Sv2Option::new(None);
    
    let request = MintQuoteRequest {
        amount,
        unit,
        header_hash,
        description,
        locking_key,
    };
    
    Ok(request.into_static())
}

/// Send mint quote request via TCP connection to mint
async fn send_sv2_mint_quote_tcp(
    pool: Arc<Mutex<super::Pool>>,
    m: SubmitSharesExtended<'static>,
    amount: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use binary_sv2::{Str0255, U256, CompressedPubKey, Sv2Option};
    use std::convert::TryInto;
    
    // Get mint connection sender
    let mint_sender = {
        let mint_connection = pool.safe_lock(|p| p.get_mint_connection())
            .map_err(|e| format!("Failed to lock pool: {}", e))?;
        match mint_connection {
            Some(sender) => sender,
            None => {
                return Err("No active mint connection available".into());
            }
        }
    };
    
    // Create SV2 mint quote request
    let unit_str = "HASH".as_bytes().to_vec();
    let unit: Str0255 = unit_str.try_into()
        .map_err(|e| format!("Failed to create unit string: {:?}", e))?;
    
    let header_hash_bytes = m.hash.inner_as_ref().to_vec();
    let header_hash: U256 = header_hash_bytes.try_into()
        .map_err(|e| format!("Failed to create header hash: {:?}", e))?;
    
    let locking_key: CompressedPubKey = m.locking_pubkey.clone();
    
    // Create optional description (empty for now)
    let description: Sv2Option<Str0255> = Sv2Option::new(None);
    
    let request = MintQuoteRequest {
        amount,
        unit,
        header_hash,
        description,
        locking_key,
    };
    
    // Send over TCP connection using the standard SV2 message pattern
    debug!("Sending SV2 mint quote request over TCP: amount={}", amount);
    
    // Create PoolMessages::MintQuote and convert to frame
    let pool_message = roles_logic_sv2::parsers::PoolMessages::Minting(
        roles_logic_sv2::parsers::Minting::MintQuoteRequest(request.into_static())
    );
    let sv2_frame: super::StdFrame = pool_message.try_into()
        .map_err(|e| format!("Failed to convert to SV2 frame: {:?}", e))?;
    let either_frame = sv2_frame.into();
    
    mint_sender.send(either_frame).await
        .map_err(|e| format!("Failed to send SV2 frame: {:?}", e))?;
    
    info!("📤 Successfully sent SV2 mint quote request via TCP");
    Ok(())
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
                    let amount = calculate_work(hash_bytes);
                    
                    // Track this share as pending for mint quote
                    let pending_share = PendingShare {
                        channel_id: m.channel_id,
                        sequence_number: m.sequence_number,
                        share_hash: m.hash.inner_as_ref().to_vec(),
                        locking_pubkey: m.locking_pubkey.inner_as_ref().to_vec(),
                        amount,
                        created_at: Instant::now(),
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
                        self.pool.clone()
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
                    // Calculate work amount
                    let hash_bytes: [u8; 32] = m.hash.inner_as_ref().try_into()
                        .map_err(|_| Error::ExpectedLen32(m.hash.inner_as_ref().len()))?;
                    let amount = calculate_work(hash_bytes);
                    
                    // Track this share as pending for mint quote
                    let pending_share = PendingShare {
                        channel_id: m.channel_id,
                        sequence_number: m.sequence_number,
                        share_hash: m.hash.inner_as_ref().to_vec(),
                        locking_pubkey: m.locking_pubkey.inner_as_ref().to_vec(),
                        amount,
                        created_at: Instant::now(),
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
                        self.pool.clone()
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
