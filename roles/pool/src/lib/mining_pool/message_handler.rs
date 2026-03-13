//! # Downstream Message Handler Implementation
//!
//! Implements the `ParseMiningMessagesFromDownstream` trait for the [`Downstream`] struct.
//!
//! This module defines how the pool logic (specifically, a `Downstream` connection instance)
//! reacts to various mining-related messages received from a connected downstream miner.

use super::super::mining_pool::Downstream;
use super::super::share_validation;
use binary_sv2::{Deserialize, Str0255};
use ehash::QuoteDispatchError;
use mint_quote_sv2::CompressedPubKey;
use quote_dispatcher::QuoteDispatcher;
use std::{
    convert::TryInto,
    sync::{Arc, RwLock},
};
use stratum_common::roles_logic_sv2::{
    bitcoin::{
        hashes::Hash as BitcoinHash,
        consensus::Decodable,
        transaction::TxOut,
        Amount,
        Target as BitcoinTarget,
    },
    channels_sv2::server::{
        error::{ExtendedChannelError, StandardChannelError},
        extended::ExtendedChannel,
        group::GroupChannel,
        jobs::job_store::DefaultJobStore,
        share_accounting::{ShareValidationError, ShareValidationResult},
        standard::StandardChannel,
    },
    errors::Error,
    handlers::mining::{ParseMiningMessagesFromDownstream, SendTo, SupportedChannelTypes},
    mining_sv2::*,
    parsers_sv2::Mining,
    template_distribution_sv2::SubmitSolution,
    utils::{Mutex, target_to_difficulty},
    Vardiff, VardiffState,
};
use tracing::{debug, error, info, warn};

/// Attempts to dispatch a quote for an accepted share to the mint service.
///
/// This is an async operation that:
/// 1. Retrieves or uses the provided locking key for attribution
/// 2. Validates the key format (must be 33 bytes)
/// 3. Parses the key as a compressed public key
/// 4. Submits the quote to the dispatcher
///
/// Returns `Ok(())` on success or a `QuoteDispatchError` if any step fails.
async fn dispatch_quote(
    dispatcher: Arc<QuoteDispatcher>,
    mint_manager: Arc<super::mint_integration::MintIntegrationManager>,
    channel_id: u32,
    sequence_number: u32,
    header_hash: [u8; 32],
    locking_key_hint: Option<Vec<u8>>,
) -> Result<(), QuoteDispatchError> {
    let header_hash_bytes = header_hash;

    // Get locking key from hint or mint manager
    let locking_key_bytes = if let Some(bytes) = locking_key_hint {
        Some(bytes)
    } else {
        mint_manager
            .get_channel_context(channel_id)
            .await
            .and_then(|ctx| ctx.locking_key_bytes.clone())
    };

    // Validate that we have a locking key
    let bytes = locking_key_bytes.ok_or(QuoteDispatchError::MissingLockingKey(channel_id))?;

    // Validate key length
    if bytes.len() != 33 {
        return Err(QuoteDispatchError::InvalidLockingKeyFormat {
            channel_id,
            length: bytes.len(),
        });
    }

    // Parse locking key as compressed public key
    let mut key_bytes = [0u8; 33];
    key_bytes.copy_from_slice(&bytes);
    let mut encoded = [0u8; 34];
    encoded[0] = key_bytes.len() as u8;
    encoded[1..].copy_from_slice(&key_bytes);

    let pubkey = CompressedPubKey::from_bytes(&mut encoded[..]).map_err(|e| {
        QuoteDispatchError::InvalidLockingKey {
            channel_id,
            reason: format!("{:?}", e),
        }
    })?;

    // Submit quote to dispatcher
    dispatcher
        .submit_quote(&header_hash_bytes, pubkey.into_static(), channel_id, sequence_number)
        .map_err(|e| QuoteDispatchError::QuoteDispatchFailed(e.to_string()))?;

    Ok(())
}

/// Helper function to send a quote request for an accepted share.
///
/// Spawns an async task to dispatch a quote for an accepted share. The task:
/// 1. Dispatches the quote to the mint service if dispatcher is configured
/// 2. Logs errors that occur (e.g., missing/invalid locking key, dispatch failure)
/// 3. Does NOT fail the share validation if dispatch fails (non-fatal)
///
/// This ensures quote failures don't break the mining protocol while still
/// providing visibility into any issues via logging.

/// Helper function to validate minimum share difficulty and return an error response if needed.
///
/// Returns `Some(error_response)` if validation fails and should be sent to miner,
/// or `None` if validation passes.
fn validate_minimum_share_difficulty(
    header_hash: &[u8; 32],
    minimum_bits: Option<u32>,
    channel_id: u32,
    sequence_number: u32,
) -> Option<SendTo<()>> {
    if let Some(min_bits) = minimum_bits {
        // Convert to big-endian for difficulty calculation (hash is little-endian bytes).
        let mut hash_bytes = *header_hash;
        hash_bytes.reverse();
        if let Err(e) = share_validation::validate_share_difficulty(&hash_bytes, Some(min_bits)) {
            error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: share-difficulty-too-low ❌ ({})", channel_id, sequence_number, e);
            let error = SubmitSharesError {
                channel_id,
                sequence_number,
                error_code: "share-difficulty-too-low"
                    .to_string()
                    .try_into()
                    .expect("error code must be valid string"),
            };
            return Some(SendTo::Respond(Mining::SubmitSharesError(error)));
        }
    }
    None
}

