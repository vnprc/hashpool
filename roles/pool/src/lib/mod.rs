pub mod error;
pub mod mining_pool;
pub mod status;
pub mod template_receiver;

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use async_channel::{bounded, unbounded};

use error::PoolError;
use mining_pool::{get_coinbase_output, Configuration, Pool};
use roles_logic_sv2::utils::Mutex;
use template_receiver::TemplateRx;
use tracing::{error, info, warn};

use tokio::select;
use cdk::{cdk_database::mint_memory::MintMemoryDatabase, nuts::{CurrencyUnit, MintInfo, Nuts}, Mint};
use bip39::Mnemonic;


#[derive(Debug, Clone)]
pub struct PoolSv2 {
    config: Configuration,
    mint: Option<Arc<Mutex<Mint>>>,
    keyset_id: Option<u64>
}

impl PoolSv2 {
    pub fn new(config: Configuration) -> PoolSv2 {
        PoolSv2 {
            config,
            mint: None,
            keyset_id: None,
        }
    }

    pub async fn start(mut self) -> Result<(), PoolError> {
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
    
        let mint = Arc::new(Mutex::new(self.create_mint().await));
        self.mint = Some(mint.clone());

        let pool = Pool::start(
            config.clone(),
            r_new_t,
            r_prev_hash,
            s_solution,
            s_message_recv_signal,
            status::Sender::DownstreamListener(status_tx),
            mint.clone(),
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

    async fn create_mint(&self) -> Mint {
        let nuts = Nuts::new().nut07(true);

        let mint_info = MintInfo::new().nuts(nuts);

        // TODO securely import mnemonic
        let mnemonic = Mnemonic::generate(12).unwrap();

        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::Hash, (0, 64));

        let mint = Mint::new(
            // TODO is mint_url necessary?
            "http://localhost:8000",
            &mnemonic.to_seed_normalized(""),
            mint_info,
            Arc::new(MintMemoryDatabase::default()),
            supported_units,
        )
        .await.unwrap();

        mint
    }

}
