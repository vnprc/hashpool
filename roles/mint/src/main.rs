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
use redis::AsyncCommands;
use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber::EnvFilter;
use bip39::Mnemonic;
use anyhow::{Result, bail};
use bitcoin::bip32::{ChildNumber, DerivationPath};
use shared_config::GlobalConfig;

use toml;
use std::fs;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("debug,sqlx=warn,hyper=warn,h2=warn"))
        .init();

    let mut args = std::env::args().skip(1); // Skip binary name

    let mint_config_path = match (args.next().as_deref(), args.next()) {
        (Some("-c"), Some(path)) => path,
        _ => {
            eprintln!("Usage: -c <mint_config_path> -g <global_config_path>");
            std::process::exit(1);
        }
    };

    let global_config_path = match (args.next().as_deref(), args.next()) {
        (Some("-g"), Some(path)) => path,
        _ => {
            eprintln!("Usage: -c <mint_config_path> -g <global_config_path>");
            std::process::exit(1);
        }
    };

    let mint_settings = config::Settings::new(Some(mint_config_path)).from_env()?;
    let global_config: GlobalConfig = toml::from_str(&fs::read_to_string(global_config_path)?)?;

    if mint_settings.ln.ln_backend == LnBackend::None {
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

    // TODO add to config
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

    let cache: HttpCache = mint_settings.info.http_cache.into();

    // let mint = Arc::new(mint_builder.add_cache(
    //     Some(cache.ttl.as_secs()),
    //     vec![],
    // ).build().await?);

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

    tokio::spawn(wait_for_invoices(mint.clone(), shutdown.clone()));

    let redis_url = global_config.redis.url.clone();
    let active_keyset_prefix = global_config.redis.active_keyset_prefix.clone();
    let create_quote_prefix = global_config.redis.create_quote_prefix.clone();
    let quote_id_prefix = global_config.redis.quote_id_prefix.clone();
    
    use redis::AsyncCommands;
    use serde_json;

    let keysets = mint.keysets().await.unwrap();
    let keyset_id = keysets.keysets.first().unwrap().id;
    let keyset = mint.keyset(&keyset_id).await.unwrap().unwrap();

    // Serialize full keyset
    let keyset_json = serde_json::to_string(&keyset).expect("Failed to serialize keyset");

    let redis_client = redis::Client::open(redis_url.clone())?;
    let mut redis_conn = redis_client.get_async_connection().await?;

    let redis_key = &active_keyset_prefix;

    // Cache and broadcast
    redis_conn.set(redis_key, &keyset_json).await?;

    tracing::info!(
        "Published keyset {} to Redis key '{}",
        keyset_id,
        redis_key,
    );

    tokio::spawn(poll_for_quotes(
        mint.clone(),
        redis_url.clone(),
        create_quote_prefix.clone(),
        quote_id_prefix.clone(),
    ));

    info!("Mint listening on {}:{}", mint_settings.info.listen_host, mint_settings.info.listen_port);
    axum::Server::bind(&format!("{}:{}", mint_settings.info.listen_host, mint_settings.info.listen_port).parse()?)
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

async fn wait_for_invoices(mint: Arc<Mint>, shutdown: Arc<Notify>) {
    if let Err(e) = mint.wait_for_paid_invoices(shutdown).await {
        tracing::error!("Error while waiting for paid invoices: {:?}", e);
    }
}

async fn handle_quote_payload(
    mint: Arc<Mint>,
    redis_url: &str,
    quote_id_prefix: &str,
    payload: &str,
) {
    let envelope: QuoteRequestEnvelope = match serde_json::from_str(payload) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Failed to parse quote request: {}", e);
            return;
        }
    };

    match mint.create_paid_mint_mining_share_quote(envelope.quote_request, envelope.blinded_messages).await {
        Ok(resp) => {
            tracing::info!("Quote created: {:?}", resp);
            let quote_mapping_key = format!("{}:{}", quote_id_prefix, resp.request);
            match redis::Client::open(redis_url) {
                Ok(client) => match client.get_async_connection().await {
                    Ok(mut redis_conn) => {
                        if let Err(e) = redis_conn.set::<_, _, ()>(&quote_mapping_key, resp.quote.to_string()).await {
                            tracing::error!("Failed to write quote to redis: {:?}", e);
                        }
                    }
                    Err(e) => tracing::error!("Failed to get redis connection: {:?}", e),
                },
                Err(e) => tracing::error!("Redis client open failed: {:?}", e),
            }
        }
        Err(err) => tracing::error!("Quote creation failed: {}", err),
    }
}

async fn poll_for_quotes(
    mint: Arc<Mint>,
    redis_url: String,
    create_quote_key: String,
    quote_id_prefix: String,
) {
    loop {
        let client_result = redis::Client::open(redis_url.clone());
        let mut conn = match client_result {
            Ok(client) => match client.get_async_connection().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!("Failed to get redis connection: {:?}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!("Redis client open failed: {:?}", e);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        let res: redis::RedisResult<Option<(String, String)>> = redis::cmd("BRPOP")
            .arg(&create_quote_key)
            .arg("0")
            .query_async(&mut conn)
            .await;

        if let Ok(Some((_, payload))) = res {
            handle_quote_payload(mint.clone(), &redis_url, &quote_id_prefix, &payload).await;
        }
    }
}