fn send_share_quote_request(
    downstream: &Downstream,
    channel_id: u32,
    sequence_number: u32,
    header_hash: [u8; 32],
    _m: &SubmitSharesStandard,
) {
    if let Some(dispatcher) = downstream.quote_dispatcher.clone() {
        let mint_manager = downstream.mint_manager.clone();
        let locking_key_bytes = downstream.locking_key_bytes.clone();

        tokio::spawn(async move {
            match dispatch_quote(
                dispatcher,
                mint_manager,
                channel_id,
                sequence_number,
                header_hash,
                locking_key_bytes,
            )
            .await
            {
                Ok(()) => {
                    debug!(
                        "Successfully queued mint quote via dispatcher: channel={}, seq={}",
                        channel_id, sequence_number
                    );
                }
                Err(e) => {
                    warn!(
                        "Quote dispatch error for channel {} (seq {}): {}",
                        channel_id, sequence_number, e
                    );
                }
            }
        });
    } else {
        debug!(
            "Quote dispatcher not configured; skipping quote for channel {}",
            channel_id
        );
    }
}

/// Helper function to send a quote request for an extended share.
///
/// Spawns an async task to dispatch a quote for an extended share. Similar to
/// `send_share_quote_request` but for extended mining channels.
///
/// Quote dispatch errors are logged but don't affect share validation.
fn send_extended_share_quote_request(
    downstream: &Downstream,
    channel_id: u32,
    sequence_number: u32,
    header_hash: [u8; 32],
    _m: &SubmitSharesExtended,
) {
    if let Some(dispatcher) = downstream.quote_dispatcher.clone() {
        let mint_manager = downstream.mint_manager.clone();
        let locking_key_bytes = downstream.locking_key_bytes.clone();

        tokio::spawn(async move {
            match dispatch_quote(
                dispatcher,
                mint_manager,
                channel_id,
                sequence_number,
                header_hash,
                locking_key_bytes,
            )
            .await
            {
                Ok(()) => {
                    debug!(
                        "Successfully queued mint quote via dispatcher (extended): channel={}, seq={}",
                        channel_id, sequence_number
                    );
                }
                Err(e) => {
                    warn!(
                        "Extended quote dispatch error for channel {} (seq {}): {}",
                        channel_id, sequence_number, e
                    );
                }
            }
        });
    } else {
        debug!(
            "Quote dispatcher not configured; skipping extended quote for channel {}",
            channel_id
        );
    }
}

/// Helper function to spawn a channel registration task with proper logging.
///
/// Spawns an async task to register a channel with the mint manager. The task:
/// 1. Registers the channel with the mint manager
/// 2. Logs success at INFO level with downstream and channel IDs
/// 3. Does NOT fail channel creation if registration takes time (non-fatal)
///
/// This ensures that channel registration happens asynchronously and doesn't block
/// channel creation while still providing visibility into registration via logging.
fn spawn_channel_registration(
    mint_manager: Arc<super::mint_integration::MintIntegrationManager>,
    downstream_id: u32,
    channel_id: u32,
    locking_key: Vec<u8>,
    channel_type: &'static str,
) {
    tokio::spawn(async move {
        mint_manager
            .register_channel(channel_id, Some(locking_key), downstream_id)
            .await;
        info!(
            "Registered {} channel {} with mint manager (downstream={})",
            channel_type, channel_id, downstream_id
        );
    });
}

impl ParseMiningMessagesFromDownstream<()> for Downstream {
    // Specifies the types of mining channels supported by this pool implementation.
    //
    // Currently always returns `SupportedChannelTypes::GroupAndExtended`, indicating
    // support for both standard (grouped under a single ID) and extended channels.
    fn get_channel_type(&self) -> SupportedChannelTypes {
        SupportedChannelTypes::GroupAndExtended
    }

    // Indicates whether downstream miners are allowed to select their own work (jobs).
    fn is_work_selection_enabled(&self) -> bool {
        true
    }

    // Checks if a downstream miner is authorized based on its user identity.
    fn is_downstream_authorized(
        _self_mutex: Arc<Mutex<Self>>,
        _user_identity: &Str0255,
    ) -> Result<bool, Error> {
        Ok(true)
    }

