use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub tcp_address: String,
    pub http_address: String,
    pub db_path: PathBuf,
}

impl Config {
    pub fn from_args() -> Result<Self, Box<dyn std::error::Error>> {
        let args: Vec<String> = env::args().collect();

        // Parse command line arguments - fail fast if not provided
        let tcp_address = args
            .iter()
            .position(|arg| arg == "--tcp-address" || arg == "-t")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --tcp-address")?;

        let http_address = args
            .iter()
            .position(|arg| arg == "--http-address" || arg == "-h")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --http-address")?;

        let db_path = args
            .iter()
            .position(|arg| arg == "--db-path" || arg == "-d")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --db-path")?;

        Ok(Config {
            tcp_address,
            http_address,
            db_path: PathBuf::from(db_path),
        })
    }
}
