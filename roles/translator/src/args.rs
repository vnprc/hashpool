//! Defines the structure and parsing logic for command-line arguments.
//!
//! It provides the `Args` struct to hold parsed arguments,
//! and the `from_args` function to parse them from the command line.
use clap::Parser;
use ext_config::{Config, File, FileFormat};
use std::path::PathBuf;
use tracing::error;
use translator_sv2::{config::TranslatorConfig, error::TproxyErrorKind};

/// Holds the parsed CLI arguments.
#[derive(Parser, Debug)]
#[command(author, version, about = "Translator Proxy", long_about = None)]
pub struct Args {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the TOML configuration file",
        default_value = "translator-config.toml"
    )]
    pub config_path: PathBuf,
    #[arg(
        short = 'g',
        long = "global-config",
        help = "Path to a shared global TOML configuration file. Values are loaded first and may be overridden by the main config."
    )]
    pub global_config_path: Option<PathBuf>,
    #[arg(
        short = 'f',
        long = "log-file",
        help = "Path to the log file. If not set, logs will only be written to stdout."
    )]
    pub log_file: Option<PathBuf>,
}

/// Process CLI args, if any.
#[allow(clippy::result_large_err)]
pub fn process_cli_args() -> Result<TranslatorConfig, TproxyErrorKind> {
    // Parse CLI arguments
    let args = Args::parse();

    // Build configuration: global config provides base values, specific config overrides
    let mut builder = Config::builder();

    if let Some(global_path) = &args.global_config_path {
        let global_str = global_path.to_str().ok_or_else(|| {
            error!("Invalid global configuration path.");
            TproxyErrorKind::BadCliArgs
        })?;
        builder = builder.add_source(File::new(global_str, FileFormat::Toml));
    }

    let config_path = args.config_path.to_str().ok_or_else(|| {
        error!("Invalid configuration path.");
        TproxyErrorKind::BadCliArgs
    })?;

    let settings = builder
        .add_source(File::new(config_path, FileFormat::Toml))
        .build()?;

    // Deserialize settings into TranslatorConfig
    let mut config = settings.try_deserialize::<TranslatorConfig>()?;

    config.set_log_dir(args.log_file);

    Ok(config)
}