    // Handles an `OpenStandardMiningChannel` message from the downstream miner.
    //
    // This attempts to add a new standard mining channel (or associate the request
    // with an existing group ID) using the `PoolChannelFactory`.
    //
    // Returns
    // - `Ok(SendTo::Multiple)` - Containing success messages (`OpenStandardMiningChannelSuccess`,
    //   `SetTarget`, `SetExtranoncePrefix`) generated by the factory upon successful channel setup.
    // - `Err(Error)` - If the channel factory fails to add the channel (e.g., invalid parameters,
    //   internal error, lock poisoning).
    fn handle_open_standard_mining_channel(
        &mut self,
        incoming: OpenStandardMiningChannel,
    ) -> Result<SendTo<()>, Error> {
        let request_id = incoming.get_request_id_as_u32();
        let user_identity = std::str::from_utf8(incoming.user_identity.as_ref())
            .map(|s| s.to_string())
            .map_err(|e| Error::InvalidUserIdentity(e.to_string()))?;

        info!("Received OpenStandardMiningChannel: {}", incoming);

        if self.requires_custom_work {
            error!("OpenStandardMiningChannel: Standard Channels are not supported for this connection");
            let open_standard_mining_channel_error = OpenMiningChannelError {
                request_id,
                error_code: "standard-channels-not-supported-for-custom-work"
                    .to_string()
                    .try_into()
                    .expect("error code must be valid string"),
            };
            return Ok(SendTo::Respond(Mining::OpenMiningChannelError(
                open_standard_mining_channel_error,
            )));
        }

        let last_future_template = self.last_future_template.clone();
        let last_set_new_prev_hash_tdp = self.last_new_prev_hash.clone();

        let pool_coinbase_output = TxOut {
            value: Amount::from_sat(last_future_template.coinbase_tx_value_remaining),
            script_pubkey: self.coinbase_reward_script.script_pubkey(),
        };

        if !self.requires_standard_jobs && self.group_channel.is_none() {
            // we only create one group channel for all standard channels

            let group_channel_id = self.channel_id_factory.next();
            let job_store = DefaultJobStore::new();

            let mut group_channel = GroupChannel::new_for_pool(
                group_channel_id,
                job_store,
                MAX_EXTRANONCE_LEN,
                self.pool_tag_string.clone(),
            )
            .map_err(Error::FailedToProcessNewTemplateGroupChannel)?;
            group_channel
                .on_new_template(
                    last_future_template.clone(),
                    vec![pool_coinbase_output.clone()],
                )
                .map_err(Error::FailedToProcessNewTemplateGroupChannel)?;

            group_channel
                .on_set_new_prev_hash(last_set_new_prev_hash_tdp.clone())
                .map_err(Error::FailedToProcessSetNewPrevHashGroupChannel)?;

            self.group_channel = Some(Arc::new(RwLock::new(group_channel)));
        }

        let mut nominal_hash_rate = incoming.nominal_hash_rate;

        // Apply optional minimum downstream hashrate policy
        if let Some(min_hashrate) = self.min_downstream_hashrate {
            if nominal_hash_rate < min_hashrate {
                info!("Clamping nominal_hash_rate from {} to {} (min_downstream_hashrate policy)",
                      nominal_hash_rate, min_hashrate);
                nominal_hash_rate = min_hashrate;
            }
        }

        let requested_max_target = incoming.max_target.into_static();
        let requested_max_target_bitcoin = BitcoinTarget::from_le_bytes(
            requested_max_target.inner_as_ref().try_into().unwrap(),
        );

        let extranonce_prefix = self
            .extranonce_prefix_factory_standard
            .safe_lock(|factory| factory.next_prefix_standard())
            .map_err(|e| Error::PoisonLock(e.to_string()))
            .and_then(|res| res.map_err(Error::ExtranoncePrefixFactoryError))?
            .to_vec();

        let channel_id = self.channel_id_factory.next();
        let job_store = DefaultJobStore::new();
        let mut standard_channel = match StandardChannel::new_for_pool(
            channel_id,
            user_identity,
            extranonce_prefix.clone(),
            requested_max_target_bitcoin,
            nominal_hash_rate,
            self.share_batch_size,
            self.shares_per_minute,
            job_store,
            self.pool_tag_string.clone(),
        ) {
            Ok(channel) => channel,
            Err(e) => match e {
                StandardChannelError::InvalidNominalHashrate => {
                    error!("OpenMiningChannelError: invalid-nominal-hashrate");
                    let open_standard_mining_channel_error = OpenMiningChannelError {
                        request_id,
                        error_code: "invalid-nominal-hashrate"
                            .to_string()
                            .try_into()
                            .expect("error code must be valid string"),
                    };
                    return Ok(SendTo::Respond(Mining::OpenMiningChannelError(
                        open_standard_mining_channel_error,
                    )));
                }
                StandardChannelError::RequestedMaxTargetOutOfRange => {
                    error!("OpenMiningChannelError: max-target-out-of-range");
                    let open_standard_mining_channel_error = OpenMiningChannelError {
                        request_id,
                        error_code: "max-target-out-of-range"
                            .to_string()
                            .try_into()
                            .expect("error code must be valid string"),
                    };
                    return Ok(SendTo::Respond(Mining::OpenMiningChannelError(
                        open_standard_mining_channel_error,
                    )));
                }
                _ => {
                    error!("error in handle_open_standard_mining_channel: {:?}", e);
                    return Err(Error::FailedToCreateStandardChannel(e));
                }
            },
        };

        let mut messages = vec![];

        let group_channel_id = if let Some(group_channel_guard) = &self.group_channel {
            let group_channel = group_channel_guard
                .read()
                .map_err(|e| Error::PoisonLock(e.to_string()))?;
            group_channel.get_group_channel_id()
        } else {
            0
        };

        let open_standard_mining_channel_success = OpenStandardMiningChannelSuccess {
            request_id: incoming.request_id,
            channel_id,
            target: standard_channel.get_target().to_le_bytes().into(),
            extranonce_prefix: standard_channel
                .get_extranonce_prefix()
                .clone()
                .try_into()
                .expect("extranonce_prefix must be valid"),
            group_channel_id,
        }
        .into_static();

        messages.push(Mining::OpenStandardMiningChannelSuccess(
            open_standard_mining_channel_success,
        ));

        if let Some(locking_key) = self.locking_key_bytes.clone() {
            let mint_manager = self.mint_manager.clone();
            let downstream_id = self.id;
            spawn_channel_registration(mint_manager, downstream_id, channel_id, locking_key, "standard");
        } else {
            debug!(
                "Skipping mint registration for standard channel {} (missing locking key)",
                channel_id
            );
        }

        // create a future standard job based on the last future template
        standard_channel
            .on_new_template(
                last_future_template.clone(),
                vec![pool_coinbase_output.clone()],
            )
            .map_err(Error::FailedToCreateStandardChannel)?;

        let future_standard_job_id = standard_channel
            .get_future_job_id_from_template_id(last_future_template.template_id)
            .expect("future job id must exist");
        let future_standard_job = standard_channel
            .get_future_job(future_standard_job_id)
            .expect("future job must exist");
        let future_standard_job_message =
            future_standard_job.get_job_message().clone().into_static();

        messages.push(Mining::NewMiningJob(future_standard_job_message));

        // SetNewPrevHash message activates the future job
        let prev_hash = last_set_new_prev_hash_tdp.prev_hash.clone();
        let header_timestamp = last_set_new_prev_hash_tdp.header_timestamp;
        let n_bits = last_set_new_prev_hash_tdp.n_bits;
        let set_new_prev_hash_mining = SetNewPrevHash {
            channel_id,
            job_id: future_standard_job_id,
            prev_hash,
            min_ntime: header_timestamp,
            nbits: n_bits,
        };
        standard_channel
            .on_set_new_prev_hash(last_set_new_prev_hash_tdp.clone())
            .map_err(Error::FailedToCreateStandardChannel)?;
        messages.push(Mining::SetNewPrevHash(set_new_prev_hash_mining));

        let messages = messages.into_iter().map(SendTo::Respond).collect();

        let vardiff = VardiffState::new()?;

        self.standard_channels
            .insert(channel_id, Arc::new(RwLock::new(standard_channel)));

        self.vardiff
            .insert(channel_id, Arc::new(RwLock::new(vardiff)));

        if let Some(group_channel_guard) = &self.group_channel {
            let mut group_channel = group_channel_guard
                .write()
                .map_err(|e| Error::PoisonLock(e.to_string()))?;
            group_channel
                .add_channel_id(channel_id, MAX_EXTRANONCE_LEN)
                .map_err(Error::FailedToProcessNewTemplateGroupChannel)?;
        }

        Ok(SendTo::Multiple(messages))
    }

