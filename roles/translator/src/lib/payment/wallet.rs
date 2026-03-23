use std::{path::Path, str::FromStr, sync::Arc};

use anyhow::{Context, Result};
use bip39::Mnemonic;
use cdk::{nuts::CurrencyUnit, wallet::Wallet};
use cdk_sqlite::WalletSqliteDatabase;
use tracing::debug;

/// Creates and initializes a CDK wallet for the translator.
pub async fn create_wallet(mint_url: &str, mnemonic: &str, db_path: &str) -> Result<Arc<Wallet>> {
    debug!("Parsing mnemonic...");
    let seed = Mnemonic::from_str(mnemonic)
        .with_context(|| format!("Invalid mnemonic: '{}'", mnemonic))?
        .to_seed_normalized("");
    let seed: [u8; 64] = seed
        .try_into()
        .map_err(|_| anyhow::anyhow!("Seed must be exactly 64 bytes"))?;

    // Priority: CDK_WALLET_DB_PATH env var > config db_path (mirrors mint's CDK_MINT_DB_PATH logic)
    let effective_path = std::env::var("CDK_WALLET_DB_PATH")
        .ok()
        .unwrap_or_else(|| db_path.to_string());
    let db_path = resolve_db_path(&effective_path);
    debug!("Resolved db_path: {}", db_path.display());

    let localstore = WalletSqliteDatabase::new(db_path)
        .await
        .context("WalletSqliteDatabase::new failed")?;

    let wallet = Wallet::new(
        mint_url,
        CurrencyUnit::Custom("hash".to_string()),
        Arc::new(localstore),
        seed,
        None,
    )
    .context("Failed to create wallet")?;

    let balance = wallet
        .total_balance()
        .await
        .context("Failed to get wallet balance")?;
    debug!("Wallet initialized, balance: {:?}", balance);

    Ok(Arc::new(wallet))
}

fn resolve_db_path(config_path: &str) -> std::path::PathBuf {
    let path = Path::new(config_path);
    let full_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .expect("Failed to get current working directory")
            .join(path)
    };

    if let Some(parent) = full_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .expect("Failed to create parent directory for DB path");
        }
    }

    full_path
}
