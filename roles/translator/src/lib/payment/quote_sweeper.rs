use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use cdk::{
    amount::SplitTarget,
    nuts::{CurrencyUnit, SecretKey, SpendingConditions},
    wallet::{MintQuote, Wallet},
    Amount,
};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::wallet::WalletFactory;

/// Upper bound on quotes minted per unit in a single sweep pass. The first
/// pass after an epoch rotation (or a long outage) can find a large backlog;
/// the cap keeps one `batch_mint` call bounded and lets the backlog drain
/// over successive passes.
const MAX_MINTS_PER_UNIT_PER_PASS: usize = 200;

/// Runs the quote sweeper loop: polls for unissued mint quotes, checks their
/// status, and mints tokens for any that are ready, once per currency unit
/// that has outstanding quotes.
///
/// Spawns an infinite background task via tokio::spawn. The task runs until
/// the process exits.
pub fn spawn_quote_sweeper(factory: Arc<WalletFactory>, locking_privkey: Option<String>) {
    if locking_privkey.is_none() {
        warn!("Quote sweeper running without locking_privkey; minted tokens cannot be signed");
    }

    tokio::spawn(async move {
        // Per-unit wallet handles, keyed by currency unit. Entries are
        // created lazily on first sight of a unit and dropped once the unit
        // has nothing left to mint.
        let wallets: Mutex<HashMap<CurrencyUnit, Arc<Wallet>>> = Mutex::new(HashMap::new());
        let mut loop_count: u64 = 0;
        loop {
            loop_count += 1;
            debug!("Quote sweeper loop #{} starting", loop_count);

            if let Err(e) =
                process_stored_quotes(&factory, &wallets, locking_privkey.as_deref()).await
            {
                error!("Quote processing failed: {}", e);
            }

            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
}

/// Reconciles mint quotes discovered via NUT-XX pubkey lookup into the wallet's
/// local store, returning the fetched quotes so the sweep can derive the set
/// of currency units to mint from them.
///
/// The mint may hold quotes locked to our key that never arrived through the
/// SV2 notification path (dropped message, translator restart, etc). This
/// queries the mint for every quote locked to `secret_key`; `cdk` now does the
/// pubkey validation, accounting, and change-guarded local storage itself (see
/// `Wallet::fetch_mint_quotes_by_pubkey`), so there is no per-quote
/// known-locally check or fetch loop to do here anymore. The lookup is
/// unit-agnostic end to end: quotes of every unit are fetched and persisted
/// with their true unit, which is what lets one designated wallet discover
/// every epoch's quotes.
///
/// Note: the lookup passes `only_mintable: true`, so the mint bounds its response to quotes
/// still mintable for this key instead of the key's full quote history. This is an
/// implementation-side filter, not yet part of the NUT itself - see the spec discussion at
/// <https://github.com/cashubtc/nuts/pull/341>.
///
/// Resilient by design: a failed lookup is logged and treated as an empty
/// result rather than aborting the sweep pass.
async fn reconcile_quotes_by_pubkey(wallet: &Wallet, secret_key: &SecretKey) -> Vec<MintQuote> {
    match wallet
        .fetch_mint_quotes_by_pubkey(&[secret_key.clone()], true)
        .await
    {
        Ok(quotes) => quotes,
        Err(e) => {
            warn!("Pubkey mint quote lookup failed: {}", e);
            Vec::new()
        }
    }
}

/// Derives the set of currency units to sweep this pass: every unit seen in
/// the quotes returned by the pubkey lookup, every unit still tracked from
/// earlier passes (a tracked unit keeps retrying until it has nothing left to
/// mint), and the base unit the translator was configured with. The unit set
/// is never derived from mint info: fetched quotes are the discovery channel,
/// which is what lets the mint retire old-epoch mint-info entries.
fn derive_sweep_units(
    fetched_units: impl IntoIterator<Item = CurrencyUnit>,
    tracked_units: impl IntoIterator<Item = CurrencyUnit>,
    base_unit: CurrencyUnit,
) -> BTreeSet<CurrencyUnit> {
    let mut units: BTreeSet<CurrencyUnit> = fetched_units.into_iter().collect();
    units.extend(tracked_units);
    units.insert(base_unit);
    units
}

/// Truncates `items` to at most `cap` entries, returning how many were
/// deferred to later passes.
fn apply_mint_cap<T>(items: &mut Vec<T>, cap: usize) -> usize {
    let deferred = items.len().saturating_sub(cap);
    items.truncate(cap);
    deferred
}

/// Formats per-unit unissued counts for the reconcile log line, e.g.
/// `unit hash: 2 unissued, unit hash_abc_101: 5 unissued`. Units with no
/// unissued quotes are omitted; when nothing is outstanding the summary reads
/// `no unissued quotes`.
fn summarize_unissued(counts: &[(CurrencyUnit, usize)]) -> String {
    let parts: Vec<String> = counts
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(unit, count)| format!("unit {unit}: {count} unissued"))
        .collect();
    if parts.is_empty() {
        "no unissued quotes".to_string()
    } else {
        parts.join(", ")
    }
}

/// One sweep pass across every currency unit with outstanding quotes.
///
/// Discovery runs once through the base wallet (the pubkey lookup is
/// unit-agnostic and persists quotes of every unit); execution runs once per
/// unit, because a CDK wallet only mints the unit it was constructed with.
/// Per-unit wallet handles are created lazily in `wallets` on first sight of
/// a unit and dropped once the unit has nothing left to mint.
///
/// Returns the total amount minted across all units this pass.
pub async fn process_stored_quotes(
    factory: &WalletFactory,
    wallets: &Mutex<HashMap<CurrencyUnit, Arc<Wallet>>>,
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

    let base = factory.base_wallet();

    // NUT-XX: discover quotes locked to our pubkey that we don't have locally
    // yet (e.g. a missed SV2 notification) before checking what's pending, so
    // this pass's per-unit get_unissued_mint_quotes() below sees them too.
    let fetched = reconcile_quotes_by_pubkey(&base, &secret_key).await;
    let fetched_count = fetched.len();

    let mut wallets = wallets.lock().await;

    let units = derive_sweep_units(
        fetched.into_iter().map(|quote| quote.unit),
        wallets.keys().cloned(),
        base.unit.clone(),
    );

    // Load each unit's unissued quotes through its own wallet handle
    // (Wallet::get_unissued_mint_quotes retains only the wallet's own unit).
    // A unit whose query fails stays tracked and is retried next pass.
    let mut pending: Vec<(CurrencyUnit, Arc<Wallet>, Vec<MintQuote>)> = Vec::new();
    for unit in units {
        let wallet = match wallets.get(&unit) {
            Some(wallet) => wallet.clone(),
            None => match factory.wallet_for_unit(&unit) {
                Ok(wallet) => {
                    wallets.insert(unit.clone(), wallet.clone());
                    wallet
                }
                Err(e) => {
                    error!("unit {unit}: failed to build wallet handle: {e}");
                    continue;
                }
            },
        };
        match wallet.get_unissued_mint_quotes().await {
            Ok(quotes) => pending.push((unit, wallet, quotes)),
            Err(e) => error!("unit {unit}: failed to fetch pending quotes from wallet: {e}"),
        }
    }

    // Per-unit reconcile log: "fetched but never minted" divergence (a unit
    // whose unissued count persists with no mint line following) is the
    // standing detection signal for unit-handling bugs.
    let counts: Vec<(CurrencyUnit, usize)> = pending
        .iter()
        .map(|(unit, _, quotes)| (unit.clone(), quotes.len()))
        .collect();
    info!(
        "reconciled {} quote(s) ({})",
        fetched_count,
        summarize_unissued(&counts)
    );

    let mut total_minted: u64 = 0;
    for (unit, wallet, quotes) in pending {
        if quotes.is_empty() {
            wallets.remove(&unit);
            continue;
        }
        let outcome = sweep_unit(&wallet, &secret_key, quotes).await;
        total_minted += outcome.minted_amount;
        if !outcome.errored && outcome.remaining_unissued == 0 {
            wallets.remove(&unit);
        }
    }

    Ok(total_minted)
}

/// Result of sweeping a single unit's quotes.
struct UnitSweepOutcome {
    /// Unissued quotes left for the unit after this pass (0 when drained).
    remaining_unissued: usize,
    /// Total amount minted for the unit this pass.
    minted_amount: u64,
    /// A step failed; the unit stays tracked and is retried next pass.
    errored: bool,
}

/// Runs one sweep over a single unit's pending quotes: stamp the signing key
/// into each quote record, batch-check status with the mint, and batch-mint
/// whatever is mintable under the P2PK spending conditions.
async fn sweep_unit(
    wallet: &Wallet,
    secret_key: &SecretKey,
    pending_quotes: Vec<MintQuote>,
) -> UnitSweepOutcome {
    let unit = &wallet.unit;
    let unissued = pending_quotes.len();
    let errored = UnitSweepOutcome {
        remaining_unissued: unissued,
        minted_amount: 0,
        errored: true,
    };

    let pubkey = secret_key.public_key();
    let spending_conditions = SpendingConditions::new_p2pk(pubkey, None);

    // Store signing key in each quote's local DB record so batch_mint includes
    // NUT-20 signatures (the mint requires them because quotes are created with pubkey set).
    // Quotes reconciled via pubkey lookup already arrive pre-stamped by
    // fetch_mint_quotes_by_pubkey; this loop still covers quotes discovered by other means
    // (e.g. the SV2 notification path / pre-existing DB rows).
    for mut quote in pending_quotes.iter().cloned() {
        quote.secret_key = Some(secret_key.clone());
        if let Err(e) = wallet.localstore.add_mint_quote(quote).await {
            error!("unit {unit}: failed to store signing key for quote: {e}");
            return errored;
        }
    }

    // Batch check quote status (1 HTTP call instead of N)
    let quote_id_strings: Vec<String> = pending_quotes.iter().map(|q| q.id.clone()).collect();
    let quote_ids: Vec<&str> = quote_id_strings.iter().map(|s| s.as_str()).collect();

    let updated_quotes = match wallet.batch_check_mint_quote_status(&quote_ids).await {
        Ok(quotes) => quotes,
        Err(e) => {
            error!("unit {unit}: failed to batch check quote status: {e}");
            return errored;
        }
    };

    let mut mintable_id_strings: Vec<String> = updated_quotes
        .iter()
        .filter(|q| q.amount_mintable() != Amount::ZERO)
        .map(|q| q.id.clone())
        .collect();

    if mintable_id_strings.is_empty() {
        debug!("unit {unit}: no mintable quotes after batch status check");
        return UnitSweepOutcome {
            remaining_unissued: unissued,
            minted_amount: 0,
            errored: false,
        };
    }

    let deferred = apply_mint_cap(&mut mintable_id_strings, MAX_MINTS_PER_UNIT_PER_PASS);
    if deferred > 0 {
        info!(
            "unit {unit}: capping mint batch to {MAX_MINTS_PER_UNIT_PER_PASS} quote(s) \
             ({deferred} deferred); the backlog drains over subsequent passes"
        );
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
            error!("unit {unit}: batch mint failed: {e}");
            return errored;
        }
    };

    let minted_amount: u64 = proofs.iter().map(|p| u64::from(p.amount)).sum();
    info!(
        "Minted {} {} from {} quote(s)",
        minted_amount,
        unit,
        mintable_ids.len()
    );

    if minted_amount > 0 {
        if let Ok(balance) = wallet.total_balance().await {
            info!("unit {unit} balance after sweep: {balance}");
        }
    }

    UnitSweepOutcome {
        remaining_unissued: unissued.saturating_sub(mintable_ids.len()),
        minted_amount,
        errored: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str) -> CurrencyUnit {
        CurrencyUnit::Custom(name.to_string().into())
    }

    #[test]
    fn derive_sweep_units_unions_fetched_tracked_and_base() {
        let fetched = vec![unit("hash_a"), unit("hash_b"), unit("hash_a")];
        let tracked = vec![unit("hash_c"), unit("hash_b")];
        let units = derive_sweep_units(fetched, tracked, unit("hash"));

        let expected: BTreeSet<CurrencyUnit> =
            [unit("hash"), unit("hash_a"), unit("hash_b"), unit("hash_c")]
                .into_iter()
                .collect();
        assert_eq!(units, expected);
    }

    #[test]
    fn derive_sweep_units_always_contains_base_unit() {
        let units = derive_sweep_units(Vec::new(), Vec::new(), unit("hash"));
        assert_eq!(units.len(), 1);
        assert!(units.contains(&unit("hash")));
    }

    #[test]
    fn derive_sweep_units_dedupes_base_against_fetched() {
        let units = derive_sweep_units(vec![unit("hash")], vec![unit("hash")], unit("hash"));
        assert_eq!(units.len(), 1);
    }

    #[test]
    fn apply_mint_cap_under_cap_defers_nothing() {
        let mut ids = vec!["a", "b"];
        assert_eq!(apply_mint_cap(&mut ids, 3), 0);
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn apply_mint_cap_at_cap_defers_nothing() {
        let mut ids = vec!["a", "b", "c"];
        assert_eq!(apply_mint_cap(&mut ids, 3), 0);
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn apply_mint_cap_over_cap_truncates_and_reports_deferred() {
        let mut ids: Vec<usize> = (0..250).collect();
        assert_eq!(apply_mint_cap(&mut ids, 200), 50);
        assert_eq!(ids.len(), 200);
        // Keeps the first `cap` items in order.
        assert_eq!(ids.first(), Some(&0));
        assert_eq!(ids.last(), Some(&199));
    }

    #[test]
    fn apply_mint_cap_zero_cap_defers_everything() {
        let mut ids = vec!["a", "b"];
        assert_eq!(apply_mint_cap(&mut ids, 0), 2);
        assert!(ids.is_empty());
    }

    #[test]
    fn summarize_unissued_reports_no_work() {
        assert_eq!(summarize_unissued(&[]), "no unissued quotes");
        assert_eq!(
            summarize_unissued(&[(unit("hash"), 0)]),
            "no unissued quotes"
        );
    }

    #[test]
    fn summarize_unissued_lists_units_with_work_and_omits_idle_units() {
        let summary = summarize_unissued(&[
            (unit("hash"), 2),
            (unit("hash_abc_101"), 0),
            (unit("hash_xyz_150"), 5),
        ]);
        assert_eq!(
            summary,
            "unit hash: 2 unissued, unit hash_xyz_150: 5 unissued"
        );
    }
}
