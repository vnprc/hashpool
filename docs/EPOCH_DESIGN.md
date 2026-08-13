# Mining Epoch Design

Hashpool issues ehash for mining shares, and the value of that ehash is scoped to a mining
epoch: the period between block rewards landing at the mint. This document is the canonical
design for epoch mechanics — how epochs are named, opened, and closed, how the mint detects a
block reward, how quotes and tokens behave across an epoch boundary, and how blockchain reorgs
are resolved correctly. It supersedes two decisions in `SETTLEMENT_DESIGN.md` (marked there):
reward detection moves from a pool-sent message to mint-side chain observation, and epoch
creation moves from pool-requested to mint-owned. Settlement itself — redeeming old-epoch
ehash for bitcoin — is a later milestone and is unchanged by this document.

## The model

An **epoch** is the period between consecutive block rewards arriving at the mint's receive
address. Each epoch has its own currency unit and its own keyset. Shares mined during an epoch
become quotes stamped with that epoch's unit; quotes sweep into ehash tokens under that
epoch's keyset. When the next reward lands, the epoch closes and a new one opens.

- **Naming:** the unit is `hash_<pool>_<height>` — `<pool>` is the pool's compressed public
  key as lowercase hex, `<height>` the height of the block whose coinbase paid the mint and
  thereby opened the epoch. The pool key namespaces epochs so one mint can serve several
  pools and traders can compare ehash across pools; a pubkey rather than a name keeps the
  anonymity set maximal and becomes self-authenticating when authenticated asset creation
  arrives. The full key is used — a truncated prefix could be ground into a collision exactly
  where assets are named. Height stays the human-readable tail: sortable, on-chain
  verifiable, and unambiguous under the one-epoch-per-height rule below.
- **The mint owns epoch identity.** The pool is oblivious: it keeps sending quote requests
  naming `HASH`, and the mint stamps the current epoch's unit at quote creation. Nothing
  pool-side changes when an epoch rolls. (In a future proxy-pool deployment where the reward
  arrives over lightning, the payer supplies the block height and a pool identity pubkey;
  the trigger below is an abstraction so that case slots in.)
- **Genesis:** on first boot the mint records the current chain height and opens the first
  epoch at it, with no backing reward. The genesis epoch is final immediately. Restarts reuse
  the persisted current epoch; genesis never re-runs.
- **One epoch per (pool, height), ever.** A duplicate reward at an already-used height (see
  Reorgs) updates the existing epoch's record; it never opens a second epoch. Epochs are
  never reopened once final.

## Quote states are the safety mechanism

Hashpool has exactly one irreversible act: minting a quote into ecash. Tokens are blinded
bearer instruments — once issued they can never be reassigned, re-denominated, or clawed
back. Quotes, by contrast, are rows in the mint's database and can be re-stamped freely. The
entire reorg design reduces to one rule: **a quote may not become a token until its epoch
boundary is beyond plausible reorg.**

The gate is the standard cashu quote state machine — no custom guards:

