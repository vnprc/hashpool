#![allow(special_module_name)]
mod lib;

use anyhow::{Context, Result};
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
    /// Opts into binding `admin_listen` to a non-loopback address. Default
    /// false: a non-loopback bind is refused at startup otherwise.
    admin_allow_non_loopback: Option<bool>,
    bitcoin_rpc: Option<BitcoinRpcConfig>,
    /// Coinbase address the watcher matches (compared as a script, never as
    /// this string). Required: the mint refuses to start without it.
    receive_address: Option<String>,
    /// Confirmations before a provisional epoch boundary is final. Default 6.
    confirmation_depth: Option<u32>,
    /// Watcher RPC poll cadence, in seconds. Default 5.
    poll_interval_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BitcoinRpcConfig {
    url: String,
    user: String,
    pass: String,
}

use lib::epoch::{admin_router, EpochManager, EpochSettings};
use lib::{connect_to_pool_sv2, setup_mint};

/// Blocks `POST /v1/mint/quote/ehash` (404): bulk-pay would mint any quote
/// it created for free at finality, and this mint's only real ehash
/// quote-creation path is the SV2 message from the pool, never HTTP.
async fn block_ehash_quote_creation(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if is_http_quote_creation(req.method(), req.uri().path()) {
        return axum::response::Response::builder()
            .status(axum::http::StatusCode::NOT_FOUND)
            .body(axum::body::Body::empty())
            .expect("static response is well-formed");
    }
    next.run(req).await
}

