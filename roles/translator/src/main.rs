#![allow(special_module_name)]
mod args;
mod lib;

use args::Args;
use error::{Error, ProxyResult};
pub use lib::{downstream_sv1, error, proxy, proxy_config, status, upstream_sv2};
use proxy_config::ProxyConfig;
use shared_config::PoolGlobalConfig;

use ext_config::{Config, File, FileFormat};

use tracing::{error, info};

/// Process CLI args, if any.
#[allow(clippy::result_large_err)]
fn process_cli_args<'a>() -> ProxyResult<'a, (ProxyConfig, PoolGlobalConfig)> {
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

    let global_config = PoolGlobalConfig::from_path(global_path)
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

    proxy_config.mint = Some(global_config.mint);
    
    // TODO get keyset from HTTP api and delete the pool config
    proxy_config.redis = Some(global_config.redis);

    info!("Proxy Config: {:?}", &proxy_config);

    lib::TranslatorSv2::new(proxy_config).start().await;
}
