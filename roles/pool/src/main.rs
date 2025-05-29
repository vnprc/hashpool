#![allow(special_module_name)]

mod lib;
use ext_config::{Config, File, FileFormat};
pub use lib::{mining_pool::Configuration, status, PoolSv2};
use tracing::error;
use shared_config::GlobalConfig;

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
        
            let config_path = config_path.unwrap_or_else(|| PathBuf::from(Self::DEFAULT_CONFIG_PATH));
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
    let global_path = args.global_config_path.to_str().expect("Invalid global config path");

    // Load local config
    let config: Configuration = match Config::builder()
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

    let global_config = match GlobalConfig::from_path(global_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load global config: {}", e);
            return;
        }
    };

    let mut pool = PoolSv2::new(config);
    pool.set_global_config(global_config);
    let _ = pool.start().await;
}
