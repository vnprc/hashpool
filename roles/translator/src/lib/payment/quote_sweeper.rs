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

/// Upper bound on quotes examined (stamped, status-checked, minted) per unit
/// in a single sweep pass. The first pass after an epoch rotation (or a long
/// outage) can find a large backlog; the cap bounds the whole per-unit
/// pipeline and lets the backlog drain over successive passes.
const MAX_QUOTES_PER_UNIT_PER_PASS: usize = 200;

/// Size of the chunks a pass's per-unit selection is split into for the
/// mint's batch endpoints. The mint's `check_mint_quotes` fails a whole batch
/// when one quote id is unknown to it, so a poisoned quote wedges at most one
/// chunk while the other chunks keep minting.
const SWEEP_CHUNK_SIZE: usize = 50;

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
/// the quotes returned by the pubkey lookup, every unit with unissued quotes
/// in the local store (so a persistently failing lookup endpoint cannot hide
/// DB backlog from the sweep or from the reconcile log), every unit still
/// tracked from earlier passes, and the base unit the translator was
/// configured with. The unit set is never derived from mint info: quotes are
/// the discovery channel, which is what lets the mint retire old-epoch
/// mint-info entries.
fn derive_sweep_units(
    fetched_units: impl IntoIterator<Item = CurrencyUnit>,
    db_backlog_units: impl IntoIterator<Item = CurrencyUnit>,
    tracked_units: impl IntoIterator<Item = CurrencyUnit>,
    base_unit: CurrencyUnit,
) -> BTreeSet<CurrencyUnit> {
    let mut units: BTreeSet<CurrencyUnit> = fetched_units.into_iter().collect();
    units.extend(db_backlog_units);
    units.extend(tracked_units);
    units.insert(base_unit);
    units
}

