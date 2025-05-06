pub mod error;
pub mod mining_pool;
pub mod status;
pub mod template_receiver;

use std::{convert::TryInto, net::SocketAddr, sync::Arc};

use async_channel::{bounded, unbounded};

use error::PoolError;
use mining_pool::{get_coinbase_output, Configuration, Pool};
use mining_sv2::cashu::{KeysetId, Sv2KeySet};
use roles_logic_sv2::utils::Mutex;
use template_receiver::TemplateRx;
use tracing::{error, info, warn};

use tokio::select;
use cdk::{nuts::{CurrencyUnit, KeySet, Keys}, Amount, amount::AmountStr};

use std::collections::BTreeMap;
use cdk::util::hex;
use cdk::nuts::PublicKey;
use std::convert::TryFrom;

#[derive(Clone)]
pub struct PoolSv2<'decoder> {
    config: Configuration,
    keyset: Option<Arc<Mutex<Sv2KeySet<'decoder>>>>,
}

// TODO remove after porting mint to use Sv2 data types
impl<'decoder> std::fmt::Debug for PoolSv2<'decoder> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolSv2")
            .field("config", &self.config)
            .field("keyset", &self.keyset)
            .field("mint", &"debug not implemented")
            .finish()
    }
}

impl PoolSv2<'_> {
    pub fn new(config: Configuration) -> PoolSv2<'static> {
        PoolSv2 {
            config,
            keyset: None,
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
    
        use redis::Commands;
        use std::{thread, time::Duration};

        let redis_url = "redis://localhost:6379";
        let redis_key = "mint:keyset:active";

        let client = redis::Client::open(redis_url).expect("invalid redis URL");
        let mut conn = client.get_connection().expect("failed to connect to redis");

        let keyset_json: String = loop {
            match conn.get::<_, String>(redis_key) {
                Ok(s) => break s,
                Err(e) => {
                    warn!("Waiting for keyset in redis: {}", e);
                    thread::sleep(Duration::from_secs(1));
                }
            }
        };

        let keyset = Self::decode_keyset_json(&keyset_json);

        let sv2_keyset: Sv2KeySet = keyset.clone().try_into()
            .expect("Failed to convert KeySet into Sv2KeySet");

        info!("Loaded keyset {} from Redis", keyset.id);

        let pool = Pool::start(
            config.clone(),
            r_new_t,
            r_prev_hash,
            s_solution,
            s_message_recv_signal,
            status::Sender::DownstreamListener(status_tx),
            sv2_keyset,
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

    // SRI encodings are completely fucked just do it live
    pub fn decode_keyset_json(raw: &str) -> KeySet {
        let s = raw.trim().trim_start_matches('{').trim_end_matches('}');
    
        let mut id = None;
        let mut unit = None;
        let mut keys_json = None;
    
        for part in s.split(",\"") {
            let entry = part.trim_start_matches('"');
            if entry.starts_with("id") {
                let val = entry.trim_start_matches("id\":\"").trim_end_matches('"');
                let id_bytes = hex::decode(val).expect("invalid hex in id");
                let mut padded = [0u8; 8];
                padded[(8 - id_bytes.len())..].copy_from_slice(&id_bytes);
                let id_u64 = u64::from_be_bytes(padded);
                id = Some(KeysetId::try_from(id_u64).expect("invalid Id").0);
            } else if entry.starts_with("unit") {
                let val = entry.trim_start_matches("unit\":\"").trim_end_matches('"');
                unit = Some(CurrencyUnit::Custom(val.to_ascii_uppercase()));
            } else if entry.starts_with("keys\":{") {
                // fix: handle nested braces manually
                let keys_start = raw.find("\"keys\":{").expect("keys not found") + 7;
                let mut brace_count = 0;
                let mut end = keys_start;
                let chars: Vec<char> = raw.chars().collect();
                for i in keys_start..chars.len() {
                    match chars[i] {
                        '{' => brace_count += 1,
                        '}' => {
                            brace_count -= 1;
                            if brace_count == 0 {
                                end = i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                keys_json = Some(&raw[keys_start..=end]);
                break;
            }
        }
    
        let id = id.expect("missing id");
        let unit = unit.expect("missing unit");
        let keys_str = keys_json.expect("missing keys")
            .trim_start_matches('{')
            .trim_end_matches('}');
    
        let mut keys_map = BTreeMap::new();
        for entry in keys_str.split("\",\"") {
            let cleaned = entry.replace('\"', "");
            let mut parts = cleaned.splitn(2, ':');
            let amount = parts.next().expect("missing amount").parse::<u64>().expect("invalid amount");
            let pubkey_hex = parts.next().expect("missing pubkey");
            let pubkey_bytes = hex::decode(pubkey_hex).expect("bad pubkey hex");
            let pubkey = PublicKey::from_slice(&pubkey_bytes).expect("bad pubkey");
    
            keys_map.insert(AmountStr::from(Amount::from(amount)), pubkey);
        }
    
        KeySet {
            id,
            unit,
            keys: Keys::new(keys_map),
        }
    }    

}