    // Handles an `OpenExtendedMiningChannel` message from the downstream miner.
    //
    // This attempts to create a new dedicated extended mining channel using the
    // `PoolChannelFactory`.
    //
    // Returns
    // - `Ok(SendTo::Multiple)` - Containing success messages (`OpenExtendedMiningChannelSuccess`,
    //   `SetTarget`, `SetExtranoncePrefix`) generated by the factory.
    // - `Err(Error)` - If the factory fails (e.g., pool doesn't support extended channels, invalid
    //   parameters, lock poisoning)
    fn handle_open_extended_mining_channel(
        &mut self,
        m: OpenExtendedMiningChannel,
    ) -> Result<SendTo<()>, Error> {
        let request_id = m.get_request_id_as_u32();
        let user_identity = std::str::from_utf8(m.user_identity.as_ref())
            .map(|s| s.to_string())
            .map_err(|e| Error::InvalidUserIdentity(e.to_string()))?;

        info!("Received OpenExtendedMiningChannel: {}", m);

        let mut nominal_hash_rate = m.nominal_hash_rate;

        // Apply optional minimum downstream hashrate policy
        if let Some(min_hashrate) = self.min_downstream_hashrate {
            if nominal_hash_rate < min_hashrate {
                info!("Clamping nominal_hash_rate from {} to {} (min_downstream_hashrate policy)",
                      nominal_hash_rate, min_hashrate);
                nominal_hash_rate = min_hashrate;
            }
        }

        let requested_max_target = m.max_target.into_static();
        let requested_max_target_bitcoin = BitcoinTarget::from_le_bytes(
            requested_max_target.inner_as_ref().try_into().unwrap(),
        );
        let requested_min_rollable_extranonce_size = m.min_extranonce_size;

        let extranonce_prefix = match self
            .extranonce_prefix_factory_extended
            .safe_lock(|factory| {
                factory.next_prefix_extended(requested_min_rollable_extranonce_size.into())
            })
            .map_err(|e| Error::PoisonLock(e.to_string()))
            .and_then(|res| res.map_err(Error::ExtranoncePrefixFactoryError))
        {
            Ok(extranonce_prefix) => extranonce_prefix.to_vec(),
            Err(_) => {
                error!("OpenMiningChannelError: min-extranonce-size-too-large");
                let open_extended_mining_channel_error = OpenMiningChannelError {
                    request_id,
                    error_code: "min-extranonce-size-too-large"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                return Ok(SendTo::Respond(Mining::OpenMiningChannelError(
                    open_extended_mining_channel_error,
                )));
            }
        };

        let channel_id = self.channel_id_factory.next();
        let job_store = DefaultJobStore::new();
        let mut extended_channel = match ExtendedChannel::new_for_pool(
            channel_id,
            user_identity,
            extranonce_prefix,
            requested_max_target_bitcoin,
            nominal_hash_rate,
            true, // version rolling always allowed
            requested_min_rollable_extranonce_size,
            self.share_batch_size,
            self.shares_per_minute,
            job_store,
            self.pool_tag_string.clone(),
        ) {
            Ok(channel) => channel,
            Err(e) => match e {
                ExtendedChannelError::InvalidNominalHashrate => {
                    error!("OpenMiningChannelError: invalid-nominal-hashrate");
                    let open_extended_mining_channel_error = OpenMiningChannelError {
                        request_id,
                        error_code: "invalid-nominal-hashrate"
                            .to_string()
                            .try_into()
                            .expect("error code must be valid string"),
                    };
                    return Ok(SendTo::Respond(Mining::OpenMiningChannelError(
                        open_extended_mining_channel_error,
                    )));
                }
                ExtendedChannelError::RequestedMaxTargetOutOfRange => {
                    error!("OpenMiningChannelError: max-target-out-of-range");
                    let open_extended_mining_channel_error = OpenMiningChannelError {
                        request_id,
                        error_code: "max-target-out-of-range"
                            .to_string()
                            .try_into()
                            .expect("error code must be valid string"),
                    };
                    return Ok(SendTo::Respond(Mining::OpenMiningChannelError(
                        open_extended_mining_channel_error,
                    )));
                }
                ExtendedChannelError::RequestedMinExtranonceSizeTooLarge => {
                    error!("OpenMiningChannelError: min-extranonce-size-too-large");
                    let open_extended_mining_channel_error = OpenMiningChannelError {
                        request_id,
                        error_code: "min-extranonce-size-too-large"
                            .to_string()
                            .try_into()
                            .expect("error code must be valid string"),
                    };
                    return Ok(SendTo::Respond(Mining::OpenMiningChannelError(
                        open_extended_mining_channel_error,
                    )));
                }
                _ => {
                    error!("error in handle_open_extended_mining_channel: {:?}", e);
                    return Err(Error::FailedToCreateExtendedChannel(e));
                }
            },
        };

        let mut messages = vec![];

        let open_extended_mining_channel_success = OpenExtendedMiningChannelSuccess {
            request_id,
            channel_id,
            target: extended_channel.get_target().to_le_bytes().into(),
            extranonce_prefix: extended_channel
                .get_extranonce_prefix()
                .clone()
                .try_into()?,
            extranonce_size: extended_channel.get_rollable_extranonce_size(),
            group_channel_id: 0,
        }
        .into_static();

        messages.push(Mining::OpenExtendedMiningChannelSuccess(
            open_extended_mining_channel_success,
        ));

        if let Some(locking_key) = self.locking_key_bytes.clone() {
            let mint_manager = self.mint_manager.clone();
            let downstream_id = self.id;
            spawn_channel_registration(mint_manager, downstream_id, channel_id, locking_key, "extended");
        } else {
            debug!(
                "Skipping mint registration for extended channel {} (missing locking key)",
                channel_id
            );
        }

        let last_set_new_prev_hash_tdp = self.last_new_prev_hash.clone();

        // if the client requires custom work, we don't need to send any extended jobs
        // so we just process the SetNewPrevHash message
        if self.requires_custom_work {
            extended_channel
                .on_set_new_prev_hash(last_set_new_prev_hash_tdp)
                .map_err(Error::FailedToCreateExtendedChannel)?;
        // if the client does not require custom work, we need to send the future extended job
        // and the SetNewPrevHash message
        } else {
            let last_future_template = self.last_future_template.clone();

            let pool_coinbase_output = TxOut {
                value: Amount::from_sat(last_future_template.coinbase_tx_value_remaining),
                script_pubkey: self.coinbase_reward_script.script_pubkey(),
            };

            // create a future extended job based on the last future template
            extended_channel
                .on_new_template(last_future_template.clone(), vec![pool_coinbase_output])
                .map_err(Error::FailedToCreateExtendedChannel)?;

            let future_extended_job_id = extended_channel
                .get_future_job_id_from_template_id(last_future_template.template_id)
                .expect("future job id must exist");
            let future_extended_job = extended_channel
                .get_future_job(future_extended_job_id)
                .expect("future job must exist");

            let future_extended_job_message =
                future_extended_job.get_job_message().clone().into_static();

            // send this future job as new job message
            // to be immediately activated with the subsequent SetNewPrevHash message
            messages.push(Mining::NewExtendedMiningJob(future_extended_job_message));

            // SetNewPrevHash message activates the future job
            let prev_hash = last_set_new_prev_hash_tdp.prev_hash.clone();
            let header_timestamp = last_set_new_prev_hash_tdp.header_timestamp;
            let n_bits = last_set_new_prev_hash_tdp.n_bits;
            let set_new_prev_hash_mining = SetNewPrevHash {
                channel_id,
                job_id: future_extended_job_id,
                prev_hash,
                min_ntime: header_timestamp,
                nbits: n_bits,
            };
            extended_channel
                .on_set_new_prev_hash(last_set_new_prev_hash_tdp)
                .map_err(Error::FailedToCreateExtendedChannel)?;
            messages.push(Mining::SetNewPrevHash(set_new_prev_hash_mining));
        }

        let messages = messages.into_iter().map(SendTo::Respond).collect();

        let vardiff = VardiffState::new()?;

        self.extended_channels
            .insert(channel_id, Arc::new(RwLock::new(extended_channel)));
        self.vardiff
            .insert(channel_id, Arc::new(RwLock::new(vardiff)));

        Ok(SendTo::Multiple(messages))
    }

    // Handles an `UpdateChannel` message from the downstream miner.
    //
    // This updates the target difficulty for the specified channel based on the
    // newly provided nominal hash rate.
    //
    // Returns
    // - `Ok(SendTo::Respond)` - Containing a `SetTarget` message with the calculated new maximum
    //   target difficulty.
    // - `Err(Error)` - If calculating the target fails or the channel factory interaction fails.
    fn handle_update_channel(&mut self, m: UpdateChannel) -> Result<SendTo<()>, Error> {
        info!("Received UpdateChannel message: {}", m);

        let channel_id = m.channel_id;
        let new_nominal_hash_rate = m.nominal_hash_rate;
        let requested_maximum_target = m.maximum_target.into_static();
        let requested_maximum_target_bitcoin = BitcoinTarget::from_le_bytes(
            requested_maximum_target.inner_as_ref().try_into().unwrap(),
        );

        let is_standard_channel = self.standard_channels.contains_key(&channel_id);
        let is_extended_channel = self.extended_channels.contains_key(&channel_id);

        if is_standard_channel {
            let mut standard_channel = self
                .standard_channels
                .get(&channel_id)
                .expect("standard channel must exist")
                .write()
                .map_err(|e| Error::PoisonLock(e.to_string()))?;
            let res = standard_channel
                .update_channel(new_nominal_hash_rate, Some(requested_maximum_target_bitcoin));
            match res {
                Ok(_) => {}
                Err(e) => {
                    error!("UpdateChannelError: {:?}", e);
                    match e {
                        StandardChannelError::InvalidNominalHashrate => {
                            error!("UpdateChannelError: invalid-nominal-hashrate");
                            let update_channel_error = UpdateChannelError {
                                channel_id,
                                error_code: "invalid-nominal-hashrate"
                                    .to_string()
                                    .try_into()
                                    .expect("error code must be valid string"),
                            };
                            return Ok(SendTo::Respond(Mining::UpdateChannelError(
                                update_channel_error,
                            )));
                        }
                        StandardChannelError::RequestedMaxTargetOutOfRange => {
                            error!("UpdateChannelError: requested-max-target-out-of-range");
                            let update_channel_error = UpdateChannelError {
                                channel_id,
                                error_code: "requested-max-target-out-of-range"
                                    .to_string()
                                    .try_into()
                                    .expect("error code must be valid string"),
                            };
                            return Ok(SendTo::Respond(Mining::UpdateChannelError(
                                update_channel_error,
                            )));
                        }
                        _ => {
                            return Err(Error::FailedToUpdateStandardChannel(e));
                        }
                    }
                }
            }
            let new_target = standard_channel.get_target();
            let set_target = SetTarget {
                channel_id,
                maximum_target: new_target.to_le_bytes().into(),
            };
            Ok(SendTo::Respond(Mining::SetTarget(set_target)))
        } else if is_extended_channel {
            let mut extended_channel = self
                .extended_channels
                .get(&channel_id)
                .expect("extended channel must exist")
                .write()
                .map_err(|e| Error::PoisonLock(e.to_string()))?;
            let res = extended_channel
                .update_channel(new_nominal_hash_rate, Some(requested_maximum_target_bitcoin));
            match res {
                Ok(_) => {}
                Err(e) => {
                    error!("UpdateChannelError: {:?}", e);
                    match e {
                        ExtendedChannelError::InvalidNominalHashrate => {
                            error!("UpdateChannelError: invalid-nominal-hashrate");
                            let update_channel_error = UpdateChannelError {
                                channel_id,
                                error_code: "invalid-nominal-hashrate"
                                    .to_string()
                                    .try_into()
                                    .expect("error code must be valid string"),
                            };
                            return Ok(SendTo::Respond(Mining::UpdateChannelError(
                                update_channel_error,
                            )));
                        }
                        ExtendedChannelError::RequestedMaxTargetOutOfRange => {
                            error!("UpdateChannelError: max-target-out-of-range");
                            let update_channel_error = UpdateChannelError {
                                channel_id,
                                error_code: "max-target-out-of-range"
                                    .to_string()
                                    .try_into()
                                    .expect("error code must be valid string"),
                            };
                            return Ok(SendTo::Respond(Mining::UpdateChannelError(
                                update_channel_error,
                            )));
                        }
                        _ => {
                            return Err(Error::FailedToUpdateExtendedChannel(e));
                        }
                    }
                }
            }
            let new_target = extended_channel.get_target();
            let set_target = SetTarget {
                channel_id,
                maximum_target: new_target.to_le_bytes().into(),
            };
            Ok(SendTo::Respond(Mining::SetTarget(set_target)))
        } else {
            error!("UpdateChannelError: invalid-channel-id");
            let update_channel_error = UpdateChannelError {
                channel_id,
                error_code: "invalid-channel-id"
                    .to_string()
                    .try_into()
                    .expect("error code must be valid string"),
            };
            Ok(SendTo::Respond(Mining::UpdateChannelError(
                update_channel_error,
            )))
        }
    }

    // Handles a `SubmitSharesStandard` message from the downstream miner.
    //
    // Validates the submitted share using the `PoolChannelFactory`. If the share is valid
    // and meets the network target, it constructs a `SubmitSolution` message and sends it
    // upstream via the `solution_sender`. Responds to the miner with `SubmitSharesSuccess`
    // or `SubmitSharesError`.
    //
    // Returns
    // - `Ok(SendTo::Respond)` - Containing either `SubmitSharesSuccess` or `SubmitSharesError`.
    // - `Err(Error)` - If the channel factory interaction fails or constructing the solution fails.
    fn handle_submit_shares_standard(
        &mut self,
        m: SubmitSharesStandard,
    ) -> Result<SendTo<()>, Error> {
        info!("Received: {}", m);

        let channel_id = m.channel_id;
        if !self.standard_channels.contains_key(&channel_id) {
            let submit_shares_error = SubmitSharesError {
                channel_id,
                sequence_number: m.sequence_number,
                error_code: "invalid-channel-id"
                    .to_string()
                    .try_into()
                    .expect("error code must be valid string"),
            };
            error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: invalid-channel-id ❌", channel_id, m.sequence_number);
            return Ok(SendTo::Respond(Mining::SubmitSharesError(
                submit_shares_error,
            )));
        }

        let mut standard_channel = self
            .standard_channels
            .get(&channel_id)
            .expect("standard channel must exist")
            .write()
            .map_err(|e| Error::PoisonLock(e.to_string()))?;

        let mut vardiff = self
            .vardiff
            .get(&channel_id)
            .expect("Vardiff must exist")
            .write()
            .map_err(|e| Error::PoisonLock(e.to_string()))?;

        let res = standard_channel.validate_share(m.clone());
        vardiff.increment_shares_since_last_update();

        match res {
            Ok(ShareValidationResult::Valid(hash)) => {
                let header_hash = *hash.as_byte_array();

                if let Some(error_response) = validate_minimum_share_difficulty(&header_hash, self.minimum_share_difficulty_bits, channel_id, m.sequence_number) {
                    return Ok(error_response);
                }

                // Record share with difficulty for time-series metrics (accepted shares only).
                if let Some(stats) = self.stats_registry.get_stats(self.id) {
                    let target = standard_channel.get_target().clone();
                    let difficulty = target_to_difficulty(target);
                    stats.record_share_with_difficulty(difficulty);
                }

                send_share_quote_request(self, channel_id, m.sequence_number, header_hash, &m);

                let share_accounting = standard_channel.get_share_accounting();
                if share_accounting.should_acknowledge() {
                    let success = SubmitSharesSuccess {
                        channel_id,
                        last_sequence_number: share_accounting.get_last_share_sequence_number(),
                        new_submits_accepted_count: share_accounting.get_last_batch_accepted(),
                        new_shares_sum: share_accounting.get_last_batch_work_sum() as u64,
                    };
                    info!("SubmitSharesStandard: {} ✅", success);
                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
                } else {
                    info!(
                        "SubmitSharesStandard: valid share | channel_id: {}, sequence_number: {} ☑️",
                        channel_id, m.sequence_number
                    );
                    Ok(SendTo::None(None))
                }
            }
            Ok(ShareValidationResult::BlockFound(hash, template_id, coinbase)) => {
                info!("SubmitSharesStandard: 💰 Block Found!!! 💰");
                let header_hash = *hash.as_byte_array();

                // Record share with difficulty for time-series metrics (accepted shares only).
                if let Some(stats) = self.stats_registry.get_stats(self.id) {
                    let target = standard_channel.get_target().clone();
                    let difficulty = target_to_difficulty(target);
                    stats.record_share_with_difficulty(difficulty);
                }

                send_share_quote_request(self, channel_id, m.sequence_number, header_hash, &m);
                // if we have a template id (i.e.: this was not a custom job)
                // we can propagate the solution to the TP
                if let Some(template_id) = template_id {
                    info!("SubmitSharesStandard: Propagating solution to the Template Provider.");
                    let solution = SubmitSolution {
                        template_id,
                        version: m.version,
                        header_timestamp: m.ntime,
                        header_nonce: m.nonce,
                        coinbase_tx: coinbase.try_into()?,
                    };
                    if self.solution_sender.try_send(solution.clone()).is_err() {
                        return Err(Error::FailedToSendSolution);
                    }
                }
                let share_accounting = standard_channel.get_share_accounting();
                let success = SubmitSharesSuccess {
                    channel_id,
                    last_sequence_number: share_accounting.get_last_share_sequence_number(),
                    new_submits_accepted_count: share_accounting.get_shares_accepted(),
                    new_shares_sum: share_accounting.get_share_work_sum() as u64,
                };
                Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
            }
            Err(ShareValidationError::Invalid) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: invalid-share ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "invalid-share"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            Err(ShareValidationError::Stale) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: stale-share ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "stale-share"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            Err(ShareValidationError::InvalidJobId) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: invalid-job-id ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "invalid-job-id"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            Err(ShareValidationError::DoesNotMeetTarget) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: difficulty-too-low ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "difficulty-too-low"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            Err(ShareValidationError::DuplicateShare) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: duplicate-share ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "duplicate-share"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            _ => {
                // any other error variations should never happen
                unreachable!()
            }
        }
    }

    // Handles a `SubmitSharesExtended` message from the downstream miner.
    //
    // Similar logic to `handle_submit_shares_standard`, but for extended shares.
    // Validates the share, checks against targets, sends solutions to Template Provider if
    // applicable, and responds with `SubmitSharesSuccess` or `SubmitSharesError`.
    //
    // Returns
    // - `Ok(SendTo::Respond)` - Containing either `SubmitSharesSuccess` or `SubmitSharesError`.
    // - `Err(Error)` - If the channel factory interaction fails or constructing the solution fails.
    fn handle_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtended,
    ) -> Result<SendTo<()>, Error> {
        info!("Received: {}", m);

        let channel_id = m.channel_id;
        if !self.extended_channels.contains_key(&channel_id) {
            let error = SubmitSharesError {
                channel_id,
                sequence_number: m.sequence_number,
                error_code: "invalid-channel-id"
                    .to_string()
                    .try_into()
                    .expect("error code must be valid string"),
            };
            error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: invalid-channel-id ❌", channel_id, m.sequence_number);
            return Ok(SendTo::Respond(Mining::SubmitSharesError(error)));
        }

        let mut extended_channel = self
            .extended_channels
            .get(&channel_id)
            .expect("extended channel must exist")
            .write()
            .map_err(|e| Error::PoisonLock(e.to_string()))?;

        let mut vardiff = self
            .vardiff
            .get(&channel_id)
            .expect("Vardiff must exist")
            .write()
            .map_err(|e| Error::PoisonLock(e.to_string()))?;

        let res = extended_channel.validate_share(m.clone());
        vardiff.increment_shares_since_last_update();

        match res {
            Ok(ShareValidationResult::Valid(hash)) => {
                let header_hash = *hash.as_byte_array();

                if let Some(error_response) = validate_minimum_share_difficulty(&header_hash, self.minimum_share_difficulty_bits, channel_id, m.sequence_number) {
                    return Ok(error_response);
                }

                // Record share with difficulty for time-series metrics (accepted shares only).
                if let Some(stats) = self.stats_registry.get_stats(self.id) {
                    let target = extended_channel.get_target().clone();
                    let difficulty = target_to_difficulty(target);
                    stats.record_share_with_difficulty(difficulty);
                }

                send_extended_share_quote_request(
                    self,
                    channel_id,
                    m.sequence_number,
                    header_hash,
                    &m,
                );

                let share_accounting = extended_channel.get_share_accounting();
                if share_accounting.should_acknowledge() {
                    let success = SubmitSharesSuccess {
                        channel_id,
                        last_sequence_number: share_accounting.get_last_share_sequence_number(),
                        new_submits_accepted_count: share_accounting.get_last_batch_accepted(),
                        new_shares_sum: share_accounting.get_last_batch_work_sum() as u64,
                    };
                    info!("SubmitSharesExtended: {} ✅", success);
                    Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
                } else {
                    info!(
                        "SubmitSharesExtended: valid share | channel_id: {}, sequence_number: {} ☑️",
                        channel_id, m.sequence_number
                    );
                    Ok(SendTo::None(None))
                }
            }
            Ok(ShareValidationResult::BlockFound(hash, template_id, coinbase)) => {
                info!("SubmitSharesExtended: 💰 Block Found!!! 💰");
                let header_hash = *hash.as_byte_array();

                // Record share with difficulty for time-series metrics (accepted shares only).
                if let Some(stats) = self.stats_registry.get_stats(self.id) {
                    let target = extended_channel.get_target().clone();
                    let difficulty = target_to_difficulty(target);
                    stats.record_share_with_difficulty(difficulty);
                }

                send_extended_share_quote_request(
                    self,
                    channel_id,
                    m.sequence_number,
                    header_hash,
                    &m,
                );
                // if we have a template id (i.e.: this was not a custom job)
                // we can propagate the solution to the TP
                if let Some(template_id) = template_id {
                    info!("SubmitSharesExtended: Propagating solution to the Template Provider.");
                    let solution = SubmitSolution {
                        template_id,
                        version: m.version,
                        header_timestamp: m.ntime,
                        header_nonce: m.nonce,
                        coinbase_tx: coinbase.try_into()?,
                    };
                    if self.solution_sender.try_send(solution.clone()).is_err() {
                        return Err(Error::FailedToSendSolution);
                    }
                }
                let share_accounting = extended_channel.get_share_accounting();
                let success = SubmitSharesSuccess {
                    channel_id,
                    last_sequence_number: share_accounting.get_last_share_sequence_number(),
                    new_submits_accepted_count: share_accounting.get_shares_accepted(),
                    new_shares_sum: share_accounting.get_share_work_sum() as u64,
                };
                Ok(SendTo::Respond(Mining::SubmitSharesSuccess(success)))
            }
            Err(ShareValidationError::Invalid) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: invalid-share ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "invalid-share"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            Err(ShareValidationError::Stale) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: stale-share ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "stale-share"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            Err(ShareValidationError::InvalidJobId) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: invalid-job-id ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "invalid-job-id"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            Err(ShareValidationError::DoesNotMeetTarget) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: difficulty-too-low ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "difficulty-too-low"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            Err(ShareValidationError::DuplicateShare) => {
                error!("SubmitSharesError: channel_id: {}, sequence_number: {}, error_code: duplicate-share ❌", channel_id, m.sequence_number);
                let error = SubmitSharesError {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: "duplicate-share"
                        .to_string()
                        .try_into()
                        .expect("error code must be valid string"),
                };
                Ok(SendTo::Respond(Mining::SubmitSharesError(error)))
            }
            _ => {
                // any other error variations should never happen
                unreachable!()
            }
        }
    }

