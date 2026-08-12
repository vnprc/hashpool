# CDK: register and retire payment processors on a running mint

Upstream-facing design for a cashubtc/cdk change. This file is written to be posted as a CDK
issue for discussion before the PR; it assumes no hashpool context beyond the motivation
paragraph. Verified against cdk at the revision hashpool pins; every touch point named below
is byte-identical to current upstream main at the time of writing.

## Motivation

A mint today declares its currency units and payment backends once, at `MintBuilder` time.
Nothing can be added or removed while the mint runs. Any mint whose unit set is dynamic must
restart to change it.

The motivating use case: hashpool issues ehash (mining-share ecash) under a new currency unit
per mining epoch — a new unit every time the pool wins a block reward. Restarting the mint per
epoch is not viable. The general capability is broader: hot-adding a lightning backend,
retiring a deprecated unit, or rotating backend infrastructure without downtime.

## Current constraints (all verified)

1. **The processor map is frozen.** `Mint.payment_processors` is
   `Arc<HashMap<PaymentProcessorKey, DynMintPayment>>` (`crates/cdk/src/mint/mod.rs`), built
   in the constructors and never mutated. Lookup is an exact `(unit, method)` key.
2. **Payment-event consumers fan out once.** `wait_for_paid_invoices` iterates the map at
   `Mint::start`, spawns one consumer task per unique processor instance (deduped by
   `Arc::ptr_eq`), and never re-reads the map. A processor added later would get no `start()`
   call and no `wait_payment_event` consumer.
3. **Stored mint info is the real quote gate.** `check_mint_request_acceptable` resolves
   NUT-04/NUT-05 settings from the DB-backed `MintInfo` (`nut04.get_settings(unit, method)` →
   `UnsupportedUnit`), and those settings are written only by `MintBuilder::add_payment_processor`.
   Registering into the map without updating stored mint info still refuses quotes.
4. **The pubsub spec snapshots the map.** `PubSubManager::new` captures a clone of the `Arc`
   at construction; subscriptions on a later-added unit would fail processor lookup.

## Proposed API

```rust
impl Mint {
    /// Register a payment processor for (unit, method) on a running mint.
    /// Starts the processor if it is not already running under another key,
    /// spawns its payment-event consumer, and updates stored mint info
    /// (NUT-04/NUT-05 method settings from `limits` and the processor's
    /// reported settings).
    pub async fn register_payment_processor(
        &self,
        unit: CurrencyUnit,
        method: PaymentMethod,
        limits: MintMeltLimits,
        processor: DynMintPayment,
    ) -> Result<(), Error>;

    /// Remove a (unit, method) registration. Removes the NUT-04/NUT-05
    /// entries so no new quotes can be created; existing quotes are
    /// unaffected (their lifecycle does not consult these settings).
    /// The processor is stopped only when no other key references it.
    pub async fn deregister_payment_processor(
        &self,
        unit: CurrencyUnit,
        method: PaymentMethod,
    ) -> Result<(), Error>;
}
```

Semantics:

- Duplicate `(unit, method)` registration is rejected, mirroring the builder.
- One processor instance may serve many keys; `Arc::ptr_eq` dedup governs `start()`/`stop()`
  and consumer spawning, exactly as it does today at boot.
- Registration is a two-phase update (map + stored mint info). Order: update stored mint info
  last, since it is the gate that makes the unit quotable; a crash between phases leaves an
  inert map entry, not a quotable unit without a processor.
- `deregister` uses the existing `nut04::Settings::remove_settings` (and the NUT-05 twin).

## Implementation sketch

- **Map:** replace the frozen `Arc<HashMap<..>>` with `Arc<ArcSwap<HashMap<..>>>` plus a
  companion `Mutex<()>` serializing read-modify-write. This is the established in-file
  pattern: `Mint.keysets` is `Arc<ArcSwap<Vec<SignatoryKeySet>>>` with `keyset_store_lock`.
  `ArcSwap` keeps `get_payment_processor` sync. Roughly a dozen read sites adapt mechanically
  with `.load()` (constructors, start/stop loops, lookup, `get_custom_payment_methods`,
  `check_mint_quote_payments`, melt paths, startup check).
- **Consumer supervision (the substantive change):** `wait_for_paid_invoices` gains a channel
  arm in its existing `select!` loop — an `mpsc::Receiver<DynMintPayment>` — and spawns a
  consumer for each received processor. The existing `is_payment_event_stream_active` guard
  and `Arc::ptr_eq` dedup make late spawns idempotent.
- **Pubsub:** `MintPubSubSpec` holds the swappable handle instead of a snapshot (its `Context`
  type and `new_instance` change accordingly).
- **Settings construction:** extract the per-method `MintMethodSettings`/`MeltMethodSettings`
  construction out of `MintBuilder::add_payment_processor` into a shared helper so the
  builder path and the runtime path cannot drift (including the onchain min-amount clamping).
- **No changes needed:** HTTP routing (`cdk-axum` custom routes are method-scoped, not
  unit-scoped); the signatory (`rotate_keyset` already creates keysets for brand-new units —
  callers must pass a non-empty amounts vector for a unit with no prior keyset); swap/melt
  verification (unit is derived per-proof from the keyset, no allowlist).

## Non-goals

- Hot-swapping the processor behind an existing live `(unit, method)` key (deregister +
  register is explicit and sufficient).
- Authorization of who may call these methods — the embedding application's concern; the API
  is on `Mint`, not on any transport.
- NUT-17 websocket advertisement for late-added units (can follow; the initial PR documents
  the gap).

## Testing

- Register a custom-method processor on a started mint; create and pay a quote in the new
  unit; mint against it.
- Deregister; assert new quote creation fails with `UnsupportedUnit` while an existing unpaid
  quote can still be paid and minted.
- Register the same processor instance under a second unit; assert single `start()`, single
  consumer, and that deregistering one key leaves the other functional.
- Crash-shaped test: registration interrupted between map and mint-info phases leaves the
  mint refusing quotes for the unit (fail-closed).
