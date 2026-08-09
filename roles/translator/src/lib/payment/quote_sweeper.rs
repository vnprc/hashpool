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

/// Reconciles mint quotes discovered via NUT-XX pubkey lookup into the wallet's
/// local store.
///
/// The mint may hold quotes locked to our key that never arrived through the
/// SV2 notification path (dropped message, translator restart, etc). This
/// queries the mint for every quote locked to `secret_key` and, for any quote
/// not already known locally, fetches and stores it the same way the
/// notification handler used to: via `Wallet::fetch_mint_quote`, which upserts
/// through cdk's own accounting logic rather than hand-constructing a record.
///
/// Resilient by design: a failed lookup or a single bad quote is logged and
/// skipped rather than aborting the sweep pass.
async fn reconcile_quotes_by_pubkey(wallet: &Arc<Wallet>, secret_key: &SecretKey) {
    let quotes = match wallet.mint_quotes_by_pubkey(&[secret_key.clone()]).await {
        Ok(quotes) => quotes,
        Err(e) => {
            warn!("Pubkey mint quote lookup failed: {}", e);
            return;
        }
    };

    let mut reconciled = 0u32;
    for quote in quotes {
        let already_known = match wallet.localstore.get_mint_quote(quote.quote()).await {
            Ok(existing) => existing.is_some(),
            Err(e) => {
                warn!(
                    "Failed to check localstore for quote {} during pubkey reconcile: {}",
                    quote.quote(),
                    e
                );
                continue;
            }
        };

        if already_known {
            continue;
        }

        match wallet
            .fetch_mint_quote(quote.quote(), Some(quote.method()))
            .await
        {
            Ok(_) => reconciled += 1,
            Err(e) => warn!(
                "Failed to reconcile mint quote {} from pubkey lookup: {}",
                quote.quote(),
                e
            ),
        }
    }

    if reconciled > 0 {
        info!(
            "Reconciled {} new mint quote(s) via pubkey lookup",
            reconciled
        );
    }
}

pub async fn process_stored_quotes(
    wallet: &Arc<Wallet>,
    locking_privkey: Option<&str>,
) -> anyhow::Result<u64> {
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

    // NUT-XX: discover quotes locked to our pubkey that we don't have locally
    // yet (e.g. a missed SV2 notification) before checking what's pending, so
    // this pass's get_unissued_mint_quotes() below sees them too.
    reconcile_quotes_by_pubkey(wallet, &secret_key).await;

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
