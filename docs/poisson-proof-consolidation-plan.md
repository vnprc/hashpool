# Poisson Proof Consolidation Plan

## Problem

The translator's faucet wallet accumulates Cashu proofs continuously as mining shares are paid out. Each share produces one or more proofs in the wallet. Without active maintenance, the proof count grows unboundedly. Beyond a certain count, any swap operation that sends all proofs to the mint at once will exceed CDK's hard input limit (1000 proofs per swap request), causing the swap to fail and leaving proofs stuck in `RESERVED` state.

The naive solution — consolidate all proofs on every polling loop — creates a worse problem: a perfectly regular consolidation pattern that the mint can trivially use to deanonymize the wallet's activity history. This plan describes a background consolidation strategy that keeps the wallet healthy while preserving meaningful privacy.

---

## Background: What the Mint Can Observe

Every swap is fully visible to the mint operator:
- The exact set of input proofs (by their cryptographic `Y` values)
- The exact set of output proofs created (by amount, though not their future owners)
- Timestamps of all operations
- The creation timestamps of each input proof (from prior mint operations)

Cashu's blinding prevents linking a proof to the user who holds it. It does **not** prevent temporal correlation or batch fingerprinting by the mint. Since the hashpool mint issues all proofs, it knows when every proof was created and can observe when they are consumed.

---

## Privacy Analysis of Consolidation Strategies

### Regular interval consolidation (worst case)

Consolidating on a fixed schedule (e.g., every 30 seconds or every 30 minutes) creates a predictable signature. The mint can observe the rhythm and map each batch to exactly one period of mining activity. All proofs created between time T and T+interval arrive at the mint in a single swap, perfectly linking them.

### Fixed threshold consolidation

Consolidating when the proof count exceeds a threshold (e.g., 500) is slightly better than fixed timing, but the threshold combined with a known hashrate makes consolidation timing predictable: `consolidation_time ≈ threshold / shares_per_hour`.

### Uniform random interval

Consolidating at a uniformly-random interval (e.g., every 15–45 minutes) has predictable second-order statistics — mean and variance are stable over time. A determined observer can fit the distribution given enough samples and use it for fingerprinting.

### Poisson process (exponentially-distributed intervals)

A Poisson process is the correct model for "roughly periodic but unpredictable" events. Its key property is **memorylessness**: the probability of consolidation in the next minute is independent of how long it has been since the last consolidation. This makes it much harder to fingerprint than any fixed or uniform-random schedule.

The exponential distribution is sampled as: `interval_secs = -ln(U) × mean_secs`, where U is a uniform random variable in (0, 1). This produces intervals that cluster around the mean but occasionally produce very short or very long gaps, which is exactly the right noise profile.

### Random subset selection

Even with Poisson timing, consolidating all proofs at once still reveals that "every proof in the wallet from the past N weeks was created by this entity." Random subset selection — taking a randomly-sized sample of randomly-chosen proofs — breaks temporal correlation within batches. Proofs from last hour mix with proofs from last week. The mint cannot reconstruct which proofs were created together by observing which ones are consolidated together.

---

## Design

### Overview

A background task runs alongside the existing quote sweeper. It wakes at Poisson-distributed intervals, checks whether the proof count exceeds a low-watermark threshold, and if so, selects a random subset of proofs for consolidation. The consolidation is a swap with `amount = None`, meaning all outputs are returned to the wallet as unspent change proofs — no token is sent to any user.

### New configuration parameters

Add to `TranslatorConfig` with `#[serde(default)]` so existing config files require no changes:

