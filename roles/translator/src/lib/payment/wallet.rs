use std::{path::Path, str::FromStr, sync::Arc};

use anyhow::{Context, Result};
use bip39::Mnemonic;
use cdk::{
    mint_url::MintUrl,
    nuts::CurrencyUnit,
    wallet::{Wallet, WalletBuilder},
};
use cdk_sqlite::WalletSqliteDatabase;
use tracing::debug;

/// Shared wallet resources plus cheap per-unit wallet construction.
///
/// A CDK wallet only mints the unit it was constructed with, but the mint
/// issues quotes in a new currency unit every epoch. The factory opens the
/// shared resources once (one sqlite localstore, one seed, one mint URL) and
/// builds a wallet handle per currency unit on demand. Every handle shares
/// the one localstore and the base wallet's mint metadata cache, so building
/// one writes nothing and makes no network calls.
pub struct WalletFactory {
    mint_url: MintUrl,
    seed: [u8; 64],
    localstore: Arc<WalletSqliteDatabase>,
    /// Wallet for the translator's configured unit. It also owns the
    /// `MintMetadataCache` every derived handle shares: the cache type is not
    /// nameable outside cdk, so it travels through the base wallet's public
    /// `metadata_cache` field.
    base: Arc<Wallet>,
}

impl WalletFactory {
    /// The wallet for the translator's configured unit (`hash`), used for
    /// quote discovery, the faucet, and quote notifications.
    pub fn base_wallet(&self) -> Arc<Wallet> {
        self.base.clone()
    }

    /// Builds a wallet handle for `unit`, sharing the factory's localstore,
    /// seed, and metadata cache. Returns the base wallet when `unit` is the
    /// base unit. Construction is synchronous and writes nothing.
    pub fn wallet_for_unit(&self, unit: &CurrencyUnit) -> Result<Arc<Wallet>> {
        if *unit == self.base.unit {
            return Ok(self.base.clone());
        }
        let wallet = WalletBuilder::new()
            .mint_url(self.mint_url.clone())
            .unit(unit.clone())
            .localstore(self.localstore.clone())
            .seed(self.seed)
            .metadata_cache(self.base.metadata_cache.clone())
            .build()
            .with_context(|| format!("Failed to build wallet for unit {unit}"))?;
        Ok(Arc::new(wallet))
    }
}

/// Opens the shared wallet resources (localstore, seed) and builds the base
/// wallet, returning the factory that derives per-unit wallets from them.
pub async fn create_wallet(mint_url: &str, mnemonic: &str, db_path: &str) -> Result<WalletFactory> {
    debug!("Parsing mnemonic...");
    let seed: [u8; 64] = Mnemonic::from_str(mnemonic)
        .with_context(|| format!("Invalid mnemonic: '{}'", mnemonic))?
        .to_seed_normalized("");

    // Priority: CDK_WALLET_DB_PATH env var > config db_path (mirrors mint's CDK_MINT_DB_PATH logic)
    let effective_path = std::env::var("CDK_WALLET_DB_PATH")
        .ok()
        .unwrap_or_else(|| db_path.to_string());
    let db_path = resolve_db_path(&effective_path);
    debug!("Resolved db_path: {}", db_path.display());

    let localstore = Arc::new(
        WalletSqliteDatabase::new(db_path)
            .await
            .context("WalletSqliteDatabase::new failed")?,
    );

    let mint_url = MintUrl::from_str(mint_url).context("Invalid mint URL")?;

    let base = WalletBuilder::new()
        .mint_url(mint_url.clone())
        .unit(CurrencyUnit::Custom("hash".to_string().into()))
        .localstore(localstore.clone())
        .seed(seed)
        .build()
        .context("Failed to create wallet")?;
    let base = Arc::new(base);

    let balance = base
        .total_balance()
        .await
        .context("Failed to get wallet balance")?;
    debug!("Wallet initialized, balance: {:?}", balance);

    Ok(WalletFactory {
        mint_url,
        seed,
        localstore,
        base,
    })
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
            std::fs::create_dir_all(parent).expect("Failed to create parent directory for DB path");
        }
    }

    full_path
}