| state | meaning here | enforced by |
|---|---|---|
| **unpaid** | share accepted, epoch boundary not yet confirmed | existing machinery: unpaid quotes are excluded from the pubkey lookup's mintable filter and rejected by the mint's state validation (checked twice, re-validated inside the DB transaction) |
| **paid** | the work's epoch context is settled; sweepable | the mint's existing pay call, invoked per quote; idempotent by payment id (the share's header hash), so replays and crash-retries are no-ops |
| **issued** | swept into ehash tokens | standard |

The share is still the payment — it just *clears* when the chain settles, exactly as a
lightning quote clears when the invoice is paid. Concretely:

- Quotes created while the current epoch is **final** are paid at creation (today's exact
  behavior).
- Quotes created while the current epoch is **provisional** (boundary not yet confirmed) are
  created **unpaid**. They are invisible to wallets — the sweep's `only_mintable` filter is
  `amount_paid > amount_issued` in SQL — and unmintable at the deepest layer.
- When the boundary reaches finality, the mint bulk-pays the epoch's quotes and they appear
  for sweeping for the first time.

Quote expiry is advisory at the pinned CDK revision (set at creation, not enforced at pay or
mint time), but the ehash quote TTL should still be configured well past the confirmation
window in case upstream starts enforcing it.

This deferred-pay hook is also where block template validation will plug in later (issue
#20): pay = epoch boundary final AND template valid. One lever, two gates, no rework.

## Reward detection: the mint watches the chain

The mint runs a thin scanner against its bitcoin node's RPC (reusing the existing
`roles/roles-utils/rpc` client): on each new block, scan the coinbase outputs for the mint's
configured receive script. Detection compares script pubkeys, not address strings.

Why detection lives at the mint and not the pool:

1. **The pool doesn't always know.** When someone else builds the template — job declaration
   today, a template-producing proxy tomorrow — the pool never submits the block and never
   learns it won. The chain always knows.
2. **The pool never gets confirmation.** It fires the solution at the template provider and
   hears nothing back; it cannot distinguish "found" from "accepted".
3. **A message can be lost; a payment cannot.** A notification sent while the mint is down is
   gone. A reward sitting in a block waits for the mint to come back and re-scan.

A pool-sent `BlockFound` message remains a possible future *optimization* (announce, then
verify on-chain) — it is no longer the trigger.

Why not the CDK on-chain backend (`cdk-bdk`): its detection is bound to the quote lifecycle
(it only watches addresses handed out by quote requests), it captures but does not surface
the funding block height that epochs are named by, and it has no coinbase awareness. The
mint's receive *custody* can move to a CDK-managed wallet in the settlement milestone; the
trigger only needs a script to watch.

The trigger is an abstraction — a reward-received event carrying height, amount, and source —
with one source implemented now (on-chain coinbase) and lightning later.

**Watcher state and recovery:** the watcher persists the last processed block and the last
~D block hashes. On restart it re-scans forward from the watermark; rewards seen during
downtime open and close epochs in sequence (intermediate epochs may be empty — that is fine).
Reward events deduplicate by height. The bitcoin node keeps the fork tree; the mint keeps an
epoch list, a provisional flag, a confirmation counter, and a few recent hashes.

## Reorgs: provisional boundaries

A reward at height H closes the current epoch and opens `hash_H` **provisionally** — new
quotes stamp into `hash_H` immediately, but unpaid. Rotating immediately matters: if rotation
waited for confirmations, post-win shares would stamp into the old epoch and dilute it, and
some could be swept before any re-stamp was possible.

- **Finality:** when the boundary has **D confirmations** (config: low on regtest for tests,
  conventionally 6+ in production), the epoch becomes final. The mint bulk-pays its quotes,
  and retires the *previous* epoch's payment-processor registration and mint-info entry (old
  quotes still mint — those settings gate only quote creation — but no new quotes can be
  created for old epochs, and the mint stays bounded as epochs accumulate). Miner experience:
  shares mined after a block win mature for D blocks, like every pool's immature-block
  window; the coinbase itself is locked for 100 blocks, so nothing spendable is delayed.
- **Dissolve:** if the boundary's block is orphaned before finality and the replacement chain
  does not pay the mint, the boundary dissolves. The mint re-stamps the epoch's quotes —
  provably never paid, never minted — back to the previous epoch **and pays them** (that
  epoch is settled; the work is good), splices the epoch record out, and the previous epoch
  resumes as current. This is the ecash-native version of orphaned shares rolling into the
  next round. Before dissolving, assert the epoch's keyset issued nothing; a nonzero issue
  count is a critical invariant violation.
- **Same-height re-mine:** if a reorg replaces the boundary block with a different block at
  the same height that also pays the mint, that is the same boundary re-mined: keep the
  epoch, swap the recorded block hash, restart the confirmation clock. Sound because a
  provisional epoch has signed nothing.
- **Residual risk:** a reorg deeper than D after tokens exist is accepted, exactly as every
  mining pool accepts it when choosing a confirmation depth. D buys the risk down; it cannot
  eliminate it. Settlement (next milestone) additionally gates on full 100-block coinbase
  maturity.

## Epoch lifecycle sequence

1. Watcher sees a coinbase output paying the mint's script at height H.
2. Close the current epoch; open `hash_H` provisionally: register the unit's payment
   processor entry, create its keyset, write the epoch record (height, block hash, reward
   amount, state = provisional).
3. New quote requests stamp `hash_H`, created unpaid. Old-epoch quotes remain paid and
   sweepable throughout.
4. At H+D (boundary still canonical): mark final; bulk-pay `hash_H` quotes; retire the
   previous epoch's registration.
5. On reorg before H+D: dissolve or re-mine handling as above.

## Proxy wallet behavior

The proxy's wallet follows epochs with no reorg awareness at all — unpaid quotes never reach
it, so re-stamps happen before any wallet has seen the quote.

- **Discovery:** the mint-quote pubkey lookup is unit-blind end to end (verified at the
  pinned revision: no unit in the wire format, no unit predicate in the mint's query, and the
  wallet stores fetched quotes with their true unit). One wallet discovers every epoch.