    // Handles a `SetCustomMiningJob` message from the downstream miner.
    //
    // This informs the `PoolChannelFactory` about the custom job details provided
    // by the miner for a specific channel.
    //
    // Returns
    // - `Ok(SendTo::Respond)` - Containing a `SetCustomMiningJobSuccess` message acknowledging the
    //   custom job setup.
    // - `Err(Error)` - If the channel factory interaction fails.
    fn handle_set_custom_mining_job(&mut self, m: SetCustomMiningJob) -> Result<SendTo<()>, Error> {
        info!("Received SetCustomMiningJob: {}", m);

        // this is a naive implementation, but ideally we should check the SetCustomMiningJob
        // message parameters, especially:
        // - the mining_job_token
        // - the amount of the pool payout output

        let custom_job_coinbase_outputs = Vec::<TxOut>::consensus_decode(
            &mut m.coinbase_tx_outputs.inner_as_ref().to_vec().as_slice(),
        )
        .map_err(|_| Error::FailedToDeserializeCoinbaseOutputs)?;

        // check that the script_pubkey from self.coinbase_reward_script
        // is present in the custom job coinbase outputs
        let missing_script = !custom_job_coinbase_outputs.iter().any(|pool_output| {
            *pool_output.script_pubkey == *self.coinbase_reward_script.script_pubkey()
        });

        if missing_script {
            error!("SetCustomMiningJobError: pool-payout-script-missing");

            let error = SetCustomMiningJobError {
                request_id: m.request_id,
                channel_id: m.channel_id,
                error_code: "pool-payout-script-missing"
                    .to_string()
                    .try_into()
                    .expect("error code must be valid string"),
            };

            return Ok(SendTo::Respond(Mining::SetCustomMiningJobError(error)));
        }

        let channel_id = m.channel_id;
        if !self.extended_channels.contains_key(&channel_id) {
            error!("SetCustomMiningJobError: invalid-channel-id");
            let error = SetCustomMiningJobError {
                request_id: m.request_id,
                channel_id,
                error_code: "invalid-channel-id"
                    .to_string()
                    .try_into()
                    .expect("error code must be valid string"),
            };
            return Ok(SendTo::Respond(Mining::SetCustomMiningJobError(error)));
        }

        let mut extended_channel = self
            .extended_channels
            .get(&channel_id)
            .expect("extended channel must exist")
            .write()
            .map_err(|e| Error::PoisonLock(e.to_string()))?;

        let job_id = extended_channel
            .on_set_custom_mining_job(m.clone().into_static())
            .map_err(Error::FailedToSetCustomMiningJob)?;

        let success = SetCustomMiningJobSuccess {
            channel_id,
            request_id: m.request_id,
            job_id,
        };
        Ok(SendTo::Respond(Mining::SetCustomMiningJobSuccess(success)))
    }