| Field | Type | Default | Description |
|---|---|---|---|
| `consolidation_enabled` | `bool` | `true` | Enable or disable the consolidation task |
| `consolidation_mean_interval_secs` | `u64` | `1800` | Mean of the exponential distribution (30 minutes) |
| `consolidation_low_watermark` | `usize` | `100` | Minimum proof count required to trigger consolidation |
| `consolidation_min_batch` | `usize` | `50` | Minimum proofs per consolidation batch |
| `consolidation_max_batch` | `usize` | `1000` | Maximum proofs per consolidation batch (matches CDK's hard input limit) |

The default of 1000 matches CDK's hard limit exactly. The consolidation task has deterministic control over how many proofs it passes to `wallet.swap()` — CDK adds no additional inputs between selection and the mint request — so there is no buffer needed. The mint's rejection condition is `inputs.len() > 1000`, so exactly 1000 inputs is accepted.

### Task: `spawn_proof_consolidator`

New method on `TranslatorSv2`, follows the same pattern as `spawn_quote_sweeper`. Spawned in `start()` when `wallet.is_some()` and `consolidation_enabled` is true.

```
fn spawn_proof_consolidator(
    &self,
    task_manager: &Arc<TaskManager>,
    wallet: Arc<Wallet>,
)
```

**Loop logic:**

```
loop:
    // Sample interval from exponential distribution (Poisson process)
    u = rand::random::<f64>().clamp(f64::EPSILON, 1.0)   // avoid ln(0)
    interval_secs = (-u.ln() * mean_interval_secs as f64) as u64
    sleep(Duration::from_secs(interval_secs))

    proofs = wallet.get_unspent_proofs().await?

    if proofs.len() < consolidation_low_watermark:
        continue

    // Random batch size
    upper = proofs.len().min(consolidation_max_batch)
    n = rand_range(consolidation_min_batch, upper)

    // Random proof selection: shuffle, take first n
    proofs.shuffle(&mut rng)
    selected = proofs[..n]

    // Sign any P2PK-locked proofs (minted proofs require signature to spend)
    selected = sign_p2pk_proofs(selected, locking_privkey)

    // Consolidate: amount=None means all outputs become wallet change (no send token)
    match wallet.swap(None, SplitTarget::default(), selected, None, false, false).await:
        Ok(_)  => log success, log new proof count
        Err(e) => log error, continue (non-fatal)
```

### Why `amount = None` for consolidation

`swap(None, ...)` instructs CDK to take the input proofs and return all output proofs as wallet change (stored as `UNSPENT`). No send token is created, no proofs are reserved for external transfer. This is purely internal wallet maintenance. The wallet's total balance is unchanged (minus any per-proof swap fees charged by the mint).

### Spending conditions on consolidation outputs

Input proofs are P2PK-locked (they were minted with `SpendingConditions::new_p2pk`) and must be signed before spending. Consolidation outputs do **not** need P2PK conditions. The P2PK on mint outputs serves a specific purpose: preventing value extraction during the window between issuance and wallet receipt. Once proofs are in the wallet database, the wallet's own seed-derived key security is sufficient. Removing P2PK from consolidation outputs simplifies subsequent operations on those proofs.

This means the `spending_conditions` parameter to `swap()` is `None` for consolidation calls. Only the input signing step (signing already-locked proofs before sending them as inputs) is needed.

### Interaction with the quote sweeper

The quote sweeper (`spawn_quote_sweeper`) runs on a fixed 15-second loop and mints new proofs from completed share quotes. The consolidation task is completely independent: different timing mechanism, different wallet operation (mint vs. swap), different purpose. They share the same `Arc<Wallet>` handle; CDK's saga pattern handles concurrent access safely via proof reservation.

One emergent interaction worth noting: the quote sweeper continuously adds new proofs to the wallet. This means the consolidation task will sometimes observe a higher proof count than at the end of the last consolidation, which is expected and fine. The low-watermark check prevents consolidation from running when the wallet is nearly empty (e.g., shortly after a large consolidation batch).

---

## Tradeoffs and Limitations

### Swap fees

CDK charges a per-proof fee on swap inputs. Every consolidation batch incurs this cost. The fee is small per proof but nonzero. The `consolidation_min_batch` and `consolidation_low_watermark` parameters exist partly to ensure consolidation only runs when there's enough material to justify the fee.

### Anonymity set

The consolidation privacy properties described above assume multiple wallets use the same mint and unit. In the current production setup, the hashpool faucet wallet is the only entity swapping `hash`-denomination proofs at this mint. The mint knows every proof it ever issued in this unit, so all consolidation operations are trivially linkable to the same wallet regardless of timing strategy. The Poisson + random-subset approach is forward-looking: it becomes meaningfully privacy-preserving as the ecosystem grows and the anonymity set expands.

### CDK input limit

CDK's mint enforces a hard limit of 1000 inputs per swap request. The `consolidation_max_batch` default of 1000 matches this limit exactly. The consolidation task has deterministic control over how many proofs it passes to `wallet.swap()`, so no buffer is needed.

---

## Implementation Checklist

- [ ] Add 5 new `#[serde(default)]` fields to `TranslatorConfig` in `config.rs`
- [ ] Add `default_*` functions for each new field following the existing pattern
- [ ] Add `rand` usage for exponential sampling and shuffle (check if `rand` is already in `translator/Cargo.toml`; if not, add it)
- [ ] Implement `spawn_proof_consolidator` in `mod.rs`
- [ ] Call `spawn_proof_consolidator` from `start()` alongside `spawn_quote_sweeper`
- [ ] Test: verify task starts, logs correctly, skips when below watermark
- [ ] Test: verify proof count decreases after consolidation cycle
- [ ] Test: verify non-fatal behavior when swap fails (error logged, task continues)
