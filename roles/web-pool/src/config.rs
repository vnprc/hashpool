use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub stats_pool_url: String,
    pub web_server_address: String,
}

impl Config {
    pub fn from_args() -> Result<Self, Box<dyn std::error::Error>> {
        let args: Vec<String> = env::args().collect();

        // Parse command line arguments
        let stats_pool_url = args
            .iter()
            .position(|arg| arg == "--stats-pool-url" || arg == "-s")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --stats-pool-url")?;

        let web_server_address = args
            .iter()
            .position(|arg| arg == "--web-address" || arg == "-w")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or("Missing required argument: --web-address")?;

        Ok(Config {
            stats_pool_url,
            web_server_address,
        })
    }
}
