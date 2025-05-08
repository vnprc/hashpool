use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::str::FromStr;

use axum::Router;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk::cdk_database::{self, MintDatabase};
use cdk::mint::{Mint, MintBuilder, MintMeltLimits};
use cdk::nuts::nut17::SupportedMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::types::QuoteTTL;
use cdk_axum::cache::HttpCache;
use cdk_mintd::config::{self, LnBackend, DatabaseEngine};
use cdk_mintd::setup::LnBackendSetup;
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber::EnvFilter;
use bip39::Mnemonic;
use anyhow::{Result, bail};
use bitcoin::bip32::{ChildNumber, DerivationPath};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("debug,sqlx=warn,hyper=warn,h2=warn"))
        .init();

    // TODO upgrade to clap if/when we add another arg
    let mut args = std::env::args().skip(1); // Skip binary name

    let config_path = match (args.next().as_deref(), args.next()) {
        (Some("-c"), Some(path)) => path,
        _ => {
            eprintln!("Error: Config file path is required.\nUsage: cargo -C roles/mint -Z unstable-options run -- -c <config_path>");
            std::process::exit(1);
        }
    };
        
    let settings = config::Settings::new(Some(config_path)).from_env()?;

    if settings.ln.ln_backend == LnBackend::None {
        bail!("Ln backend must be set");
    }

    let work_dir: PathBuf = home::home_dir()
        .unwrap()
        .join(".cdk-mintd");

    std::fs::create_dir_all(&work_dir)?;

    // TODO go back to the builder pattern and config file
    // figure out how to specify num keys

    // let db: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync> = match settings.database.engine {
    //     DatabaseEngine::Sqlite => {
    //         let path = work_dir.join("cdk-mintd.sqlite");
    //         let sqlite = MintSqliteDatabase::new(&path).await?;
    //         sqlite.migrate().await;
    //         Arc::new(sqlite)
    //     }
    //     DatabaseEngine::Redb => {
    //         let path = work_dir.join("cdk-mintd.redb");
    //         Arc::new(MintRedbDatabase::new(&path)?)
    //     }
    // };

    // let mint_info = settings.mint_info.clone();
    // let info = settings.info.clone();
    // let mut mint_builder = MintBuilder::new()
    //     .with_localstore(db)
    //     .with_name(mint_info.name)
    //     .with_description(mint_info.description)
    //     .with_seed(Mnemonic::from_str(&info.mnemonic)?.to_seed_normalized("").to_vec());

    // let melt_limits = MintMeltLimits {
    //     mint_min: settings.ln.min_mint,
    //     mint_max: settings.ln.max_mint,
    //     melt_min: settings.ln.min_melt,
    //     melt_max: settings.ln.max_melt,
    // };

    // if settings.ln.ln_backend == LnBackend::FakeWallet {
    //     let fake_cfg = settings.clone().fake_wallet.expect("FakeWallet config required");
    //     for unit in &fake_cfg.supported_units {
    //         let ln = fake_cfg.setup(&mut vec![], &settings, unit.clone()).await?;
    //         mint_builder = mint_builder
    //             .add_ln_backend(unit.clone(), PaymentMethod::Bolt11, melt_limits, Arc::new(ln))
    //             .add_supported_websockets(SupportedMethods::new(PaymentMethod::Bolt11, unit.clone()));
    //     }
    // } else {
    //     bail!("Only fakewallet backend supported in this minimal launcher");
    // }

    pub const HASH_CURRENCY_UNIT: &str = "HASH";
    pub const HASH_DERIVATION_PATH: u32 = 1337;
    const NUM_KEYS: u8 = 64;

    // TODO securely import mnemonic
    let mnemonic = Mnemonic::generate(12).unwrap();

    let hash_currency_unit = CurrencyUnit::Custom(HASH_CURRENCY_UNIT.to_string());

    let mut currency_units = HashMap::new();
    currency_units.insert(hash_currency_unit.clone(), (0, NUM_KEYS));

    let mut derivation_paths = HashMap::new();
    derivation_paths.insert(hash_currency_unit, DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("Failed to create purpose index 0"),
        ChildNumber::from_hardened_idx(HASH_DERIVATION_PATH).expect(&format!("Failed to create coin type index {}", HASH_DERIVATION_PATH)),
        ChildNumber::from_hardened_idx(0).expect("Failed to create account index 0"),
    ]));

    let cache: HttpCache = settings.info.http_cache.into();

    let mint = Arc::new(Mint::new(
        &mnemonic.to_seed_normalized(""),
        Arc::new(MintMemoryDatabase::default()),
        HashMap::new(),
        currency_units,
        derivation_paths,
    )
    .await.unwrap());

    mint.check_pending_mint_quotes().await?;
    mint.check_pending_melt_quotes().await?;
    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
    let router: Router = cdk_axum::create_mint_router_with_custom_cache(mint.clone(), cache).await?;
    let shutdown = Arc::new(Notify::new());

    tokio::spawn({
        let shutdown = shutdown.clone();
        let mint = mint.clone();
        async move {
            mint.wait_for_paid_invoices(shutdown).await;
        }
    });

    // publish keyset to redis
    use redis::AsyncCommands;
    use serde_json;

    let keysets = mint.keysets().await.unwrap();
    let keyset_id = keysets.keysets.first().unwrap().id;
    let keyset = mint.keyset(&keyset_id).await.unwrap().unwrap();

    // Serialize full keyset
    let keyset_json = serde_json::to_string(&keyset).expect("Failed to serialize keyset");

    let redis_client = redis::Client::open(REDIS_URL)?;
    let mut redis_conn = redis_client.get_async_connection().await?;

    let redis_key = format!("mint:keyset:active");

    // Cache and broadcast
    redis_conn.set(&redis_key, &keyset_json).await?;

    tracing::info!(
        "Published keyset {} to Redis key '{}",
        keyset_id,
        redis_key,
    );

    // TODO move this code to a more appropriate module
    const REDIS_KEY_CREATE_QUOTE: &str = "mint:quotes:create";
    const REDIS_URL: &str = "redis://localhost:6379";

    // quote polling loop
    tokio::spawn({
        let mint = mint.clone();
        async move {
            loop {
                match redis::Client::open(REDIS_URL) {
                    Ok(client) => match client.get_async_connection().await {
                        Ok(mut conn) => {
                            loop {
                                let res: redis::RedisResult<Option<(String, String)>> = redis::cmd("BRPOP")
                                    .arg(REDIS_KEY_CREATE_QUOTE)
                                    .arg("0")
                                    .query_async(&mut conn)
                                    .await;

                                match res {
                                    Ok(Some((_, payload))) => {
                                        let val: serde_json::Value = serde_json::from_str(&payload).unwrap();
                                        let quote_part = &val["quote_request"];
                                        if let Some(obj) = quote_part.as_object() {
                                            let keys: Vec<_> = obj.keys().collect();
                                            println!("quote_request fields: {:?}", keys);
                                        } else {
                                            println!("quote_request is not an object!");
                                        }

                                        match serde_json::from_str::<QuoteRequestEnvelope>(&payload) {
                                            Ok(envelope) => {
                                                match mint.create_paid_mint_mining_share_quote(envelope.quote_request, envelope.blinded_messages).await {
                                                    Ok(resp) => {
                                                        tracing::info!("Quote created: {:?}", resp);

                                                        let quote_id = resp.quote.to_string();
                                                        let header_hash = resp.request.to_string();

                                                        let quote_mapping_key = format!("mint:quotes:hash:{}", header_hash);
                                                        match redis::Client::open(REDIS_URL) {
                                                            Ok(client) => match client.get_async_connection().await {
                                                                Ok(mut redis_conn) => {
                                                                    if let Err(e) = redis_conn.set::<_, _, ()>(&quote_mapping_key, &quote_id).await {
                                                                        tracing::warn!("Failed to write quote-header to Redis: {:?}", e);
                                                                    }
                                                                }
                                                                Err(e) => tracing::warn!("Failed to get Redis connection: {:?}", e),
                                                            },
                                                            Err(e) => tracing::warn!("Failed to open Redis client: {:?}", e),
                                                        };
                                                    }
                                                    Err(err) => tracing::warn!("Failed to create quote: {}", err),
                                                }
                                            }
                                            Err(e) => {
                                                tracing::warn!("Failed to parse quote request from Redis: {}", e);
                                            }
                                        }
                                    }
                                    Ok(None) => continue,
                                    Err(e) => {
                                        tracing::warn!("Redis BRPOP error: {:?}", e);
                                        break; // Reconnect on failure
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to connect to Redis: {:?}", e);
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Redis client setup failed: {:?}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }
    });

    info!("Mint listening on {}:{}", settings.info.listen_host, settings.info.listen_port);
    axum::Server::bind(&format!("{}:{}", settings.info.listen_host, settings.info.listen_port).parse()?)
        .serve(router.into_make_service())
        .await?;

    Ok(())
}

// TODO move this somewhere more appropriate. Into cdk probably
use cdk::nuts::nut04::MintQuoteMiningShareRequest;
use cdk::nuts::BlindedMessage;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct QuoteRequestEnvelope {
    quote_request: MintQuoteMiningShareRequest,
    blinded_messages: Vec<BlindedMessage>,
}