/// Plans one unit's sweep work for a pass: items that look ready are selected
/// first (stable order otherwise), the selection is capped at `cap`, and the
/// capped selection is split into chunks of `chunk_size` for the mint's batch
/// endpoints. Returns the chunks and how many items were deferred to later
/// passes.
///
/// The ready-first ordering means quotes that already look mintable from
/// local state cannot be starved behind a head-of-line block of stale or
/// unpaid ones when a backlog exceeds the cap; the mint's batch status check
/// still decides the truth for everything selected.
fn plan_sweep<T>(
    items: Vec<T>,
    looks_ready: impl Fn(&T) -> bool,
    cap: usize,
    chunk_size: usize,
) -> (Vec<Vec<T>>, usize) {
    let (ready, not_ready): (Vec<T>, Vec<T>) = items.into_iter().partition(looks_ready);
    let mut selected = ready;
    selected.extend(not_ready);

    let deferred = selected.len().saturating_sub(cap);
    selected.truncate(cap);

    let chunk_size = chunk_size.max(1);
    let mut chunks: Vec<Vec<T>> = Vec::new();
    let mut items = selected.into_iter();
    loop {
        let chunk: Vec<T> = items.by_ref().take(chunk_size).collect();
        if chunk.is_empty() {
            break;
        }
        chunks.push(chunk);
    }

    (chunks, deferred)
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
/// Discovery has two channels: the pubkey lookup through the base wallet
/// (unit-agnostic; persists quotes of every unit) and one unit-blind read of
/// the local store's unissued quotes, so a failing lookup endpoint cannot
/// hide DB backlog. Execution runs once per unit, because a CDK wallet only
/// mints the unit it was constructed with. Per-unit wallet handles are
/// created lazily in `wallets` on first sight of a unit and dropped once the
/// unit has nothing left to mint.
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

    // The lookup is one discovery channel; the local store is the other. The
    // localstore-level query is unit-blind (the unit filter in
    // Wallet::get_unissued_mint_quotes is a wallet-level retain, not SQL), so
    // one call surfaces every unit with DB backlog even when the lookup
    // endpoint fails persistently.
    let db_backlog_units: Vec<CurrencyUnit> = match base.localstore.get_unissued_mint_quotes().await
    {
        Ok(quotes) => quotes
            .into_iter()
            .filter(|quote| quote.mint_url == base.mint_url)
            .map(|quote| quote.unit)
            .collect(),
        Err(e) => {
            warn!("Failed to read unissued quotes from the local store: {}", e);
            Vec::new()
        }
    };

    let mut wallets = wallets.lock().await;

    let units = derive_sweep_units(
        fetched.into_iter().map(|quote| quote.unit),
        db_backlog_units,
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

/// Runs one sweep over a single unit's pending quotes. The pending set is
/// capped and split into chunks up front, so the whole pipeline (signing-key
/// stamping, batch status check, batch mint) is bounded per pass; a failed
/// chunk is logged and skipped while the remaining chunks keep going.
async fn sweep_unit(
    wallet: &Wallet,
    secret_key: &SecretKey,
    pending_quotes: Vec<MintQuote>,
) -> UnitSweepOutcome {
    let unit = &wallet.unit;
    let unissued = pending_quotes.len();

    let (chunks, deferred) = plan_sweep(
        pending_quotes,
        |quote| quote.amount_mintable() != Amount::ZERO,
        MAX_QUOTES_PER_UNIT_PER_PASS,
        SWEEP_CHUNK_SIZE,
    );
    if deferred > 0 {
        info!(
            "unit {unit}: sweeping {} of {unissued} unissued quote(s) this pass \
             ({deferred} deferred); the backlog drains over subsequent passes",
            unissued - deferred
        );
    }

    let pubkey = secret_key.public_key();
    let spending_conditions = SpendingConditions::new_p2pk(pubkey, None);

    let mut minted_quotes: usize = 0;
    let mut minted_amount: u64 = 0;
    let mut errored = false;

    for chunk in chunks {
        match sweep_chunk(wallet, secret_key, &spending_conditions, chunk).await {
            Some((quotes, amount)) => {
                minted_quotes += quotes;
                minted_amount += amount;
            }
            None => errored = true,
        }
    }

    if minted_quotes > 0 {
        info!("Minted {minted_amount} {unit} from {minted_quotes} quote(s)");
        if let Ok(balance) = wallet.total_balance().await {
            info!("unit {unit} balance after sweep: {balance}");
        }
    } else if !errored {
        debug!("unit {unit}: no mintable quotes after batch status check");
    }

    UnitSweepOutcome {
        remaining_unissued: unissued.saturating_sub(minted_quotes),
        minted_amount,
        errored,
    }
}

/// Processes one chunk of a unit's pending quotes: stamp the signing key into
/// each quote's local record, batch-check status with the mint, and
/// batch-mint whatever is mintable under the P2PK spending conditions.
///
/// Returns the minted (quote count, amount), or None after logging when a
/// step failed. A failed chunk does not stop the unit's later chunks: the
/// mint's `check_mint_quotes` fails a whole batch on one unknown quote id, so
/// chunking bounds that blast radius to `SWEEP_CHUNK_SIZE` quotes.
async fn sweep_chunk(
    wallet: &Wallet,
    secret_key: &SecretKey,
    spending_conditions: &SpendingConditions,
    chunk: Vec<MintQuote>,
) -> Option<(usize, u64)> {
    let unit = &wallet.unit;

    // Store signing key in each quote's local DB record so batch_mint includes
    // NUT-20 signatures (the mint requires them because quotes are created with pubkey set).
    // Quotes reconciled via pubkey lookup already arrive pre-stamped by
    // fetch_mint_quotes_by_pubkey; this loop still covers quotes discovered by other means
    // (e.g. the SV2 notification path / pre-existing DB rows).
    for mut quote in chunk.iter().cloned() {
        quote.secret_key = Some(secret_key.clone());
        if let Err(e) = wallet.localstore.add_mint_quote(quote).await {
            error!("unit {unit}: failed to store signing key for quote: {e}");
            return None;
        }
    }

    // Batch check quote status (1 HTTP call per chunk instead of N)
    let quote_id_strings: Vec<String> = chunk.iter().map(|q| q.id.clone()).collect();
    let quote_ids: Vec<&str> = quote_id_strings.iter().map(|s| s.as_str()).collect();

    let updated_quotes = match wallet.batch_check_mint_quote_status(&quote_ids).await {
        Ok(quotes) => quotes,
        Err(e) => {
            error!(
                "unit {unit}: batch status check failed for a chunk of {} quote(s): {e}",
                quote_ids.len()
            );
            return None;
        }
    };

    let mintable_id_strings: Vec<String> = updated_quotes
        .iter()
        .filter(|q| q.amount_mintable() != Amount::ZERO)
        .map(|q| q.id.clone())
        .collect();

    if mintable_id_strings.is_empty() {
        return Some((0, 0));
    }

    let mintable_ids: Vec<&str> = mintable_id_strings.iter().map(|s| s.as_str()).collect();

    // Batch mint (1 HTTP call per chunk instead of N)
    let proofs = match wallet
        .batch_mint(
            &mintable_ids,
            SplitTarget::default(),
            Some(spending_conditions.clone()),
            None,
        )
        .await
    {
        Ok(p) => p,
        Err(e) => {
            error!(
                "unit {unit}: batch mint failed for a chunk of {} quote(s): {e}",
                mintable_ids.len()
            );
            return None;
        }
    };

    let amount: u64 = proofs.iter().map(|p| u64::from(p.amount)).sum();
    Some((mintable_ids.len(), amount))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str) -> CurrencyUnit {
        CurrencyUnit::Custom(name.to_string().into())
    }

    #[test]
    fn derive_sweep_units_unions_all_sources_and_base() {
        let fetched = vec![unit("hash_a"), unit("hash_b"), unit("hash_a")];
        let db_backlog = vec![unit("hash_d"), unit("hash_a")];
        let tracked = vec![unit("hash_c"), unit("hash_b")];
        let units = derive_sweep_units(fetched, db_backlog, tracked, unit("hash"));

        let expected: BTreeSet<CurrencyUnit> = [
            unit("hash"),
            unit("hash_a"),
            unit("hash_b"),
            unit("hash_c"),
            unit("hash_d"),
        ]
        .into_iter()
        .collect();
        assert_eq!(units, expected);
    }

    #[test]
    fn derive_sweep_units_includes_db_backlog_when_lookup_returns_nothing() {
        // A persistently failing pubkey lookup must not hide units whose
        // unissued quotes are already in the local store.
        let units = derive_sweep_units(
            Vec::new(),
            vec![unit("hash_old_epoch")],
            Vec::new(),
            unit("hash"),
        );
        assert_eq!(units.len(), 2);
        assert!(units.contains(&unit("hash_old_epoch")));
        assert!(units.contains(&unit("hash")));
    }

    #[test]
    fn derive_sweep_units_always_contains_base_unit() {
        let units = derive_sweep_units(Vec::new(), Vec::new(), Vec::new(), unit("hash"));
        assert_eq!(units.len(), 1);
        assert!(units.contains(&unit("hash")));
    }

    #[test]
    fn derive_sweep_units_dedupes_base_against_sources() {
        let units = derive_sweep_units(
            vec![unit("hash")],
            vec![unit("hash")],
            vec![unit("hash")],
            unit("hash"),
        );
        assert_eq!(units.len(), 1);
    }

    #[test]
    fn plan_sweep_orders_ready_items_first_within_cap() {
        let items = vec![1, 2, 3, 4, 5, 6];
        // Even numbers "look mintable": they fill the cap before any odd one.
        let (chunks, deferred) = plan_sweep(items, |n| n % 2 == 0, 3, 2);
        assert_eq!(deferred, 3);
        assert_eq!(chunks, vec![vec![2, 4], vec![6]]);
    }

    #[test]
    fn plan_sweep_preserves_order_when_nothing_is_ready() {
        let (chunks, deferred) = plan_sweep(vec![1, 2, 3], |_| false, 10, 10);
        assert_eq!(deferred, 0);
        assert_eq!(chunks, vec![vec![1, 2, 3]]);
    }

    #[test]
    fn plan_sweep_empty_input_yields_no_chunks() {
        let (chunks, deferred) = plan_sweep(Vec::<u32>::new(), |_| true, 5, 5);
        assert!(chunks.is_empty());
        assert_eq!(deferred, 0);
    }

    #[test]
    fn plan_sweep_at_cap_defers_nothing() {
        let (chunks, deferred) = plan_sweep(vec![1, 2, 3], |_| true, 3, 2);
        assert_eq!(deferred, 0);
        assert_eq!(chunks, vec![vec![1, 2], vec![3]]);
    }

    #[test]
    fn plan_sweep_caps_total_work_and_chunks_the_selection() {
        let items: Vec<usize> = (0..250).collect();
        let (chunks, deferred) = plan_sweep(items, |_| true, 200, 50);
        assert_eq!(deferred, 50);
        assert_eq!(chunks.len(), 4);
        assert!(chunks.iter().all(|chunk| chunk.len() == 50));
        // Order preserved: first selected item first, cap boundary respected.
        assert_eq!(chunks[0][0], 0);
        assert_eq!(chunks[3][49], 199);
    }

    #[test]
    fn plan_sweep_zero_chunk_size_is_treated_as_one() {
        let (chunks, deferred) = plan_sweep(vec![1, 2], |_| true, 5, 0);
        assert_eq!(deferred, 0);
        assert_eq!(chunks, vec![vec![1], vec![2]]);
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
