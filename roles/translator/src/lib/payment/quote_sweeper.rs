use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use cdk::{
    amount::SplitTarget,
    nuts::{CurrencyUnit, SecretKey, SpendingConditions},
    wallet::{types::KeysetLoadPolicy, MintQuote, Wallet},
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

/// Returns the fetched quotes so the sweep can derive the units to mint from them.
///
/// Unit-agnostic end to end, which is what lets one wallet discover every epoch's
/// quotes. Passes `only_mintable: true`, an implementation-side filter not yet part
/// of the NUT: <https://github.com/cashubtc/nuts/pull/341>.
/// A failed lookup is logged and treated as empty rather than aborting the pass.
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

/// The mint metadata cache is per-mint, not per-unit, so a cache populated by
/// an older unit reads as fresh for a brand-new epoch unit and every mint then
/// fails with "Unknown Keyset". Forces one refresh instead. See vnprc/hashpool#29.
/// Returns whether the unit's keyset is now cached; the caller must not mint
/// into this unit's quotes when it returns false.
async fn ensure_keyset_cached(wallet: &Wallet) -> bool {
    if wallet.keysets(KeysetLoadPolicy::CacheOnly).await.is_ok() {
        return true;
    }
    match wallet.keysets(KeysetLoadPolicy::Refresh).await {
        Ok(_) => true,
        Err(e) => {
            warn!(
                "unit {}: failed to refresh keysets on first sight: {e}",
                wallet.unit
            );
            false
        }
    }
}

/// Clears reservations a failed mint left behind, which otherwise wedge a paid
/// quote forever behind "Quote already in use by another operation". Runs
/// every pass, not just at startup, so a leak from the same pass clears too.
/// Uses cdk's narrow `cleanup_orphaned_quote_reservations`, not
/// `recover_incomplete_sagas`: the narrow call never resumes or compensates a
/// saga, so it is safe on the base wallet's handle even though the faucet
/// drives its own Send saga over the same handle.
async fn recover_leaked_reservations(wallet: &Wallet) {
    let unit = &wallet.unit;

    let cleared = match wallet.cleanup_orphaned_quote_reservations().await {
        Ok(cleared) => cleared,
        Err(e) => {
            error!("unit {unit}: orphaned reservation cleanup failed: {e}");
            return;
        }
    };

    if cleared > 0 {
        info!("unit {unit}: recovery cleared {cleared} leaked quote reservation(s)");
    }
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

    // Runs before the per-unit get_unissued_mint_quotes() so this pass also sees
    // quotes a missed SV2 notification would otherwise hide until the next one.
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
                    if !ensure_keyset_cached(&wallet).await {
                        // Skip this pass rather than reserve quotes behind a
                        // keyset we just failed to look up; the next pass
                        // retries from the local-store backlog.
                        continue;
                    }
                    wallets.insert(unit.clone(), wallet.clone());
                    wallet
                }
                Err(e) => {
                    error!("unit {unit}: failed to build wallet handle: {e}");
                    continue;
                }
            },
        };
        recover_leaked_reservations(&wallet).await;
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
    // Pubkey-lookup quotes arrive pre-stamped; this covers other discovery paths.
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

    /// `MintConnector` double serving canned keyset data; cdk's own
    /// `MockMintConnector` is `#[cfg(test)]`-gated inside the cdk crate and
    /// unusable from here.
    #[derive(Clone)]
    struct FakeMintConnector {
        keyset_infos: Arc<Mutex<Vec<cdk::nuts::KeySetInfo>>>,
        full_keysets: Arc<Mutex<HashMap<cdk::nuts::Id, cdk::nuts::KeySet>>>,
        signing_keys: Arc<Mutex<HashMap<cdk::nuts::Id, SecretKey>>>,
    }

    impl std::fmt::Debug for FakeMintConnector {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FakeMintConnector").finish()
        }
    }

    impl FakeMintConnector {
        fn new(
            keyset_infos: Vec<cdk::nuts::KeySetInfo>,
            full_keysets: Vec<cdk::nuts::KeySet>,
        ) -> Self {
            Self {
                keyset_infos: Arc::new(Mutex::new(keyset_infos)),
                full_keysets: Arc::new(Mutex::new(
                    full_keysets.into_iter().map(|k| (k.id, k)).collect(),
                )),
                signing_keys: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        async fn add_keyset(&self, info: cdk::nuts::KeySetInfo, full: cdk::nuts::KeySet) {
            self.keyset_infos.lock().await.push(info);
            self.full_keysets.lock().await.insert(full.id, full);
        }

        async fn add_signing_key(&self, keyset_id: cdk::nuts::Id, key: SecretKey) {
            self.signing_keys.lock().await.insert(keyset_id, key);
        }
    }

    #[allow(unused_variables)]
    #[async_trait::async_trait]
    impl cdk::wallet::MintConnector for FakeMintConnector {
        // cdk's default features (always on in this workspace, see
        // roles/Cargo.toml) include "bip353", which adds this method to the
        // trait; translator_sv2 has no feature of its own to mirror that
        // gate with, so this stub is unconditional.
        async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn fetch_lnurl_pay_request(
            &self,
            url: &str,
        ) -> Result<cdk::wallet::LnurlPayResponse, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn fetch_lnurl_invoice(
            &self,
            url: &str,
        ) -> Result<cdk::wallet::LnurlPayInvoiceResponse, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn get_mint_keys(&self) -> Result<Vec<cdk::nuts::KeySet>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn get_mint_keyset(
            &self,
            keyset_id: cdk::nuts::Id,
        ) -> Result<cdk::nuts::KeySet, cdk::Error> {
            self.full_keysets
                .lock()
                .await
                .get(&keyset_id)
                .cloned()
                .ok_or(cdk::Error::UnknownKeySet)
        }

        async fn get_mint_keysets(&self) -> Result<cdk::nuts::KeysetResponse, cdk::Error> {
            Ok(cdk::nuts::KeysetResponse {
                keysets: self.keyset_infos.lock().await.clone(),
            })
        }

        async fn post_mint_quote(
            &self,
            request: cdk::MintQuoteRequest,
        ) -> Result<cdk::MintQuoteResponse<String>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn post_mint(
            &self,
            method: &cdk::nuts::PaymentMethod,
            request: cdk::nuts::MintRequest<String>,
        ) -> Result<cdk::nuts::MintResponse, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn post_batch_check_mint_quote_status(
            &self,
            method: &cdk::nuts::PaymentMethod,
            request: cdk::nuts::BatchCheckMintQuoteRequest<String>,
        ) -> Result<Vec<cdk::MintQuoteResponse<String>>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn post_batch_mint(
            &self,
            _method: &cdk::nuts::PaymentMethod,
            request: cdk::nuts::BatchMintRequest<String>,
        ) -> Result<cdk::nuts::MintResponse, cdk::Error> {
            let signing_keys = self.signing_keys.lock().await;
            let mut signatures = Vec::with_capacity(request.outputs.len());
            for output in &request.outputs {
                let key = signing_keys
                    .get(&output.keyset_id)
                    .ok_or_else(|| cdk::Error::Custom("not used in this test".to_string()))?;
                let c = cdk::dhke::sign_message(key, &output.blinded_secret)
                    .map_err(|e| cdk::Error::Custom(format!("fake sign_message failed: {e}")))?;
                signatures.push(cdk::nuts::BlindSignature {
                    amount: output.amount,
                    keyset_id: output.keyset_id,
                    c,
                    dleq: None,
                });
            }
            Ok(cdk::nuts::MintResponse { signatures })
        }

        async fn post_melt_quote(
            &self,
            request: cdk::MeltQuoteRequest,
        ) -> Result<cdk::MeltQuoteCreateResponse<String>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn get_mint_quote_status(
            &self,
            method: cdk::nuts::PaymentMethod,
            quote_id: &str,
        ) -> Result<cdk::MintQuoteResponse<String>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn post_mint_quote_by_pubkey(
            &self,
            request: cdk::nuts::nutxx::MintQuoteByPubkeyRequest,
        ) -> Result<Vec<cdk::MintQuoteResponse<String>>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn get_melt_quote_status(
            &self,
            method: cdk::nuts::PaymentMethod,
            quote_id: &str,
        ) -> Result<cdk::MeltQuoteResponse<String>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn post_melt(
            &self,
            method: &cdk::nuts::PaymentMethod,
            request: cdk::nuts::MeltRequest<String>,
        ) -> Result<cdk::MeltQuoteResponse<String>, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn post_swap(
            &self,
            request: cdk::nuts::SwapRequest,
        ) -> Result<cdk::nuts::SwapResponse, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn get_mint_info(&self) -> Result<cdk::nuts::MintInfo, cdk::Error> {
            Ok(cdk::nuts::MintInfo::default())
        }

        async fn post_check_state(
            &self,
            request: cdk::nuts::CheckStateRequest,
        ) -> Result<cdk::nuts::CheckStateResponse, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn post_restore(
            &self,
            request: cdk::nuts::RestoreRequest,
        ) -> Result<cdk::nuts::RestoreResponse, cdk::Error> {
            Err(cdk::Error::Custom("not used in this test".to_string()))
        }

        async fn get_auth_wallet(&self) -> Option<cdk::wallet::AuthWallet> {
            None
        }

        async fn set_auth_wallet(&self, wallet: Option<cdk::wallet::AuthWallet>) {}
    }

    fn fake_keyset(unit: CurrencyUnit) -> (cdk::nuts::KeySetInfo, cdk::nuts::KeySet) {
        let keys = cdk::nuts::Keys::new(std::collections::BTreeMap::from([(
            Amount::from(1),
            SecretKey::generate().public_key(),
        )]));
        let id = cdk::nuts::Id::v1_from_keys(&keys);
        let info = cdk::nuts::KeySetInfo {
            id,
            unit: unit.clone(),
            active: true,
            input_fee_ppk: 0,
            final_expiry: None,
        };
        let full = cdk::nuts::KeySet {
            id,
            unit,
            active: Some(true),
            keys,
            input_fee_ppk: 0,
            final_expiry: None,
        };
        (info, full)
    }

    /// Like `fake_keyset`, but with a chosen `denomination` and its private
    /// key, for tests needing `post_batch_mint` to sign for real. Setting
    /// `denomination` to the quote amount keeps the greedy split to one output.
    fn fake_keyset_for_amount(
        unit: CurrencyUnit,
        denomination: Amount,
    ) -> (cdk::nuts::KeySetInfo, cdk::nuts::KeySet, SecretKey) {
        let secret_key = SecretKey::generate();
        let keys = cdk::nuts::Keys::new(std::collections::BTreeMap::from([(
            denomination,
            secret_key.public_key(),
        )]));
        let id = cdk::nuts::Id::v1_from_keys(&keys);
        let info = cdk::nuts::KeySetInfo {
            id,
            unit: unit.clone(),
            active: true,
            input_fee_ppk: 0,
            final_expiry: None,
        };
        let full = cdk::nuts::KeySet {
            id,
            unit,
            active: Some(true),
            keys,
            input_fee_ppk: 0,
            final_expiry: None,
        };
        (info, full, secret_key)
    }

    #[tokio::test]
    async fn mint_attempt_on_unit_with_uncached_keyset_needs_a_refresh() {
        use std::str::FromStr;

        let mint_url = cdk::mint_url::MintUrl::from_str("https://fake-mint.example.com").unwrap();
        let old_unit = unit("hash");
        let new_unit = unit("hash_pool_122");

        let (old_info, old_full) = fake_keyset(old_unit.clone());
        let connector = FakeMintConnector::new(vec![old_info], vec![old_full]);

        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());

        let wallet_a = cdk::wallet::WalletBuilder::new()
            .mint_url(mint_url.clone())
            .unit(old_unit)
            .localstore(localstore.clone())
            .seed([0u8; 64])
            .client(connector.clone())
            .build()
            .expect("wallet_a should build");

        wallet_a
            .keysets(KeysetLoadPolicy::Refresh)
            .await
            .expect("initial refresh should succeed");

        let (new_info, new_full) = fake_keyset(new_unit.clone());
        let new_id = new_full.id;
        connector.add_keyset(new_info, new_full).await;

        let wallet_b = cdk::wallet::WalletBuilder::new()
            .mint_url(mint_url)
            .unit(new_unit)
            .localstore(localstore)
            .seed([0u8; 64])
            .client(connector)
            .metadata_cache(wallet_a.metadata_cache.clone())
            .build()
            .expect("wallet_b should build");

        let before_refresh = wallet_b.keysets(KeysetLoadPolicy::CacheThenNetwork).await;
        assert!(
            matches!(before_refresh, Err(cdk::Error::UnknownKeySet)),
            "expected the stale-but-fresh cache to hide the new unit's keyset, got {before_refresh:?}"
        );

        ensure_keyset_cached(&wallet_b).await;

        let keysets = wallet_b
            .keysets(KeysetLoadPolicy::CacheThenNetwork)
            .await
            .expect("refreshing on first sight must make the new unit's keyset locally visible");
        assert!(
            keysets.iter().any(|k| k.id == new_id),
            "refreshed keysets did not include the new unit's keyset"
        );
    }

    #[tokio::test]
    async fn unknown_keyset_failure_wedges_the_quote_until_recovery_runs() {
        use std::str::FromStr;

        let mint_url = cdk::mint_url::MintUrl::from_str("https://fake-mint.example.com").unwrap();
        let old_unit = unit("hash");
        let new_unit = unit("hash_pool_122");

        let (old_info, old_full) = fake_keyset(old_unit.clone());
        let connector = FakeMintConnector::new(vec![old_info], vec![old_full]);

        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());

        let wallet_a = cdk::wallet::WalletBuilder::new()
            .mint_url(mint_url.clone())
            .unit(old_unit)
            .localstore(localstore.clone())
            .seed([0u8; 64])
            .client(connector.clone())
            .build()
            .expect("wallet_a should build");
        wallet_a
            .keysets(KeysetLoadPolicy::Refresh)
            .await
            .expect("initial refresh should succeed");

        let (new_info, new_full) = fake_keyset(new_unit.clone());
        connector.add_keyset(new_info, new_full).await;

        let wallet_b = cdk::wallet::WalletBuilder::new()
            .mint_url(mint_url)
            .unit(new_unit.clone())
            .localstore(localstore)
            .seed([0u8; 64])
            .client(connector)
            .metadata_cache(wallet_a.metadata_cache.clone())
            .build()
            .expect("wallet_b should build");

        let quote_id = "quote-1".to_string();
        let mut quote = cdk::wallet::MintQuote::new(
            quote_id.clone(),
            wallet_b.mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(Amount::from(10)),
            new_unit,
            "fake-request".to_string(),
            0,
            None,
        );
        quote.state = cdk::nuts::MintQuoteState::Paid;
        wallet_b
            .localstore
            .add_mint_quote(quote)
            .await
            .expect("seeding the quote should succeed");

        let first = wallet_b
            .batch_mint(&[quote_id.as_str()], SplitTarget::default(), None, None)
            .await;
        assert!(
            matches!(first, Err(cdk::Error::UnknownKeySet)),
            "expected the first attempt to fail on the uncached keyset, got {first:?}"
        );

        let stuck = wallet_b
            .localstore
            .get_mint_quote(&quote_id)
            .await
            .expect("lookup should succeed")
            .expect("quote should still be present");
        assert!(
            stuck.used_by_operation.is_some(),
            "expected the failed attempt to leave the quote reserved"
        );

        let second = wallet_b
            .batch_mint(&[quote_id.as_str()], SplitTarget::default(), None, None)
            .await;
        assert!(
            matches!(
                second,
                Err(cdk::Error::Database(cdk::cdk_database::Error::QuoteAlreadyInUse))
            ),
            "expected the second attempt to find the quote already reserved, got {second:?}"
        );
    }

    #[tokio::test]
    async fn leaked_reservation_recovers_and_mints_on_a_later_pass() {
        use std::str::FromStr;

        let mint_url = cdk::mint_url::MintUrl::from_str("https://fake-mint.example.com").unwrap();
        let old_unit = unit("hash");
        let new_unit = unit("hash_pool_122");
        let quote_amount = Amount::from(10);

        let (old_info, old_full) = fake_keyset(old_unit.clone());
        let connector = FakeMintConnector::new(vec![old_info], vec![old_full]);

        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());

        let wallet_a = cdk::wallet::WalletBuilder::new()
            .mint_url(mint_url.clone())
            .unit(old_unit)
            .localstore(localstore.clone())
            .seed([0u8; 64])
            .client(connector.clone())
            .build()
            .expect("wallet_a should build");
        wallet_a
            .keysets(KeysetLoadPolicy::Refresh)
            .await
            .expect("initial refresh should succeed");

        let (new_info, new_full, new_secret_key) =
            fake_keyset_for_amount(new_unit.clone(), quote_amount);
        let new_keyset_id = new_full.id;
        connector.add_keyset(new_info, new_full).await;
        connector.add_signing_key(new_keyset_id, new_secret_key).await;

        let wallet_b = cdk::wallet::WalletBuilder::new()
            .mint_url(mint_url)
            .unit(new_unit.clone())
            .localstore(localstore)
            .seed([0u8; 64])
            .client(connector)
            .metadata_cache(wallet_a.metadata_cache.clone())
            .build()
            .expect("wallet_b should build");

        let quote_id = "quote-1".to_string();
        let mut quote = cdk::wallet::MintQuote::new(
            quote_id.clone(),
            wallet_b.mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(quote_amount),
            new_unit,
            "fake-request".to_string(),
            0,
            None,
        );
        quote.state = cdk::nuts::MintQuoteState::Paid;
        wallet_b
            .localstore
            .add_mint_quote(quote)
            .await
            .expect("seeding the quote should succeed");

        let first = wallet_b
            .batch_mint(&[quote_id.as_str()], SplitTarget::default(), None, None)
            .await;
        assert!(
            matches!(first, Err(cdk::Error::UnknownKeySet)),
            "expected the first attempt to fail on the uncached keyset, got {first:?}"
        );

        let stuck = wallet_b
            .localstore
            .get_mint_quote(&quote_id)
            .await
            .expect("lookup should succeed")
            .expect("quote should still be present");
        assert!(
            stuck.used_by_operation.is_some(),
            "expected the failed attempt to leave the quote reserved"
        );

        let second = wallet_b
            .batch_mint(&[quote_id.as_str()], SplitTarget::default(), None, None)
            .await;
        assert!(
            matches!(
                second,
                Err(cdk::Error::Database(cdk::cdk_database::Error::QuoteAlreadyInUse))
            ),
            "expected the second attempt to find the quote already reserved, got {second:?}"
        );

        ensure_keyset_cached(&wallet_b).await;
        recover_leaked_reservations(&wallet_b).await;

        let recovered = wallet_b
            .localstore
            .get_mint_quote(&quote_id)
            .await
            .expect("lookup should succeed")
            .expect("quote should still be present");
        assert!(
            recovered.used_by_operation.is_none(),
            "recovery should have released the leaked reservation"
        );

        let proofs = wallet_b
            .batch_mint(&[quote_id.as_str()], SplitTarget::default(), None, None)
            .await
            .expect("the quote should be mintable once the leaked reservation is cleared");
        let minted: u64 = proofs.iter().map(|p| u64::from(p.amount)).sum();
        assert_eq!(minted, u64::from(quote_amount));
    }

    #[tokio::test]
    async fn leaked_reservation_on_the_base_unit_is_cleared_by_a_sweep_pass() {
        use std::str::FromStr;

        let mint_url = cdk::mint_url::MintUrl::from_str("https://fake-mint.example.com").unwrap();
        let base_unit = unit("hash");

        let (base_info, base_full) = fake_keyset(base_unit.clone());
        let connector = FakeMintConnector::new(vec![base_info], vec![base_full]);

        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());

        let base_wallet = cdk::wallet::WalletBuilder::new()
            .mint_url(mint_url.clone())
            .unit(base_unit.clone())
            .localstore(localstore.clone())
            .seed([0u8; 64])
            .client(connector)
            .build()
            .expect("base wallet should build");
        let base_wallet = Arc::new(base_wallet);

        let quote_id = "quote-1".to_string();
        let mut quote = cdk::wallet::MintQuote::new(
            quote_id.clone(),
            base_wallet.mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(Amount::from(10)),
            base_unit,
            "fake-request".to_string(),
            0,
            None,
        );
        quote.state = cdk::nuts::MintQuoteState::Paid;
        base_wallet
            .localstore
            .add_mint_quote(quote)
            .await
            .expect("seeding the quote should succeed");

        // A reservation with no backing saga -- the orphan shape a crashed or
        // failed mint attempt leaves behind -- created directly rather than
        // via a failed batch_mint, since only the leak's presence matters here.
        let leaked_operation_id = uuid::Uuid::new_v4();
        base_wallet
            .localstore
            .reserve_mint_quote(&quote_id, &leaked_operation_id)
            .await
            .expect("reserving the quote should succeed");
        let leaked = base_wallet
            .localstore
            .get_mint_quote(&quote_id)
            .await
            .expect("lookup should succeed")
            .expect("quote should still be present");
        assert!(
            leaked.used_by_operation.is_some(),
            "expected the seeded reservation to be in place before the sweep"
        );

        let factory = WalletFactory::for_test(mint_url, [0u8; 64], localstore, base_wallet.clone());
        let wallets: Mutex<HashMap<CurrencyUnit, Arc<Wallet>>> = Mutex::new(HashMap::new());
        let locking_privkey = SecretKey::generate().to_secret_hex();

        process_stored_quotes(&factory, &wallets, Some(&locking_privkey))
            .await
            .expect("a sweep pass should not error");

        let recovered = base_wallet
            .localstore
            .get_mint_quote(&quote_id)
            .await
            .expect("lookup should succeed")
            .expect("quote should still be present");
        assert!(
            recovered.used_by_operation.is_none(),
            "a sweep pass must clear a leaked reservation on the base unit, \
             not just on other units"
        );
    }

    #[tokio::test]
    async fn failed_keyset_refresh_skips_the_unit_and_reserves_nothing() {
        use std::str::FromStr;

        let mint_url = cdk::mint_url::MintUrl::from_str("https://fake-mint.example.com").unwrap();
        let new_unit = unit("hash_pool_122");

        let connector = FakeMintConnector::new(Vec::new(), Vec::new());
        let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());

        let wallet = cdk::wallet::WalletBuilder::new()
            .mint_url(mint_url)
            .unit(new_unit.clone())
            .localstore(localstore)
            .seed([0u8; 64])
            .client(connector)
            .build()
            .expect("wallet should build");

        let quote_id = "quote-1".to_string();
        let mut quote = cdk::wallet::MintQuote::new(
            quote_id.clone(),
            wallet.mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(Amount::from(10)),
            new_unit,
            "fake-request".to_string(),
            0,
            None,
        );
        quote.state = cdk::nuts::MintQuoteState::Paid;
        wallet
            .localstore
            .add_mint_quote(quote)
            .await
            .expect("seeding the quote should succeed");

        // Mirrors process_stored_quotes's guard: mint only when the refresh succeeded.
        if ensure_keyset_cached(&wallet).await {
            let _ = wallet
                .batch_mint(&[quote_id.as_str()], SplitTarget::default(), None, None)
                .await;
        }

        let after = wallet
            .localstore
            .get_mint_quote(&quote_id)
            .await
            .expect("lookup should succeed")
            .expect("quote should still be present");
        assert!(
            after.used_by_operation.is_none(),
            "a unit skipped for a failed refresh must leave no new reservation"
        );
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
