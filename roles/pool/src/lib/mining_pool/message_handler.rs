use super::super::mining_pool::Downstream;
use bitcoin_hashes::sha256::Hash;
use mining_sv2::cashu::calculate_work;
use mint_pool_messaging::{MintPoolMessageHub, MintQuoteRequest, MintQuoteResponse};
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
use std::{convert::{TryInto, TryFrom}, sync::Arc};
use tracing::{error, info, debug};

/// Adds a pending share to the PendingShareManager (Phase 1)
fn add_pending_share_sync(
    pool: Arc<Mutex<super::Pool>>, 
    m: &SubmitSharesExtended<'_>
) -> Result<(), Error> {
    use super::pending_shares::PendingShare;
    use tokio::time::Instant;

    let share_hash = m.hash.inner_as_ref().to_vec();
    let pending_share = PendingShare {
        channel_id: m.channel_id,
        sequence_number: m.sequence_number, 
        share_hash: share_hash.clone(),
        share_data: m.clone().into_static(),
        created_at: Instant::now(),
    };

    // Spawn a task to add the pending share asynchronously
    let pool_clone = pool.clone();
    tokio::spawn(async move {
        let result = pool_clone.safe_lock(|p| {
            // Create a future and block on it
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(p.pending_share_manager.add_pending_share(pending_share))
            });
            Ok::<(), String>(())
        });
        
        if let Err(e) = result {
            error!("Failed to add pending share: {}", e);
        }
    });

    Ok(())
}


/// Handle mint quote response received from mint
/// This function logs the quote response details
pub fn handle_mint_quote_response(response: MintQuoteResponse<'static>) {
    // Extract quote_id as string for logging
    let quote_id_str = std::str::from_utf8(response.quote_id.inner_as_ref())
        .unwrap_or("invalid_utf8");
    
    info!("✅ Received mint quote response: quote_id={}", quote_id_str);
    
    // TODO: Store quote_id for correlation with shares and eventual SubmitSharesSuccess
    debug!("TODO: Correlate quote_id {} with pending share submission", quote_id_str);
}

/// Send a mint quote request via SV2 TCP connection

/// Send mint quote request via TCP connection to mint (fire and forget for now)
async fn send_sv2_mint_quote_tcp_async(
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
                        quote_id: binary_sv2::Str0255::try_from(String::from("")).expect("Invalid string"),
                        keyset_id: [0u8; 32].into(),
                    };

                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))

                },
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::ShareMeetDownstreamTarget => {
                 let success = SubmitSharesSuccess {
                        channel_id: m.channel_id,
                        last_sequence_number: m.sequence_number,
                        new_submits_accepted_count: 1,
                        new_shares_sum: 0,
                        hash: [0u8; 32].into(),
                        quote_id: binary_sv2::Str0255::try_from(String::from("")).expect("Invalid string"),
                        keyset_id: [0u8; 32].into(),
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

                    // Send mint quote in background (temporarily simplified)
                    if let Some(config) = self.sv2_config.as_ref() {
                        if config.enabled {
                            let m_static = m.clone().into_static();
                            let pool_clone = self.pool.clone();
                            tokio::spawn(async move {
                                let header_hash = bitcoin_hashes::sha256::Hash::from_slice(m_static.hash.inner_as_ref())
                                    .expect("Invalid header hash");
                                let amount = calculate_work(header_hash.to_byte_array());
                                
                                if let Err(e) = send_sv2_mint_quote_tcp_async(pool_clone, m_static, amount).await {
                                    error!("Failed to send mint quote: {}", e);
                                }
                            });
                        }
                    }

                    // Phase 1: Add to pending shares and return None (deferred response)
                    if let Err(e) = add_pending_share_sync(self.pool.clone(), &m) {
                        error!("Failed to add pending share: {}", e);
                    }
                    
                    Ok(SendTo::None(None))

                },
                roles_logic_sv2::channel_logic::channel_factory::OnNewShare::ShareMeetDownstreamTarget => {
                    // Send quote request in background (fire and forget for now)
                    if let Some(config) = self.sv2_config.as_ref() {
                        if config.enabled {
                            let m_static = m.clone().into_static();
                            let pool_clone = self.pool.clone();
                            tokio::spawn(async move {
                                let header_hash = bitcoin_hashes::sha256::Hash::from_slice(m_static.hash.inner_as_ref())
                                    .expect("Invalid header hash");
                                let amount = calculate_work(header_hash.to_byte_array());
                                
                                if let Err(e) = send_sv2_mint_quote_tcp_async(pool_clone, m_static, amount).await {
                                    error!("Background mint quote failed: {}", e);
                                }
                            });
                        }
                    }

                    // Phase 1: Add to pending shares and return None (deferred response) 
                    if let Err(e) = add_pending_share_sync(self.pool.clone(), &m) {
                        error!("Failed to add pending share: {}", e);
                    }
                    
                    Ok(SendTo::None(None))
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
