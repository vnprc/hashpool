#![allow(special_module_name)]

mod lib;
use ext_config::{Config, File, FileFormat};
pub use lib::{mining_pool::Configuration, status, PoolSv2};
use shared_config::PoolGlobalConfig;
use tracing::error;

mod args {
    use std::path::PathBuf;

    #[derive(Debug)]
    pub struct Args {
        pub config_path: PathBuf,
        pub global_config_path: PathBuf,
    }

    impl Args {
        const DEFAULT_CONFIG_PATH: &'static str = "pool-config.toml";
        const HELP_MSG: &'static str =
            "Usage: -h/--help, -c/--config <path|default pool-config.toml>, -g/--global <path>";

        pub fn from_args() -> Result<Self, String> {
            let args: Vec<String> = std::env::args().collect();

            if args.len() == 1 {
                println!("Using default config path: {}", Self::DEFAULT_CONFIG_PATH);
                println!("{}\n", Self::HELP_MSG);
            }

            let mut config_path = None;
            let mut global_config_path = None;
            let mut iter = args.into_iter().skip(1);

            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "-c" | "--config" => {
                        config_path = iter.next().map(PathBuf::from);
                    }
                    "-g" | "--global" => {
                        global_config_path = iter.next().map(PathBuf::from);
                    }
                    "-h" | "--help" => return Err(Self::HELP_MSG.to_string()),
                    _ => {}
                }
            }

            let config_path =
                config_path.unwrap_or_else(|| PathBuf::from(Self::DEFAULT_CONFIG_PATH));
            let global_config_path = global_config_path.ok_or("Missing -g/--global <path>")?;

            Ok(Self {
                config_path,
                global_config_path,
            })
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = match args::Args::from_args() {
        Ok(cfg) => cfg,
        Err(help) => {
            error!("{}", help);
            return;
        }
    };

    let config_path = args.config_path.to_str().expect("Invalid config path");
    let global_path = args
        .global_config_path
        .to_str()
        .expect("Invalid global config path");

    // Load local config
    let mut config: Configuration = match Config::builder()
        .add_source(File::new(config_path, FileFormat::Toml))
        .build()
    {
        Ok(settings) => match settings.try_deserialize::<Configuration>() {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to deserialize config: {}", e);
                return;
            }
        },
        Err(e) => {
            error!("Failed to build config: {}", e);
            return;
        }
    };

    let global_config = match PoolGlobalConfig::from_path(global_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load global config: {}", e);
            return;
        }
    };

    // Load snapshot polling interval from shared config
    // Try to read the shared config file to get the stats.snapshot_poll_interval_secs
    if let Ok(shared_config_str) = std::fs::read_to_string(global_path) {
        if let Ok(shared_config) = toml::from_str::<toml::Value>(&shared_config_str) {
            if let Some(interval) = shared_config
                .get("stats")
                .and_then(|s| s.get("snapshot_poll_interval_secs"))
                .and_then(|v| v.as_integer())
            {
                config.snapshot_poll_interval_secs = interval as u64;
            }
        }
    }

    let mut pool = PoolSv2::new(config, global_config.sv2_messaging, global_config.ehash);
    let _ = pool.start().await;
}