/// The raw path cannot be compared directly: axum decodes `{method}` and
/// cdk lowercases it. The match must be exactly `v1/mint/quote/ehash` —
/// not just the `v1/mint/quote` prefix — because `/v1/mint/quote/pubkey`
/// (the NUT-XX pubkey lookup) and `/v1/mint/quote/<method>/check` (the
/// NUT-29 batch status check) share that prefix and must keep working.
fn is_http_quote_creation(method: &hyper::Method, raw_path: &str) -> bool {
    if method != hyper::Method::POST {
        return false;
    }
    let decoded = percent_encoding::percent_decode_str(raw_path).decode_utf8_lossy();
    let segments: Vec<String> = decoded
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    segments.len() == 4
        && segments[0] == "v1"
        && segments[1] == "mint"
        && segments[2] == "quote"
        && segments[3] == "ehash"
}

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
    let receive_address = hashpool_cfg.receive_address.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "[hashpool_mint] receive_address is required (coinbase script the watcher matches)"
        )
    })?;
    let receive_script = receive_address
        .parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()
        .map_err(|e| anyhow::anyhow!("invalid [hashpool_mint] receive_address: {e}"))?
        .assume_checked()
        .script_pubkey();
    info!(
        script = %hex::encode(receive_script.as_bytes()),
        "mint receive script"
    );

    let confirmation_depth = hashpool_cfg.confirmation_depth.unwrap_or(6);
    anyhow::ensure!(
        confirmation_depth >= 1,
        "[hashpool_mint] confirmation_depth must be >= 1"
    );
    let poll_interval_secs = hashpool_cfg.poll_interval_secs.unwrap_or(5);
    anyhow::ensure!(
        poll_interval_secs >= 1,
        "[hashpool_mint] poll_interval_secs must be >= 1"
    );

    let mint_db_path = lib::resolve_and_prepare_db_path(&db_path);
    let epoch_settings = EpochSettings {
        pool_pubkey: {
            let pool_pubkey = hashpool_cfg.pool_pubkey.clone().ok_or_else(|| {
                anyhow::anyhow!("[hashpool_mint] pool_pubkey is required (namespaces epoch units)")
            })?;
            lib::epoch::naming::validate_pool_pubkey(&pool_pubkey)
                .with_context(|| format!("loading {mint_config_path}"))?
        },
        store_path: hashpool_cfg
            .epoch_store_path
            .clone()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                mint_db_path
                    .parent()
                    .expect("db path has a parent")
                    .join("epochs.json")
            }),
        rpc_url: rpc_cfg.url,
        rpc_user: rpc_cfg.user,
        rpc_pass: rpc_cfg.pass,
        admin_listen: {
            let admin_listen = hashpool_cfg
                .admin_listen
                .clone()
                .unwrap_or_else(|| "127.0.0.1:3339".to_string());
            let admin_allow_non_loopback = hashpool_cfg.admin_allow_non_loopback.unwrap_or(false);
            let admin_listen_addr = lib::epoch::admin::validate_admin_listen(
                &admin_listen,
                admin_allow_non_loopback,
            )?;
            if !admin_listen_addr.ip().is_loopback() {
                tracing::warn!(
                    addr = %admin_listen_addr,
                    "[hashpool_mint] admin_allow_non_loopback is set: the unauthenticated epoch rotation lever is reachable from the network"
                );
            }
            admin_listen
        },
        receive_script,
        confirmation_depth,
        poll_interval: std::time::Duration::from_secs(poll_interval_secs),
        mint_db_path,
    };
    let admin_listen = epoch_settings.admin_listen.clone();
    let epochs = EpochManager::load_or_genesis(mint.clone(), epoch_settings).await?;
    // Resume (register-then-retire, above) completes before the watcher
    // starts and before the listeners bind below.
    epochs.spawn_watcher();

    // Manual rotation lever on a loopback-only listener (admin_listen is
    // validated above). No authentication: the lever is reachable only from
    // processes on the same host, and any such process can already read the
    // mint config, which holds the mint mnemonic — a credential here would
    // gate nothing the host does not already have.
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
    let router = cdk_axum::create_mint_router_with_custom_cache(mint.clone(), cache, vec!["ehash".to_string()], true).await?
        .layer(axum::middleware::from_fn(block_ehash_quote_creation));

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// The dev config's `receive_address` is a placeholder no wallet
    /// controls: the P2WPKH regtest address of the secp256k1 generator
    /// point. Pins the derivation against the committed config so the two
    /// cannot silently drift apart.
    #[test]
    fn dev_config_receive_address_is_the_generator_point_p2wpkh_address() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/mint.config.toml");
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let config: MintConfig = toml::from_str(&contents).unwrap();
        let configured = config
            .hashpool_mint
            .expect("[hashpool_mint] section must be present")
            .receive_address
            .expect("receive_address must be present");

        let generator_point =
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
        let pubkey = bitcoin::secp256k1::PublicKey::from_str(generator_point).unwrap();
        let compressed = bitcoin::CompressedPublicKey(pubkey);
        let expected = bitcoin::Address::p2wpkh(&compressed, bitcoin::Network::Regtest).to_string();

        assert_eq!(configured, expected);
    }

    fn blocked(raw_path: &str) {
        assert!(
            is_http_quote_creation(&hyper::Method::POST, raw_path),
            "{raw_path} must be blocked"
        );
    }

    fn allowed(method: &hyper::Method, raw_path: &str) {
        assert!(
            !is_http_quote_creation(method, raw_path),
            "{method} {raw_path} must be allowed through"
        );
    }

    #[test]
    fn blocks_post_ehash_quote_creation_regardless_of_encoding() {
        blocked("/v1/mint/quote/ehash");
        blocked("/v1/mint/quote/%65hash");
        blocked("/v1/mint/quote/EHASH");
        blocked("/v1//mint/quote/ehash/");
    }

    #[test]
    fn allows_everything_that_is_not_ehash_quote_creation() {
        // The NUT-XX pubkey lookup and the NUT-29 batch check share the
        // `/v1/mint/quote` prefix and must keep working.
        allowed(&hyper::Method::POST, "/v1/mint/quote/pubkey");
        allowed(&hyper::Method::POST, "/v1/mint/quote/ehash/check");
        allowed(&hyper::Method::GET, "/v1/mint/quote/ehash/abc");
        allowed(&hyper::Method::POST, "/v1/mint/ehash");
        allowed(&hyper::Method::POST, "/v1/mint/ehash/batch");
        allowed(&hyper::Method::POST, "/v1/swap");
    }
}