- **Execution:** a CDK wallet only mints the unit it was constructed with, so the sweeper
  derives the unit set *from the fetched quotes* — never from mint info, which is exactly
  what lets the mint retire old entries — and lazily creates a cheap per-unit wallet handle.
  Handles share the one localstore, one seed, and one keyset metadata cache; constructing one
  writes nothing. Verified safe: schema and keyset counters are unit- or keyset-scoped.
- **Housekeeping:** drop handles for units with nothing left to mint; cap batch size per
  sweep pass (the first pass after a rotation drains a backlog); log the reconcile per unit —
  "fetched but never minted" divergence is the standing detection signal for unit-handling
  bugs.
- Faucet and web UI must not break with multiple units present; epoch-aware display is
  explicitly out of scope for this milestone.

## Required CDK change

Opening a unit at runtime needs one upstream change: registering (and retiring) payment
processors on a running mint. Today the processor map, the per-processor payment-event
consumer tasks, and the stored mint-info settings that gate quote creation are all fixed at
construction. The proposed API and implementation sketch live in
`docs/pr/cdk-dynamic-payment-processors.md` and will be filed upstream; hashpool runs the
fork commit until it merges.

Facts verified at the pinned revision that this design relies on:

- `Mint::rotate_keyset` already handles a brand-new unit (fresh derivation index, collision
  check); the amounts vector must be non-empty for a new unit.
- HTTP routes are method-scoped, not unit-scoped — the `ehash` method is already mounted, so
  new units need no router changes.
- Unit names hash into a 31-bit derivation-index space with a collision check at creation.
  Collisions are vanishingly rare at production epoch cadence but the unit name is fixed by
  pool and height, so on `UnitStringCollision` the mint appends a deterministic suffix
  (`hash_<pool>_<height>_1`, `_2`, …) rather than failing.

## Configuration

| knob | meaning | dev default | production guidance |
|---|---|---|---|
| mint receive script | coinbase script the watcher matches | address from the regtest harness wallet | operator-supplied; custody design arrives with settlement |
| pool identity | compressed pubkey namespacing the unit (`hash_<pool>_<height>`); paired with the receive script in config — over lightning the payer will supply it | generated dev keypair (not the miner locking key) | the pool's published identity key |
| confirmation depth D | boundary finality | 1–3 (tests force reorgs) | 6+ |
| poll interval | watcher RPC cadence | seconds | seconds; epochs are hours-days |
| ehash quote TTL | must comfortably exceed D | days | days |

## Out of scope (this milestone)

Settlement and redemption of old-epoch ehash; old-unit end-of-life and redemption windows;
epoch-aware wallet and UI treatment; authenticated quote and asset creation; the pool→mint
`BlockFound` announcement; embedding the keyset id in the coinbase; block template validation
(issue #20 — its result will gate the same pay-at-finality hook built here). Old units
simply sit,
mintable for their outstanding quotes and swappable within their own unit, until settlement
arrives.

## Test plan (all regtest)

Regtest reality: every share is a block (~2/second with the stock CPU miner), so blocks are
uncontrollable but *rewards to the mint* are fully controllable — the dev default points the
pool coinbase away from the mint, and tests create reward events deliberately. Signet was
evaluated and rejected: pool-built coinbases can never be valid signet blocks, so it removes
the one path regtest uniquely tests, while adding signer infrastructure.

1. **Deterministic trigger:** node + mint only; `generatetoaddress 1 <mint address>`; assert
   exactly one rotation, correct unit name, epoch record written, quotes flip paid at D.
2. **Full-stack:** point the pool coinbase at the mint address, run the miner briefly; the
   pool's own winning block rotates the epoch; assert monotonic epoch heights.
3. **Catch-up:** stop the mint, mine several rewards, restart; epochs open and close in
   sequence from the watermark.
4. **Dissolve:** with D=3, mine a reward, then invalidate the block and extend the other
   branch without paying the mint; assert quotes re-stamped to the previous epoch and paid,
   epoch record spliced.
5. **Same-height re-mine:** invalidate and re-mine a paying block at the same height; assert
   the epoch is reused with the new block hash and a restarted confirmation clock.

Reward-amount assertions compute the subsidy from height (it halves every 150 regtest
blocks). The dev config's `minimum_difficulty = 0` makes ehash amounts explode
(`2^leading_zeros`); accounting-flavored assertions should set a sane minimum difficulty.
