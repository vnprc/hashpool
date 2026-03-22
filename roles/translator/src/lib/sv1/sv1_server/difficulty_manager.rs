use std::sync::Arc;

use crate::{is_aggregated, is_non_aggregated, sv1::sv1_server::sv1_server::PendingTargetUpdate};

use stratum_apps::{
    stratum_core::{
        bitcoin::Target,
        channels_sv2::{target::hash_rate_to_target, Vardiff},
        mining_sv2::{SetTarget, UpdateChannel},
        parsers_sv2::Mining,
        stratum_translation::sv2_to_sv1::build_sv1_set_difficulty_from_sv2_target,
    },
    utils::types::{ChannelId, DownstreamId, Hashrate},
};
use tracing::{debug, error, info, trace, warn};

use crate::sv1::Sv1Server;

enum AggregatedSnapshot {
    Active {
        total_hashrate: Hashrate,
        min_target: Target,
    },
    NoDownstreams,
}

impl Sv1Server {
    /// Spawns the variable difficulty adjustment loop.
    ///
    /// This method implements the SV1 server's variable difficulty logic for all downstreams.
    /// Every 60 seconds, this method updates the difficulty state for each downstream.
    pub async fn spawn_vardiff_loop(self: Arc<Self>) {
        info!("Variable difficulty adjustment enabled - starting vardiff loop");

        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            info!("Starting vardiff loop for downstreams");

            self.handle_vardiff_updates().await;
        }
    }

    /// Handles variable difficulty adjustments for all connected downstreams.
    ///
    /// This method implements the core vardiff logic:
    /// 1. For each downstream, calculate if a target update is needed
    /// 2. Always send UpdateChannel to keep upstream informed
    /// 3. Compare new target with upstream target to decide when to send set_difficulty:
    ///    - If new_target >= upstream_target: send set_difficulty immediately
    ///    - If new_target < upstream_target: wait for SetTarget response before sending
    ///      set_difficulty
    /// 4. Handle aggregated vs non-aggregated modes for UpdateChannel messages
    async fn handle_vardiff_updates(&self) {
        let mut immediate_updates = Vec::new();
        let mut all_updates = Vec::new(); // All updates will generate UpdateChannel messages

        for vardiff_key_pair in self.vardiff.iter() {
            let downstream_id = vardiff_key_pair.key();
            let vardiff = vardiff_key_pair.value();
            debug!("Updating vardiff for downstream_id: {}", downstream_id);
            let Some(downstream) = self.downstreams.get(downstream_id) else {
                continue;
            };
            let (channel_id, hashrate, target, upstream_target) =
                downstream.downstream_data.super_safe_lock(|data| {
                    // It's safe to unwrap hashrate because we know that
                    // the downstream has a hashrate (we are
                    // doing vardiff)
                    (
                        data.channel_id,
                        data.hashrate.unwrap(),
                        data.target,
                        data.upstream_target,
                    )
                });

            let Some(channel_id) = channel_id else {
                error!("Channel id is none for downstream_id: {}", downstream_id);
                continue;
            };
            let new_hashrate_opt = vardiff.super_safe_lock(|state| {
                state.try_vardiff(hashrate, &target, self.shares_per_minute)
            });

            if let Ok(Some(new_hashrate)) = new_hashrate_opt {
                // Calculate new target based on new hashrate
                let new_target: Target =
                    match hash_rate_to_target(new_hashrate as f64, self.shares_per_minute as f64) {
                        Ok(target) => target,
                        Err(e) => {
                            error!(
                                "Failed to calculate target for hashrate {}: {:?}",
                                new_hashrate, e
                            );
                            continue;
                        }
                    };
                // Always update the downstream's pending target and hashrate
                if let Some(d) = self.downstreams.get(downstream_id) {
                    _ = d.downstream_data.safe_lock(|data| {
                        data.set_pending_target(new_target, d.downstream_id);
                        data.set_pending_hashrate(Some(new_hashrate), d.downstream_id);
                    });
                }
                // All updates will be sent as UpdateChannel messages
                all_updates.push((*downstream_id, channel_id, new_target, new_hashrate));
                // Determine if we should send set_difficulty immediately or wait
                match upstream_target {
                    Some(upstream_target) => {
                        if new_target >= upstream_target {
                            // Case 1: new_target >= upstream_target, send set_difficulty
                            // immediately
                            trace!(
                                "✅ Target comparison: new_target ({:?}) >= upstream_target ({:?}) for downstream {}, will send set_difficulty immediately",
                                new_target, upstream_target, downstream_id
                            );
                            immediate_updates.push((channel_id, Some(*downstream_id), new_target));
                        } else {
                            // Case 2: new_target < upstream_target, delay set_difficulty until
                            // SetTarget
                            trace!(
                                "⏳ Target comparison: new_target ({:?}) < upstream_target ({:?}) for downstream {}, will delay set_difficulty until SetTarget",
                                new_target, upstream_target, downstream_id
                            );
                            self.pending_target_updates.super_safe_lock(|data| {
                                data.push(PendingTargetUpdate {
                                    downstream_id: *downstream_id,
                                    new_target,
                                    new_hashrate,
                                })
                            });
                        }
                    }
                    None => {
                        // No upstream target set yet, send set_difficulty immediately as fallback
                        trace!(
                            "No upstream target set for downstream {}, will send set_difficulty immediately",
                            downstream_id
                        );
                        immediate_updates.push((channel_id, Some(*downstream_id), new_target));
                    }
                }
            }
        }

        // Send UpdateChannel messages for ALL updates (both immediate and delayed)
        if !all_updates.is_empty() {
            self.send_update_channel_messages(all_updates).await;
        }

        // Process immediate set_difficulty updates (for new_target >= upstream_target)
        for (channel_id, downstream_id, target) in immediate_updates {
            // Send set_difficulty message immediately
            if let Ok(set_difficulty_msg) = build_sv1_set_difficulty_from_sv2_target(target) {
                if let Err(e) = self
                    .sv1_server_channel_state
                    .sv1_server_to_downstream_sender
                    .send((channel_id, downstream_id, set_difficulty_msg))
                {
                    error!(
                        "Failed to send immediate SetDifficulty message to downstream {}: {:?}",
                        downstream_id.unwrap_or(0),
                        e
                    );
                } else {
                    trace!(
                        "Sent immediate SetDifficulty to downstream {} (new_target >= upstream_target)",
                        downstream_id.unwrap_or(0)
                    );
                }
            }
        }
    }

    /// Sends UpdateChannel messages for all target updates.
    ///
    /// Always sends UpdateChannel to keep upstream informed about target changes.
    /// Handles both aggregated and non-aggregated modes:
    /// - Aggregated: Send single UpdateChannel with minimum target and sum of hashrates
    /// - Non-aggregated: Send individual UpdateChannel for each downstream
    async fn send_update_channel_messages(
        &self,
        all_updates: Vec<(DownstreamId, ChannelId, Target, Hashrate)>, /* (downstream_id,
                                                                        * channel_id,
                                                                        * new_target,
                                                                        * new_hashrate) */
    ) {
        if is_aggregated() {
            // Aggregated mode: Send single UpdateChannel with minimum target and total hashrate of
            // ALL downstreams
            self.send_aggregated_update_channel(all_updates).await;
        } else {
            // Non-aggregated mode: Send individual UpdateChannel for each downstream
            self.send_non_aggregated_update_channels(all_updates).await;
        }
    }

    async fn send_aggregated_update_channel(
        &self,
        all_updates: Vec<(DownstreamId, ChannelId, Target, Hashrate)>,
    ) {
        // Nothing to do if we received no updates
        let Some((_, channel_id, _, _)) = all_updates.first() else {
            return;
        };

        if self.downstreams.is_empty() {
            return;
        }

        let mut min_target: Option<Target> = None;
        let mut total_hashrate: Hashrate = 0.0;

        for downstream in self.downstreams.iter() {
            let downstream = downstream.value();
            downstream.downstream_data.super_safe_lock(|d| {
                let target = *d.pending_target.as_ref().unwrap_or(&d.target);
                let hashrate = d
                    .pending_hashrate
                    .unwrap_or_else(|| d.hashrate.expect("vardiff implies hashrate"));

                min_target = Some(match min_target {
                    Some(current) => current.min(target),
                    None => target,
                });

                total_hashrate += hashrate;
            });
        }

        let min_target = min_target.expect("at least one downstream must exist");
        let downstream_count = self.downstreams.len();

        let update_channel = UpdateChannel {
            channel_id: *channel_id,
            nominal_hash_rate: total_hashrate,
            maximum_target: min_target.to_le_bytes().into(),
        };

        debug!(
            "Sending aggregated UpdateChannel: channel_id={}, total_hashrate={}, min_target={:?}, downstreams={}, vardiff_updates={}",
            channel_id,
            total_hashrate,
            min_target,
            downstream_count,
            all_updates.len()
        );

        if let Err(e) = self
            .sv1_server_channel_state
            .channel_manager_sender
            .send((Mining::UpdateChannel(update_channel), None))
            .await
        {
            error!("Failed to send aggregated UpdateChannel: {:?}", e);
        }
    }

    async fn send_non_aggregated_update_channels(
        &self,
        all_updates: Vec<(DownstreamId, ChannelId, Target, Hashrate)>,
    ) {
        for (downstream_id, channel_id, new_target, new_hashrate) in all_updates {
            let update_channel = UpdateChannel {
                channel_id,
                nominal_hash_rate: new_hashrate,
                maximum_target: new_target.to_le_bytes().into(),
            };

            debug!(
                "Sending UpdateChannel for downstream {}: channel_id={}, hashrate={}, target={:?}",
                downstream_id, channel_id, new_hashrate, new_target
            );

            if let Err(e) = self
                .sv1_server_channel_state
                .channel_manager_sender
                .send((Mining::UpdateChannel(update_channel), None))
                .await
            {
                error!(
                    "Failed to send UpdateChannel for downstream {}: {:?}",
                    downstream_id, e
                );
            }
        }
    }

    /// Handles SetTarget messages from the ChannelManager.
    ///
    /// Aggregated mode: Single SetTarget updates all downstreams and processes all pending updates
    /// Non-aggregated mode: Each SetTarget updates one specific downstream and processes its
    /// pending update
    pub async fn handle_set_target_message(&self, set_target: SetTarget<'_>) {
        let new_upstream_target =
            Target::from_le_bytes(set_target.maximum_target.inner_as_ref().try_into().unwrap());
        debug!(
            "Received SetTarget for channel {}: new_upstream_target = {:?}",
            set_target.channel_id, new_upstream_target
        );

        if is_aggregated() {
            return self
                .handle_aggregated_set_target(new_upstream_target, set_target.channel_id)
                .await;
        }

        self.handle_non_aggregated_set_target(set_target.channel_id, new_upstream_target)
            .await;
    }

    /// Handles SetTarget in aggregated mode.
    /// Updates all downstreams and processes all pending set_difficulty messages.
    async fn handle_aggregated_set_target(
        &self,
        new_upstream_target: Target,
        channel_id: ChannelId,
    ) {
        debug!("Aggregated mode: Updating upstream target for all downstreams");

        for downstream in self.downstreams.iter() {
            let downstream = downstream.value();
            downstream.downstream_data.super_safe_lock(|d| {
                d.set_upstream_target(new_upstream_target, downstream.downstream_id);
            });
        }

        // Process ALL pending difficulty updates that can now be sent downstream
        let applicable_updates =
            self.get_pending_difficulty_updates(new_upstream_target, None, channel_id);

        self.send_pending_set_difficulty_messages_to_downstream(applicable_updates)
            .await;
    }

    /// Handles SetTarget in non-aggregated mode.
    /// Updates the specific downstream and processes its pending set_difficulty message.
    async fn handle_non_aggregated_set_target(
        &self,
        channel_id: ChannelId,
        new_upstream_target: Target,
    ) {
        debug!(
            "Non-aggregated mode: Processing SetTarget for channel {}",
            channel_id
        );

        let affected = self.downstreams.iter().find(|downstream| {
            downstream
                .downstream_data
                .super_safe_lock(|d| d.channel_id == Some(channel_id))
        });

        let Some(downstream) = affected else {
            warn!("No downstream found for channel {}", channel_id);
            return;
        };

        let downstream_id = downstream.downstream_id;

        downstream.downstream_data.super_safe_lock(|d| {
            d.set_upstream_target(new_upstream_target, downstream_id);
        });

        trace!("Updated upstream target for downstream {}", downstream_id);

        let applicable_updates = self.get_pending_difficulty_updates(
            new_upstream_target,
            Some(downstream_id),
            channel_id,
        );

        self.send_pending_set_difficulty_messages_to_downstream(applicable_updates)
            .await;
    }

    /// Gets pending updates that can now be applied based on the new upstream target.
    /// If downstream_id is provided, only returns updates for that specific downstream.
    /// Logs a warning if the upstream target is higher than any requested target.
    fn get_pending_difficulty_updates(
        &self,
        new_upstream_target: Target,
        downstream_id: Option<DownstreamId>,
        channel_id: ChannelId,
    ) -> Vec<PendingTargetUpdate> {
        let mut applicable_updates = Vec::new();

        self.pending_target_updates.super_safe_lock(|data| {
            data.retain(|pending_update| {
                // Check if we should process this update
                let should_process = match downstream_id {
                    Some(downstream_id) => pending_update.downstream_id == downstream_id,
                    None => true, // Process all in aggregated mode
                };

                if !should_process {
                    return true; // keep in pending list (not relevant for this SetTarget)
                }

                if pending_update.new_target >= new_upstream_target {
                    // Target is acceptable, can apply immediately
                    applicable_updates.push(pending_update.clone());
                    false // remove from pending list
                } else {
                    // WARNING: Upstream gave us a target higher than what we requested
                    error!(
                        "❌ Protocol issue: SetTarget response has target ({:?}) which is higher than requested target ({:?}) in UpdateChannel for channel {:?}. Ignoring this pending update for downstream {:?}.",
                        new_upstream_target, pending_update.new_target, channel_id, pending_update.downstream_id
                    );
                    false // remove from pending list (don't keep invalid requests)
                }
            });
        });
        applicable_updates
    }

    /// Sends set_difficulty messages for all applicable pending updates.
    async fn send_pending_set_difficulty_messages_to_downstream(
        &self,
        difficulty_updates: Vec<PendingTargetUpdate>,
    ) {
        for update in difficulty_updates {
            let channel_id = self
                .downstreams
                .get(&update.downstream_id)
                .and_then(|ds| ds.downstream_data.super_safe_lock(|d| d.channel_id));

            let Some(channel_id) = channel_id else {
                trace!(
                    "Skipping SetDifficulty for downstream {}: no channel_id yet",
                    update.downstream_id
                );
                continue;
            };

            let set_difficulty_msg =
                match build_sv1_set_difficulty_from_sv2_target(update.new_target) {
                    Ok(msg) => msg,
                    Err(e) => {
                        error!(
                            "Failed to build SetDifficulty for downstream {}: {:?}",
                            update.downstream_id, e
                        );
                        continue;
                    }
                };

            if let Err(e) = self
                .sv1_server_channel_state
                .sv1_server_to_downstream_sender
                .send((channel_id, Some(update.downstream_id), set_difficulty_msg))
            {
                error!(
                    "Failed to send SetDifficulty to downstream {}: {:?}",
                    update.downstream_id, e
                );
            } else {
                trace!("Sent SetDifficulty to downstream {}", update.downstream_id);
            }
        }
    }

    /// Sends an UpdateChannel message for aggregated mode when downstream state changes
    /// (e.g., disconnect). Calculates total hashrate and minimum target among all remaining
    /// downstreams.
    pub async fn send_update_channel_on_downstream_state_change(&self) {
        if is_non_aggregated() {
            return;
        }

        let is_empty = self.downstreams.is_empty();

        let snapshot = if is_empty {
            AggregatedSnapshot::NoDownstreams
        } else {
            let mut total_hashrate: Hashrate = 0.0;
            let mut min_target: Option<Target> = None;

            for downstream in self.downstreams.iter() {
                let downstream = downstream.value();
                downstream.downstream_data.super_safe_lock(|d| {
                    let hashrate = d.pending_hashrate.unwrap_or_else(|| {
                        d.hashrate
                            .expect("vardiff implies downstream must have a hashrate")
                    });

                    let target = *d.pending_target.as_ref().unwrap_or(&d.target);

                    total_hashrate += hashrate;
                    min_target = Some(match min_target {
                        Some(current) => current.min(target),
                        None => target,
                    });
                });
            }

            AggregatedSnapshot::Active {
                total_hashrate,
                min_target: min_target.expect("downstreams is non-empty"),
            }
        };

        let update = match snapshot {
            AggregatedSnapshot::Active {
                total_hashrate,
                min_target,
            } => UpdateChannel {
                channel_id: 0, // ChannelManager will rewrite to upstream extended channel id
                nominal_hash_rate: total_hashrate,
                maximum_target: min_target.to_le_bytes().into(),
            },

            AggregatedSnapshot::NoDownstreams => UpdateChannel {
                channel_id: 0,
                nominal_hash_rate: 0.0,
                maximum_target: [0xFF; 32].into(),
            },
        };

        if let Err(e) = self
            .sv1_server_channel_state
            .channel_manager_sender
            .send((Mining::UpdateChannel(update), None))
            .await
        {
            error!(
                "Failed to send UpdateChannel after downstream state change: {:?}",
                e
            );
        }
    }
}
