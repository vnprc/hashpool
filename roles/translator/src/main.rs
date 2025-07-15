#![allow(special_module_name)]
mod args;
mod lib;

use args::Args;
use error::{Error, ProxyResult};
pub use lib::{downstream_sv1, error, proxy, proxy_config, status, upstream_sv2};
use proxy_config::ProxyConfig;
use shared_config::MinerGlobalConfig;

use ext_config::{Config, File, FileFormat};

use tracing::error;

/// Process CLI args, if any.
#[allow(clippy::result_large_err)]
fn process_cli_args<'a>() -> ProxyResult<'a, (ProxyConfig, MinerGlobalConfig)> {
    // Parse CLI arguments
    let args = Args::from_args().map_err(|help| {
        error!("{}", help);
        Error::BadCliArgs
    })?;

    // Build configuration from the provided file path
    let config_path = args.config_path.to_str().ok_or_else(|| {
        error!("Invalid configuration path.");
        Error::BadCliArgs
    })?;

    let settings = Config::builder()
        .add_source(File::new(config_path, FileFormat::Toml))
        .build()?;

    // Deserialize settings into ProxyConfig
    let proxy_config = settings.try_deserialize::<ProxyConfig>()?;

    let global_path = args.global_config_path.to_str().ok_or_else(|| {
        error!("Invalid global configuration path.");
        Error::BadCliArgs
    })?;

    let global_config = MinerGlobalConfig::from_path(global_path)
        .map_err(|_| Error::BadCliArgs)?;

    Ok((proxy_config, global_config))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let (mut proxy_config, global_config) = match process_cli_args() {
        Ok(cfgs) => cfgs,
        Err(e) => panic!("failed to load config: {}", e),
    };

    // override config file with env var for improved devex configurability
    if let Ok(db_path_override) = std::env::var("CDK_WALLET_DB_PATH") {
        tracing::info!("Overriding wallet.dbPath with env var CDK_WALLET_DB_PATH={}", db_path_override);
        proxy_config.wallet.db_path = db_path_override;
    }

    proxy_config.mint = Some(global_config.mint);

    tracing::info!("Proxy Config: {:?}", &proxy_config);

    lib::TranslatorSv2::new(proxy_config).start().await;
}
