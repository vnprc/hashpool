#![allow(special_module_name)]
mod lib;

use anyhow::Result;
use cdk_axum::cache::HttpCache;
use cdk_mintd::config;
use serde::{Deserialize, Serialize};
use shared_config::PoolGlobalConfig;
use std::fs;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

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
    /// Pool identity: compressed secp256k1 pubkey (hex) namespacing epoch
    /// units (`hash_<pool>_<height>`). Required.
    pool_pubkey: Option<String>,
    /// Epoch record store; defaults to `epochs.json` beside the mint database.
    epoch_store_path: Option<String>,
    /// Loopback listener for the manual rotation lever. Default 127.0.0.1:3339.
    admin_listen: Option<String>,
    bitcoin_rpc: Option<BitcoinRpcConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BitcoinRpcConfig {
    url: String,
    user: String,
    pass: String,
}

use lib::epoch::{admin_router, EpochManager, EpochSettings};
use lib::{connect_to_pool_sv2, setup_mint};

#[tokio::main]
async fn main() -> Result<()> {
    // Respect RUST_LOG env var, defaulting to info level with dependency filtering
    // Note: CDK mint uses CDK's tracing configuration. To log to file, redirect stdout/stderr
    // in the systemd service or use RUST_LOG with external log capture.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn,h2=warn"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Simple argument parser: extract values by flag
    fn get_arg(args: &[String], flag: &str) -> Option<String> {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map(|w| w[1].clone())
    }

    let mint_config_path = get_arg(&args, "-c")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: -c <mint_config_path>"))?;
    let global_config_path = get_arg(&args, "-g")
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: -g <global_config_path>"))?;
    let _log_file = get_arg(&args, "-f").or_else(|| get_arg(&args, "--log-file"));

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
    let mint = setup_mint(mint_config.cdk_settings.clone(), db_path.clone()).await?;

    // Epoch mechanics: load the persisted current epoch or open genesis at the
    // current chain height. Fails loud if the pool identity or bitcoind RPC
    // config is missing. See docs/EPOCH_DESIGN.md.
    let hashpool_cfg = mint_config.hashpool_mint.clone().ok_or_else(|| {
        anyhow::anyhow!("[hashpool_mint] config section is required for epoch mechanics")
    })?;
    let rpc_cfg = hashpool_cfg.bitcoin_rpc.clone().ok_or_else(|| {
        anyhow::anyhow!("[hashpool_mint.bitcoin_rpc] url/user/pass are required (epoch genesis and rotation read the chain height)")
    })?;
    let epoch_settings = EpochSettings {
        pool_pubkey: hashpool_cfg.pool_pubkey.clone().ok_or_else(|| {
            anyhow::anyhow!("[hashpool_mint] pool_pubkey is required (namespaces epoch units)")
        })?,
        store_path: hashpool_cfg
            .epoch_store_path
            .clone()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                lib::resolve_and_prepare_db_path(&db_path)
                    .parent()
                    .expect("db path has a parent")
                    .join("epochs.json")
            }),
        rpc_url: rpc_cfg.url,
        rpc_user: rpc_cfg.user,
        rpc_pass: rpc_cfg.pass,
        admin_listen: hashpool_cfg
            .admin_listen
            .clone()
            .unwrap_or_else(|| "127.0.0.1:3339".to_string()),
    };
    let admin_listen = epoch_settings.admin_listen.clone();
    let epochs = EpochManager::load_or_genesis(mint.clone(), epoch_settings).await?;

    // Manual rotation lever on a loopback-only listener.
    let admin = admin_router(epochs.clone());
    let admin_listener = TcpListener::bind(&admin_listen).await?;
    info!("Epoch admin listening on {}", admin_listen);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(admin_listener, admin).await {
            tracing::error!("epoch admin server exited: {e}");
        }
    });

    // Setup HTTP cache and router
    let cache: HttpCache = HttpCache::from_config(mint_config.cdk_settings.info.http_cache).await?;
    let router = cdk_axum::create_mint_router_with_custom_cache(mint.clone(), cache, vec!["ehash".to_string()], true).await?;

    // Start SV2 connection to pool if enabled
    if let Some(ref sv2_config) = global_config.sv2_messaging {
        if sv2_config.enabled {
            tokio::spawn(connect_to_pool_sv2(
                mint.clone(),
                epochs.clone(),
                sv2_config.clone(),
            ));
        }
    }

    // Start HTTP server
    let addr = format!(
        "{}:{}",
        mint_config.cdk_settings.info.listen_host, mint_config.cdk_settings.info.listen_port
    );
    info!("Mint listening on {}", addr);
    let listener = TcpListener::bind(&addr).await?;

    axum::serve(listener, router).await?;

    Ok(())
}
