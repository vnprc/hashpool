pub mod error;
pub mod mining_pool;
pub mod status;
pub mod stats;
pub mod template_receiver;
pub mod web;

use std::net::SocketAddr;

use async_channel::{bounded, unbounded};

use error::PoolError;
use mining_pool::{get_coinbase_output, Configuration, Pool};
use shared_config::{Sv2MessagingConfig, EhashConfig};
use template_receiver::TemplateRx;
use tracing::{error, info, warn};

use tokio::select;

#[derive(Clone)]
pub struct PoolSv2 {
    config: Configuration,
    sv2_messaging_config: Option<Sv2MessagingConfig>,
    ehash_config: Option<EhashConfig>,
}

// TODO remove after porting mint to use Sv2 data types
impl std::fmt::Debug for PoolSv2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolSv2")
            .field("config", &self.config)
            .field("mint", &"debug not implemented")
            .field("sv2_messaging_config", &self.sv2_messaging_config)
            .field("ehash_config", &self.ehash_config)
            .finish()
    }
}

impl PoolSv2 {
    pub fn new(config: Configuration, sv2_messaging_config: Option<Sv2MessagingConfig>, ehash_config: Option<EhashConfig>) -> PoolSv2 {
        // PHASE 1: Initialize message interceptor for TLV extraction
        #[cfg(feature = "extension_hooks")]
        {
            let interceptor = ehash_extension::EhashMessageInterceptor::new();
            set_message_interceptor(Box::new(interceptor));
            tracing::info!("Initialized ehash extension interceptor for TLV extraction");
        }
        
        PoolSv2 {
            config,
            sv2_messaging_config,
            ehash_config,
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

        let pool = Pool::start(
            config.clone(),
            r_new_t,
            r_prev_hash,
            s_solution,
            s_message_recv_signal,
            status::Sender::DownstreamListener(status_tx.clone()),
            self.sv2_messaging_config.clone(),
            self.ehash_config.clone(),
        );

        // Start web server on port 8081 (different from proxy's 3030 and any 8080 services)
        info!("Initializing pool web server...");
        let web_server = web::WebServer::new(pool.clone(), 8081);
        tokio::spawn(async move {
            info!("Starting pool web server task...");
            if let Err(e) = web_server.start().await {
                error!("Pool web server error: {}", e);
            }
        });

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

// PHASE 1: External extension support infrastructure
#[cfg(feature = "extension_hooks")]
use std::sync::OnceLock;

#[cfg(feature = "extension_hooks")]
static MESSAGE_INTERCEPTOR: OnceLock<Box<dyn ehash_extension::MessageInterceptor + Send + Sync>> = OnceLock::new();

#[cfg(feature = "extension_hooks")]
pub fn set_message_interceptor(interceptor: Box<dyn ehash_extension::MessageInterceptor + Send + Sync>) {
    tracing::info!("🔧 set_message_interceptor called with extension_hooks feature enabled");
    match MESSAGE_INTERCEPTOR.set(interceptor) {
        Ok(()) => {
            tracing::info!("✅ Message interceptor set successfully");
            // Immediately test if we can retrieve it
            if MESSAGE_INTERCEPTOR.get().is_some() {
                tracing::info!("✅ Immediate retrieval test: interceptor found");
            } else {
                tracing::error!("❌ Immediate retrieval test: interceptor NOT found");
            }
        }
        Err(_) => tracing::error!("❌ Failed to set message interceptor - already set"),
    }
}

#[cfg(feature = "extension_hooks")]
pub fn get_message_interceptor() -> Option<&'static Box<dyn ehash_extension::MessageInterceptor + Send + Sync>> {
    let result = MESSAGE_INTERCEPTOR.get();
    tracing::debug!("🔍 get_message_interceptor called, result: {}", if result.is_some() { "Some(interceptor)" } else { "None" });
    result
}

#[cfg(not(feature = "extension_hooks"))]
pub fn get_message_interceptor() -> Option<&'static ()> {
    None
}
