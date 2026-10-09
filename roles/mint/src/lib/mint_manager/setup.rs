use std::{path::PathBuf, str::FromStr, sync::Arc};

use anyhow::Result;
use bip39::Mnemonic;
use cdk::{
    mint::{Mint, MintBuilder},
    nuts::Nut29Settings,
    types::QuoteTTL,
};
use cdk_mintd::config;
use cdk_sqlite::MintSqliteDatabase;

/// Setup and initialize the mint.
///
/// No currency unit is configured here: every unit is a mining epoch, opened at
/// runtime by the `EpochManager` — genesis on first boot, then one per block
/// reward. See docs/EPOCH_DESIGN.md.
pub async fn setup_mint(mint_settings: config::Settings, db_path: String) -> Result<Arc<Mint>> {
    let mnemonic = Mnemonic::from_str(&mint_settings.info.mnemonic.unwrap())
        .map_err(|e| anyhow::anyhow!("Invalid mnemonic in mint config: {}", e))?;
    let seed = mnemonic.to_seed("");

    // Database setup
    let mint_db_path = resolve_and_prepare_db_path(&db_path);

    let db = Arc::new(MintSqliteDatabase::new(mint_db_path).await?);

    let builder = MintBuilder::new(db.clone())
        .with_name(mint_settings.mint_info.name.clone())
        .with_description(mint_settings.mint_info.description.clone())
        .with_urls(vec![mint_settings.info.url.clone()])
        .with_batch_minting(None, Some(vec!["ehash".to_string()]));

    let mint = Arc::new(
        builder
            .build_with_seed(db, &seed)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to build mint: {}", e))?,
    );

    // Keep NUT-29 method settings fresh across upgrades. NUT-04/05 entries are
    // runtime state owned by epoch registration — never overwrite them here, or
    // a restart would strand the resumed epoch's quote creation.
    {
        let mut stored_info = mint.mint_info().await?;
        stored_info.nuts.nut29 = Nut29Settings::new(None, Some(vec!["ehash".to_string()]));
        mint.set_mint_info(stored_info).await?;
    }

    // Quotes must comfortably outlive the epoch-boundary confirmation window
    // (docs/EPOCH_DESIGN.md): 7 days, generous against any sane depth D.
    mint.set_quote_ttl(QuoteTTL::new(604_800, 604_800)).await?;

    // Start background tasks for invoice monitoring
    mint.start().await?;

    Ok(mint)
}

/// Resolve and prepare database path
pub fn resolve_and_prepare_db_path(config_path: &str) -> PathBuf {
    use std::{env, path::Path};

    let path = Path::new(config_path);
    let full_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .expect("Failed to get current working directory")
            .join(path)
    };

    // Create parent directories if they don't exist
    if let Some(parent) = full_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).expect("Failed to create database directory");
        }
    }

    full_path
}
