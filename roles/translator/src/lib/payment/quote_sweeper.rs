use std::sync::Arc;
use std::time::Duration;

use cdk::{
    amount::SplitTarget,
    nuts::{SecretKey, SpendingConditions},
    wallet::Wallet,
    Amount,
};
use tracing::{debug, error, info, warn};

/// Runs the quote sweeper loop: polls for unissued mint quotes, checks their
/// status, and mints tokens for any that are ready.
///
/// Spawns an infinite background task via tokio::spawn. The task runs until
/// the process exits.
pub fn spawn_quote_sweeper(wallet: Arc<Wallet>, locking_privkey: Option<String>) {
    if locking_privkey.is_none() {
        warn!("Quote sweeper running without locking_privkey; minted tokens cannot be signed");
    }

    tokio::spawn(async move {
        let mut loop_count: u64 = 0;
        loop {
            loop_count += 1;
            debug!("Quote sweeper loop #{} starting", loop_count);

            match process_stored_quotes(&wallet, locking_privkey.as_deref()).await {
                Ok(minted_amount) => {
                    if minted_amount > 0 {
                        if let Ok(balance) = wallet.total_balance().await {
                            info!("Wallet balance after sweep: {} ehash", balance);
                        }
                    }
                }
                Err(e) => {
                    error!("Quote processing failed: {}", e);
                }
            }

            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
}

pub async fn process_stored_quotes(
    wallet: &Arc<Wallet>,
    locking_privkey: Option<&str>,
) -> anyhow::Result<u64> {
    let pending_quotes = match wallet.get_unissued_mint_quotes().await {
        Ok(quotes) => quotes,
        Err(e) => {
            error!("Failed to fetch pending quotes from wallet: {}", e);
            return Ok(0);
        }
    };

    info!("Found {} pending quotes", pending_quotes.len());

    if pending_quotes.is_empty() {
        return Ok(0);
    }

    let secret_key = match locking_privkey {
        Some(privkey_hex) => match hex::decode(privkey_hex) {
            Ok(privkey_bytes) => match SecretKey::from_slice(&privkey_bytes) {
                Ok(sk) => sk,
                Err(e) => {
                    error!("Invalid secret key format: {}", e);
                    return Ok(0);
                }
            },
            Err(e) => {
                error!("Failed to decode secret key hex: {}", e);
                return Ok(0);
            }
        },
        None => {
            debug!("Skipping mint: no locking_privkey configured");
            return Ok(0);
        }
    };

    let pubkey = secret_key.public_key();
    let spending_conditions = SpendingConditions::new_p2pk(pubkey, None);

    // Store signing key in each quote's local DB record so batch_mint includes
    // NUT-20 signatures (the mint requires them because quotes are created with pubkey set).
    for mut quote in pending_quotes.iter().cloned() {
        quote.secret_key = Some(secret_key.clone());
        if let Err(e) = wallet.localstore.add_mint_quote(quote).await {
            error!("Failed to store signing key for quote: {}", e);
            return Ok(0);
        }
    }

    // Batch check quote status (1 HTTP call instead of N)
    let quote_id_strings: Vec<String> = pending_quotes.iter().map(|q| q.id.clone()).collect();
    let quote_ids: Vec<&str> = quote_id_strings.iter().map(|s| s.as_str()).collect();

    let updated_quotes = match wallet.batch_check_mint_quote_status(&quote_ids).await {
        Ok(quotes) => quotes,
        Err(e) => {
            error!("Failed to batch check quote status: {}", e);
            return Ok(0);
        }
    };

    let mintable_id_strings: Vec<String> = updated_quotes
        .iter()
        .filter(|q| q.amount_mintable() != Amount::ZERO)
        .map(|q| q.id.clone())
        .collect();

    if mintable_id_strings.is_empty() {
        debug!("No mintable quotes after batch status check");
        return Ok(0);
    }

    let mintable_ids: Vec<&str> = mintable_id_strings.iter().map(|s| s.as_str()).collect();

    // Batch mint (1 HTTP call instead of N)
    let proofs = match wallet
        .batch_mint(
            &mintable_ids,
            SplitTarget::default(),
            Some(spending_conditions),
            None,
        )
        .await
    {
        Ok(p) => p,
        Err(e) => {
            error!("Batch mint failed: {}", e);
            return Ok(0);
        }
    };

    let total_minted: u64 = proofs.iter().map(|p| u64::from(p.amount)).sum();
    info!(
        "Minted {} ehash from {} quotes",
        total_minted,
        mintable_ids.len()
    );

    Ok(total_minted)
}
