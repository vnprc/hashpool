use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::str::FromStr;

use cdk::mint::Mint;
use cdk::nuts::CurrencyUnit;
use cdk::nuts::nutXX::MintQuoteMiningShareRequest;
use cdk::types::QuoteTTL;
use cdk_axum::cache::HttpCache;
use cdk_mintd::config::{self, LnBackend};
use cdk::cdk_payment::MintPayment;
use cdk::cdk_payment;
use cdk_signatory::db_signatory::DbSignatory;
use cdk::types::PaymentProcessorKey;
use cdk_sqlite::MintSqliteDatabase;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber::EnvFilter;
use bip39::Mnemonic;
use anyhow::{Result, bail};
use bitcoin::bip32::{ChildNumber, DerivationPath};
use shared_config::PoolGlobalConfig;

use toml;
use std::{env, fs};

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
    let global_config: PoolGlobalConfig = toml::from_str(&fs::read_to_string(global_config_path)?)?;

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

    let mnemonic = Mnemonic::from_str(&mint_settings.info.mnemonic.unwrap())
        .map_err(|e| anyhow::anyhow!("Invalid mnemonic in mint config: {}", e))?;
    let seed_bytes : &[u8] = &mnemonic.to_seed("");

    let hash_currency_unit = CurrencyUnit::Custom(HASH_CURRENCY_UNIT.to_string());

    let mut currency_units = HashMap::new();
    currency_units.insert(hash_currency_unit.clone(), (0, NUM_KEYS));

    let mut derivation_paths = HashMap::new();
    derivation_paths.insert(hash_currency_unit.clone(), DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("Failed to create purpose index 0"),
        ChildNumber::from_hardened_idx(HASH_DERIVATION_PATH).expect(&format!("Failed to create coin type index {}", HASH_DERIVATION_PATH)),
        ChildNumber::from_hardened_idx(0).expect("Failed to create account index 0"),
    ]));

    let cache: HttpCache = mint_settings.info.http_cache.into();

    // TODO update settings to accept mint db path, just use env var for now
    let mut mint_db_path = resolve_and_prepare_db_path(".devenv/state/mint/mint.sqlite");

    // override config file with env var for improved devex configurability
    if let Ok(db_path_override) = std::env::var("CDK_MINT_DB_PATH") {
        tracing::info!("Overriding mint.dbPath with env var CDK_MINT_DB_PATH={}", db_path_override);
        mint_db_path = resolve_and_prepare_db_path(&db_path_override);
    }

    let db = Arc::new(MintSqliteDatabase::new(mint_db_path).await?);

    let signatory = Arc::new(
        DbSignatory::new(
            db.clone(),
            seed_bytes,
            currency_units,
            derivation_paths,
        ).await.unwrap()
    );

    let ln: HashMap<PaymentProcessorKey, Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>> = HashMap::new();

    let mint = Arc::new(Mint::new(
        signatory,
        db,
        ln,
    )
    .await.unwrap());

    // mint.check_pending_mint_quotes().await?;
    // mint.check_pending_melt_quotes().await?;
    
    // Set mint info in database
    use cdk::nuts::{MintInfo, Nuts, PaymentMethod, MintMethodSettings};
    use cdk::Amount;
    
    // Configure NUT-04 settings for MiningShare payment method with HASH unit
    let mining_share_method = MintMethodSettings {
        method: PaymentMethod::MiningShare,
        unit: hash_currency_unit.clone(),
        min_amount: Some(Amount::from(1)),
        // TODO update units to 2^bits not just raw bits
        max_amount: Some(Amount::from(256)),
        options: None,
    };
    
    let mut nuts = Nuts::new();
    nuts.nut04.methods.push(mining_share_method);
    nuts.nut04.disabled = false;
    
    let mint_info = MintInfo {
        name: Some(mint_settings.mint_info.name.clone()),
        description: Some(mint_settings.mint_info.description.clone()),
        pubkey: None,
        version: None,
        description_long: None,
        contact: None,
        nuts,
        icon_url: None,
        urls: None,
        motd: None,
        time: None,
        tos_url: None,
    };
    mint.set_mint_info(mint_info).await?;
    
    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;

    let router = cdk_axum::create_mint_router_with_custom_cache(mint.clone(), cache, false).await?;
    let shutdown = Arc::new(Notify::new());

    tokio::spawn(wait_for_invoices(mint.clone(), shutdown.clone()));

    let redis_url = global_config.redis.url.clone();
    let active_keyset_prefix = global_config.redis.active_keyset_prefix.clone();
    let create_quote_prefix = global_config.redis.create_quote_prefix.clone();
    
    use redis::AsyncCommands;
    use serde_json;

    let keysets = mint.keysets();
    let keyset_id = keysets.keysets.first().unwrap().id;
    let keyset = mint.keyset(&keyset_id).unwrap();

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
    ));

    info!("Mint listening on {}:{}", mint_settings.info.listen_host, mint_settings.info.listen_port);
    let addr = format!("{}:{}", mint_settings.info.listen_host, mint_settings.info.listen_port);
    let listener = TcpListener::bind(&addr).await?;

    axum::serve(listener, router).await?;

    Ok(())
}

async fn wait_for_invoices(mint: Arc<Mint>, shutdown: Arc<Notify>) {
    if let Err(e) = mint.wait_for_paid_invoices(shutdown).await {
        tracing::error!("Error while waiting for paid invoices: {:?}", e);
    }
}

async fn handle_quote_payload(
    mint: Arc<Mint>,
    payload: &str,
) {
    let quote_request: MintQuoteMiningShareRequest = match serde_json::from_str(payload) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("Failed to parse quote request: {}", e);
            return;
        }
    };

    match mint.create_mint_mining_share_quote(quote_request).await {
        Ok(resp) => tracing::info!("Quote created: {:?}", resp),
        Err(err) => tracing::error!("Quote creation failed: {}", err),
    }
}

async fn poll_for_quotes(
    mint: Arc<Mint>,
    redis_url: String,
    create_quote_key: String,
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
            handle_quote_payload(mint.clone(), &payload).await;
        }
    }
}

fn resolve_and_prepare_db_path(config_path: &str) -> PathBuf {
    let path = Path::new(config_path);
    let full_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .expect("Failed to get current working directory")
            .join(path)
    };

    if let Some(parent) = full_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).expect("Failed to create parent directory for DB path");
        }
    }

    full_path
}