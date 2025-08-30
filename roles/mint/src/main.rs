mod lib;

use anyhow::Result;
use cdk_axum::cache::HttpCache;
use cdk_mintd::config;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;
use shared_config::PoolGlobalConfig;
use std::{fs, sync::Arc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

/// Extended config for hashpool-specific mint settings
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MintConfig {
    #[serde(flatten)]
    cdk_settings: config::Settings,
    hashpool_mint: Option<HashpoolMintConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HashpoolMintConfig {
    db_path: Option<String>,
}

use lib::{connect_to_pool_sv2, setup_mint};

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

    // Parse mint config 
    let mint_config_str = fs::read_to_string(&mint_config_path)?;
    let mint_config: MintConfig = toml::from_str(&mint_config_str)?;
    
    let global_config: PoolGlobalConfig = toml::from_str(&fs::read_to_string(global_config_path)?)?;

    // Setup mint with all required components - determine database path
    // Priority: env var > config file (no hardcoded fallback)
    let db_path = std::env::var("CDK_MINT_DB_PATH")
        .ok()
        .or_else(|| {
            mint_config.hashpool_mint
                .as_ref()
                .and_then(|hm| hm.db_path.as_ref())
                .map(|p| p.clone())
        })
        .ok_or_else(|| anyhow::anyhow!(
            "Database path must be specified either via CDK_MINT_DB_PATH environment variable or [hashpool_mint] db_path config"
        ))?;
    
    tracing::info!("Using database path: {}", db_path);
    let mint = setup_mint(mint_config.cdk_settings.clone(), db_path).await?;

    // Setup HTTP cache and router
    let cache: HttpCache = mint_config.cdk_settings.info.http_cache.into();
    let router = cdk_axum::create_mint_router_with_custom_cache(mint.clone(), cache, false).await?;

    // Publish keyset to Redis for pool coordination
    publish_keyset_to_redis(&mint, &global_config).await?;

    // Start SV2 connection to pool if enabled
    if let Some(ref sv2_config) = global_config.sv2_messaging {
        if sv2_config.enabled {
            tokio::spawn(connect_to_pool_sv2(
                mint.clone(),
                sv2_config.clone(),
            ));
        }
    }

    // Start HTTP server
    let addr = format!("{}:{}", mint_config.cdk_settings.info.listen_host, mint_config.cdk_settings.info.listen_port);
    info!("Mint listening on {}", addr);
    let listener = TcpListener::bind(&addr).await?;

    axum::serve(listener, router).await?;

    Ok(())
}

/// Publish active keyset to Redis for pool coordination
async fn publish_keyset_to_redis(
    mint: &Arc<cdk::mint::Mint>,
    global_config: &PoolGlobalConfig,
) -> Result<()> {
    let redis_url = global_config.redis.url.clone();
    let active_keyset_prefix = global_config.redis.active_keyset_prefix.clone();
    
    let keysets = mint.keysets();
    let keyset_id = keysets.keysets.first().unwrap().id;
    let keyset = mint.keyset(&keyset_id).unwrap();

    // Create Redis connection
    let client = redis::Client::open(redis_url)?;
    let mut redis_conn = client.get_multiplexed_tokio_connection().await?;

    // Serialize keyset for Redis
    let keyset_json = serde_json::to_string(&keyset)?;
    let redis_key = active_keyset_prefix;

    // Publish to Redis
    redis_conn.set(redis_key.clone(), &keyset_json).await?;

    info!(
        "Published keyset {} to Redis key '{}'",
        keyset_id,
        redis_key,
    );

    Ok(())
}