pub mod error;
pub mod mining_pool;
pub mod status;
pub mod template_receiver;

use std::{convert::TryInto, net::SocketAddr, sync::Arc};

use async_channel::{bounded, unbounded};

use error::PoolError;
use mining_pool::{get_coinbase_output, Configuration, Pool};
use mint_pool_messaging::{MessagingConfig, MintPoolMessageHub, Role};
use roles_logic_sv2::utils::Mutex;
use shared_config::Sv2MessagingConfig;
use template_receiver::TemplateRx;
use tracing::{error, info, warn};

use tokio::select;
use std::{thread, time::Duration};

#[derive(Clone)]
pub struct PoolSv2 {
    config: Configuration,
    sv2_messaging_config: Option<Sv2MessagingConfig>,
}

// TODO remove after porting mint to use Sv2 data types
impl std::fmt::Debug for PoolSv2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolSv2")
            .field("config", &self.config)
            .field("mint", &"debug not implemented")
            .field("sv2_messaging_config", &self.sv2_messaging_config)
            .finish()
    }
}

impl PoolSv2 {
    pub fn new(config: Configuration, sv2_messaging_config: Option<Sv2MessagingConfig>) -> PoolSv2 {
        PoolSv2 {
            config,
            sv2_messaging_config,
        }
    }

    pub async fn start(&mut self) -> Result<(), PoolError> {
        let config = self.config.clone();
        let (status_tx, status_rx) = unbounded();
        let (s_new_t, r_new_t) = bounded(10);
        let (s_prev_hash, r_prev_hash) = bounded(10);
        let (s_solution, r_solution) = bounded(10);
        let (s_message_recv_signal, r_message_recv_signal) = bounded(10);
        let coinbase_output_result = get_coinbase_output(&config);
        let coinbase_output_len = coinbase_output_result?.len() as u32;
        let tp_authority_public_key = config.tp_authority_public_key;
        let tp_address: SocketAddr = config.tp_address.parse().unwrap();
        
        // Debugging information
        dbg!(&tp_address, &tp_authority_public_key, &coinbase_output_len);

        let template_rx_res = TemplateRx::connect(
            config.tp_address.parse().unwrap(),
            s_new_t,
            s_prev_hash,
            r_solution,
            r_message_recv_signal,
            status::Sender::Upstream(status_tx.clone()),
            coinbase_output_len,
            tp_authority_public_key,
        )
        .await;

        if let Err(e) = template_rx_res {
            error!("Could not connect to Template Provider: {}", e);
            return Err(e);
        }

        // Initialize SV2 messaging hub if enabled
        let sv2_hub = if let Some(ref sv2_config) = self.sv2_messaging_config {
            if sv2_config.enabled {
                let messaging_config = MessagingConfig {
                    broadcast_buffer_size: sv2_config.broadcast_buffer_size,
                    mpsc_buffer_size: sv2_config.mpsc_buffer_size,
                    max_retries: sv2_config.max_retries,
                    timeout_ms: sv2_config.timeout_ms,
                };
                
                let hub = MintPoolMessageHub::new(messaging_config);
                // Register this pool as a pool connection
                hub.register_connection("pool-main".to_string(), Role::Pool).await;
                info!("SV2 messaging hub initialized and pool registered");
                Some(hub)
            } else {
                info!("SV2 messaging is disabled in configuration");
                None
            }
        } else {
            info!("No SV2 messaging configuration found");
            None
        };

        let pool = Pool::start(
            config.clone(),
            r_new_t,
            r_prev_hash,
            s_solution,
            s_message_recv_signal,
            status::Sender::DownstreamListener(status_tx),
            sv2_hub,
            self.sv2_messaging_config.clone(),
        );

        // Start the error handling loop
        // See `./status.rs` and `utils/error_handling` for information on how this operates
        loop {
            let task_status = select! {
                task_status = status_rx.recv() => task_status,
                interrupt_signal = tokio::signal::ctrl_c() => {
                    match interrupt_signal {
                        Ok(()) => {
                            info!("Interrupt received");
                        },
                        Err(err) => {
                            error!("Unable to listen for interrupt signal: {}", err);
                            // we also shut down in case of error
                        },
                    }
                    break Ok(());
                }
            };
            let task_status: status::Status = task_status.unwrap();

            match task_status.state {
                // Should only be sent by the downstream listener
                status::State::DownstreamShutdown(err) => {
                    error!(
                        "SHUTDOWN from Downstream: {}\nTry to restart the downstream listener",
                        err
                    );
                    break Ok(());
                }
                status::State::TemplateProviderShutdown(err) => {
                    error!("SHUTDOWN from Upstream: {}\nTry to reconnecting or connecting to a new upstream", err);
                    break Ok(());
                }
                status::State::Healthy(msg) => {
                    info!("HEALTHY message: {}", msg);
                }
                status::State::DownstreamInstanceDropped(downstream_id) => {
                    warn!("Dropping downstream instance {} from pool", downstream_id);
                    if pool
                        .safe_lock(|p| p.remove_downstream(downstream_id))
                        .is_err()
                    {
                        break Ok(());
                    }
                }
            }
        }
    }
}
