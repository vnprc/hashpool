use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::str::FromStr;

use cdk::mint::Mint;
use cdk::nuts::{CurrencyUnit, MintInfo, Nuts, PaymentMethod, MintMethodSettings};
use cdk::types::QuoteTTL;
use cdk::Amount;
use cdk_mintd::config::{self, LnBackend};
use cdk::cdk_payment::MintPayment;
use cdk::cdk_payment;
use cdk_signatory::db_signatory::DbSignatory;
use cdk::types::PaymentProcessorKey;
use cdk_sqlite::MintSqliteDatabase;
use bip39::Mnemonic;
use anyhow::{Result, bail};

/// Setup and initialize the mint with all required components
pub async fn setup_mint(mint_settings: config::Settings) -> Result<Arc<Mint>> {
    // TODO add to config
    const NUM_KEYS: u8 = 64;

    let mnemonic = Mnemonic::from_str(&mint_settings.info.mnemonic.unwrap())
        .map_err(|e| anyhow::anyhow!("Invalid mnemonic in mint config: {}", e))?;
    let seed_bytes: &[u8] = &mnemonic.to_seed("");

    let hash_currency_unit = CurrencyUnit::Hash;

    let mut currency_units = HashMap::new();
    currency_units.insert(hash_currency_unit.clone(), (0, NUM_KEYS));

    // Database setup with env var override support
    let mut mint_db_path = resolve_and_prepare_db_path(".devenv/state/mint/mint.sqlite");
    
    if let Ok(db_path_override) = std::env::var("CDK_MINT_DB_PATH") {
        tracing::info!("Overriding mint.dbPath with env var CDK_MINT_DB_PATH={}", db_path_override);
        mint_db_path = resolve_and_prepare_db_path(&db_path_override);
    }

    let db = Arc::new(MintSqliteDatabase::new(mint_db_path).await?);

    let signatory = Arc::new(
        DbSignatory::new(
            db.clone(),
            seed_bytes,
            currency_units,
            HashMap::new(),
        ).await.unwrap()
    );

    let ln: HashMap<PaymentProcessorKey, Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>> = HashMap::new();

    // Configure NUT-04 settings for MiningShare payment method with HASH unit
    let mining_share_method = MintMethodSettings {
        method: PaymentMethod::MiningShare,
        unit: hash_currency_unit.clone(),
        min_amount: Some(Amount::from(1)),
        // TODO update units to 2^bits not just raw bits
        max_amount: Some(Amount::from(256)),
        options: None,
    };
    
    let mut nuts = Nuts::new();
    nuts.nut04.methods.push(mining_share_method);
    nuts.nut04.disabled = false;
    
    let mint_info = MintInfo {
        name: Some(mint_settings.mint_info.name.clone()),
        description: Some(mint_settings.mint_info.description.clone()),
        pubkey: None,
        version: None,
        description_long: None,
        contact: None,
        nuts,
        icon_url: None,
        urls: None,
        motd: None,
        time: None,
        tos_url: None,
    };

    let mint = Arc::new(Mint::new(
        mint_info,
        signatory,
        db,
        ln,
    ).await.unwrap());
    
    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;

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
            std::fs::create_dir_all(parent)
                .expect("Failed to create database directory");
        }
    }

    full_path
}