    fn handle_close_channel(&mut self, m: CloseChannel) -> Result<SendTo<()>, Error> {
        info!("Received Close Channel: {m}");
        self.extended_channels.remove(&m.channel_id);
        self.standard_channels.remove(&m.channel_id);
        self.vardiff.remove(&m.channel_id);
        Ok(SendTo::None(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_dispatch_error_missing_locking_key() {
        let error = QuoteDispatchError::MissingLockingKey(42);
        assert_eq!(error.to_string(), "Missing locking key for channel 42");
    }

    #[test]
    fn test_quote_dispatch_error_invalid_locking_key_format() {
        let error = QuoteDispatchError::InvalidLockingKeyFormat {
            channel_id: 42,
            length: 32,
        };
        assert!(error.to_string().contains("channel 42"));
        assert!(error.to_string().contains("expected 33 bytes"));
        assert!(error.to_string().contains("got 32"));
    }

    #[test]
    fn test_quote_dispatch_error_invalid_locking_key() {
        let error = QuoteDispatchError::InvalidLockingKey {
            channel_id: 42,
            reason: "invalid checksum".to_string(),
        };
        assert!(error.to_string().contains("channel 42"));
        assert!(error.to_string().contains("invalid checksum"));
    }

    #[test]
    fn test_quote_dispatch_error_mint_dispatcher_unavailable() {
        let error = QuoteDispatchError::MintDispatcherUnavailable;
        assert_eq!(error.to_string(), "Mint dispatcher is not available");
    }

    #[test]
    fn test_quote_dispatch_error_dispatch_failed() {
        let error = QuoteDispatchError::QuoteDispatchFailed("network timeout".to_string());
        assert!(error.to_string().contains("Quote dispatch failed"));
        assert!(error.to_string().contains("network timeout"));
    }

    #[test]
    fn test_quote_dispatch_error_is_error_trait() {
        let error = QuoteDispatchError::MissingLockingKey(1);
        let _: &dyn std::error::Error = &error;
    }
}
