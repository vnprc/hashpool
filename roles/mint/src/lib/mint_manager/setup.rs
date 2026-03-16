use std::{collections::HashMap, path::PathBuf, str::FromStr, sync::Arc};

use anyhow::Result;
use bip39::Mnemonic;
use cdk::{
    cdk_payment::MintPayment,
    mint::{Mint, MintBuilder, MintMeltLimits, UnitConfig},
    nuts::{CurrencyUnit, PaymentMethod},
    types::QuoteTTL,
};
use cdk_ehash::EhashPaymentProcessor;
use cdk_mintd::config;
use cdk_signatory::{db_signatory::DbSignatory, signatory::Signatory};
use cdk_sqlite::MintSqliteDatabase;

/// Setup and initialize the mint with all required components
pub async fn setup_mint(mint_settings: config::Settings, db_path: String) -> Result<Arc<Mint>> {
    // TODO add to config
    const NUM_KEYS: u8 = 64;

    let mnemonic = Mnemonic::from_str(&mint_settings.info.mnemonic.unwrap())
        .map_err(|e| anyhow::anyhow!("Invalid mnemonic in mint config: {}", e))?;
    let seed_bytes: &[u8] = &mnemonic.to_seed("");

    let hash_currency_unit = CurrencyUnit::Custom("hash".to_string());

    let amounts: Vec<u64> = (0..NUM_KEYS as u32).map(|i| 2_u64.pow(i)).collect();
    let mut currency_units = HashMap::new();
    currency_units.insert(hash_currency_unit.clone(), (0_u64, amounts.clone()));

    // Database setup
    let mint_db_path = resolve_and_prepare_db_path(&db_path);

    let db = Arc::new(MintSqliteDatabase::new(mint_db_path).await?);

    let signatory = Arc::new(
        DbSignatory::new(db.clone(), seed_bytes, currency_units, HashMap::new())
            .await
            .unwrap(),
    );

    let ehash_processor = Arc::new(EhashPaymentProcessor::new(hash_currency_unit.clone()));

    let mut builder = MintBuilder::new(db)
        .with_name(mint_settings.mint_info.name.clone())
        .with_description(mint_settings.mint_info.description.clone())
        .with_urls(vec![mint_settings.info.url.clone()]);

    builder
        .configure_unit(
            hash_currency_unit.clone(),
            UnitConfig {
                amounts,
                input_fee_ppk: 0,
            },
        )
        .map_err(|e| anyhow::anyhow!("Failed to configure unit: {}", e))?;

    builder
        .add_payment_processor(
            hash_currency_unit.clone(),
            PaymentMethod::Custom("ehash".to_string()),
            MintMeltLimits::new(1, u64::MAX),
            ehash_processor as Arc<dyn MintPayment<Err = cdk::cdk_payment::Error> + Send + Sync>,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to add payment processor: {}", e))?;

    // Save current NUT-04 config before building (for DB migration below)
    let current_nut04 = builder.current_mint_info().nuts.nut04.clone();

    let signatory_dyn: Arc<dyn Signatory + Send + Sync> = signatory;
    let mint = Arc::new(
        builder
            .build_with_signatory(signatory_dyn)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to build mint: {}", e))?,
    );

    // Ensure NUT-04 settings reflect the current code configuration.
    // Mint::new merges only pubkey/nut21/nut22 from the provided mint_info when a
    // stored config already exists, so NUT-04 may be stale after an upgrade.
    {
        let mut stored_info = mint.mint_info().await?;
        stored_info.nuts.nut04 = current_nut04;
        mint.set_mint_info(stored_info).await?;
    }

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
            std::fs::create_dir_all(parent).expect("Failed to create database directory");
        }
    }

    full_path
}
