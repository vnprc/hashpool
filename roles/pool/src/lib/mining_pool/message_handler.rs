use super::super::mining_pool::Downstream;
use bitcoin_hashes::sha256::Hash;
use cdk::secp256k1::hashes::Hash as CdkHashTrait;
use mining_sv2::cashu::calculate_work;
use mint_pool_messaging::{MintPoolMessageHub, MintQuoteRequest};
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
use shared_config::{RedisConfig, Sv2MessagingConfig};
use std::{convert::TryInto, sync::Arc};
use tracing::{error, info, debug};

/// Creates a mint quote request and sends it via both Redis and SV2 messaging
fn create_and_enqueue_mining_share_quote(
    m: SubmitSharesExtended<'_>,
    redis_config: &RedisConfig,
    sv2_hub: Option<&Arc<MintPoolMessageHub>>,
    sv2_config: Option<&Sv2MessagingConfig>,
    pool: Arc<Mutex<super::Pool>>,
) -> Result<(), roles_logic_sv2::Error> {
    let header_hash = Hash::from_slice(m.hash.inner_as_ref())
        .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Invalid header hash: {e}")))?;
    
    let amount = calculate_work(header_hash.to_byte_array());
    
    // Convert locking_pubkey from SV2 format to CDK format
    let pubkey_bytes = m.locking_pubkey.inner_as_ref().to_vec();
    debug!("Pool received pubkey bytes ({} bytes): {:?}", pubkey_bytes.len(), pubkey_bytes);
    debug!("Pool received pubkey hex: {}", cdk::util::hex::encode(&pubkey_bytes));
    let pubkey = cdk::nuts::PublicKey::from_slice(&pubkey_bytes)
        .map_err(|e| {
            error!("CDK PublicKey::from_slice failed with bytes ({} bytes): {:?}, hex: {}, error: {}", 
                pubkey_bytes.len(), pubkey_bytes, cdk::util::hex::encode(&pubkey_bytes), e);
            roles_logic_sv2::Error::KeysetError(format!("Invalid locking pubkey: {}", e))
        })?;
    
    // Convert header_hash to CDK Hash type
    let cdk_header_hash = CdkHashTrait::from_slice(&header_hash.to_byte_array())
        .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Failed to convert header hash: {}", e)))?;
    
    // Convert keyset_id from SV2 format to CDK format using the proper adapter
    let keyset_id_bytes = m.keyset_id.inner_as_ref();
    
    debug!("Converting keyset ID from {} bytes: {}", keyset_id_bytes.len(), cdk::util::hex::encode(keyset_id_bytes));
    
    let keyset_id = mining_sv2::cashu::keyset_from_sv2_bytes(keyset_id_bytes)
        .map_err(|e| roles_logic_sv2::Error::KeysetError(format!("Failed to convert keyset ID: {}", e)))?;
    
    // Create CDK quote request for Redis (existing functionality)
    let quote_request = cdk::nuts::nutXX::MintQuoteMiningShareRequest {
        amount: amount.into(),
        unit: cdk::nuts::CurrencyUnit::Hash,
        header_hash: cdk_header_hash,
        description: None,
        pubkey: pubkey,
        keyset_id,
    };
    
    // Send via Redis (existing functionality)
    let json = mining_sv2::cashu::format_quote_event_json(&quote_request);
    tokio::spawn(enqueue_quote_event(redis_config.clone(), json));
    
    // Send via SV2 messaging (new functionality) if enabled
    if let (Some(_hub), Some(config)) = (sv2_hub, sv2_config) {
        if config.enabled {
            // Convert to static lifetime for the async task
            let m_static = m.into_static();
            tokio::spawn(async move {
                if let Err(e) = send_sv2_mint_quote_tcp(pool, m_static, amount).await {
                    error!("Failed to send SV2 mint quote via TCP: {}", e);
                } else {
                    info!("Successfully sent mint quote via SV2 TCP");
                }
            });
        }
    }
    
    Ok(())
}

/// Send a mint quote request via SV2 TCP connection
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
    
    // Pad keyset_id to 32 bytes for U256 (keyset IDs are 8 bytes, U256 needs 32 bytes)
    let keyset_id_bytes = m.keyset_id.inner_as_ref();
    let mut padded_keyset_id = [0u8; 32];
    padded_keyset_id[24..].copy_from_slice(keyset_id_bytes); // Right-pad with the 8-byte keyset ID
    let keyset_id: U256 = padded_keyset_id.to_vec().try_into()
        .map_err(|e| format!("Failed to create keyset ID: {:?}", e))?;
    
    // Create optional description (empty for now)
    let description: Sv2Option<Str0255> = Sv2Option::new(None);
    
    let request = MintQuoteRequest {
        amount,
        unit,
        header_hash,
        description,
        locking_key,
        keyset_id,
    };
    
    // Send over TCP connection using the standard SV2 message pattern
    debug!("Sending SV2 mint quote request over TCP: amount={}", amount);
    
    // Create PoolMessages::MintQuote and convert to frame
    let pool_message = roles_logic_sv2::parsers::PoolMessages::MintQuote(
        roles_logic_sv2::parsers::MintQuote::MintQuoteRequest(request.into_static())
    );
    let sv2_frame: super::StdFrame = pool_message.try_into()
        .map_err(|e| format!("Failed to convert to SV2 frame: {:?}", e))?;
    let either_frame = sv2_frame.into();
    
    mint_sender.send(either_frame).await
        .map_err(|e| format!("Failed to send SV2 frame: {:?}", e))?;
    
    info!("Successfully sent SV2 mint quote request via TCP");
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

                    create_and_enqueue_mining_share_quote(
                        m.clone(), 
                        &self.redis_config,
                        self.sv2_hub.as_ref(),
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
                    create_and_enqueue_mining_share_quote(
                        m.clone(), 
                        &self.redis_config,
                        self.sv2_hub.as_ref(),
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
