use std::{
    backtrace::Backtrace,
    fs::OpenOptions,
    io::{self, IsTerminal},
    panic,
    path::Path,
    str::FromStr,
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{fmt, prelude::*, EnvFilter, Registry};

/// Initialize logging to stdout and optionally to a file.
///
/// If `log_file` is Some, logs will be written to both stdout and the file.
/// If `log_level` is not provided or is invalid, it defaults to "info".
pub fn init_logging(log_file: Option<&Path>) {
    let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let log_level_filter = LevelFilter::from_str(&rust_log).unwrap_or(LevelFilter::INFO);
    let env_filter = EnvFilter::new(log_level_filter.to_string());
    let stdout_layer = fmt::layer()
        .with_writer(io::stdout)
        .with_ansi(io::stdout().is_terminal());

    let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = match log_file {
        Some(path) => {
            // Log to both file and stdout
            let path = path.to_owned();
            // Open file only once, and not on every write.
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("Failed to open log file");
            let file_layer = fmt::layer().with_writer(file).with_ansi(false);
            Box::new(
                Registry::default()
                    .with(env_filter)
                    .with(stdout_layer)
                    .with(file_layer),
            )
        }
        None => {
            // Log only to stdout
            Box::new(Registry::default().with(env_filter).with(stdout_layer))
        }
    };

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set global subscriber");

    // Set up a panic hook that records panic information and a backtrace
    // as tracing events, ensuring they are persisted in the log file.
    let default_panic_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let backtrace = Backtrace::force_capture();
        tracing::error!("panic: {panic_info}");
        tracing::error!("Backtrace: {backtrace}");
        default_panic_hook(panic_info);
    }));
}
