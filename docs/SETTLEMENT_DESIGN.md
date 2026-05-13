# Hashpool Settlement Design

## Overview

Hashpool issues ehash tokens for each accepted mining share. When the pool finds a block, the
current mining epoch closes and ehash holders can settle their tokens for BTC. This document
defines the settlement architecture: how miners choose between on-chain coinbase payouts and
ecash redemption, how keyset rotation works at epoch boundaries, and the quote-based mechanism
that ties it all together.

## Core Concepts

### Mining Epoch

An epoch is the period between consecutive blocks found by the pool. Each epoch has:
- A unique ehash currency unit (e.g., `HASH_epoch_42`)
- A corresponding CDK keyset that issues tokens for that epoch
- A set of accumulating melt quotes from miners who opted into on-chain payouts

When the pool finds a block, the current epoch closes: the keyset rotates, quotes settle, and
a new epoch begins.

### Two Payout Paths

**Ecash path (default):** Miner holds ehash tokens as bearer ecash. When the epoch closes,
tokens become redeemable at the mint for BTC-backed ecash. The miner controls bearer tokens
throughout. Standard Cashu redemption.

**On-chain path (opt-in):** Miner creates an accumulating melt quote specifying a Bitcoin
payout address. Over the course of the epoch, the miner burns ehash tokens into this quote.
If the address appears in the coinbase when the pool finds a block, the quote is marked PAID.
If not, the quote falls back to ecash.

### Why Quote-Based Settlement

Earlier designs explored NUT-11 P2PK locking and 2-of-2 multisig custody between the pool and
miner. These were rejected because:

1. **Cashu blinding prevents mint visibility.** The mint cannot see P2PK locking conditions
   until token redemption. This makes proactive token invalidation impossible.

2. **Double-spend requires active prevention.** Any design where tokens remain in circulation
   while a parallel on-chain payout is committed creates a double-spend vector.

3. **Burning tokens at contribution time eliminates double-spend by construction.** Once tokens
   are burned into a quote, they cannot be redeemed elsewhere. The mint holds a clean obligation
   that settles via one of two paths: on-chain coinbase payout or ecash fallback (with
   additional paths possible as more payment methods are added).

## Required Mint Capabilities

Two new capabilities are required at the CDK mint layer before the settlement mechanism can
function. These were identified in `docs/EHASH_PROTOCOL.md` and are prerequisites for
everything else in this design.

### Authenticated Asset Creation

When a new mining epoch begins, the pool must create a new ehash currency unit and
corresponding keyset at the mint. Only an authorized pool should be able to do this.

- The pool authenticates with the mint as an **ehash issuer** (via the existing SV2 Noise
  handshake or a separate authentication mechanism).
- The pool sends an authenticated request to create a new asset (currency unit + keyset)
  for the new epoch.
- To support multiple pools on a single mint, currency units should include the pool
  identifier (e.g., `HASH_poolname_epoch_42`). Pools may only create assets under their
  own namespace.
- This capability must be implemented in **cdk-ehash**.

### Authenticated Quote Creation

When the pool receives a valid share, it creates a mint quote at the mint so the miner can
receive ehash tokens. Only authorized pools should be able to create these quotes.

- The pool submits an authenticated quote creation request to the mint.
- Since the pool validates shares (proof of work), quotes are created in **PAID** status —
  the share IS the payment.
- This is the existing `MintQuoteRequest` flow, but it needs authentication enforcement
  so arbitrary parties cannot mint ehash tokens.
- The SV2 Noise connection already provides an authenticated channel. The mint must verify
  that the connected pool is authorized to create quotes for the requested currency unit.
- This capability must be implemented in **cdk-ehash**.

### Interaction with Keyset Rotation

At epoch close (block found):
1. Pool stops creating quotes for the closing epoch's currency unit
2. Pool sends authenticated request to create a new asset for the next epoch
3. Mint creates new keyset, begins accepting quotes for the new currency unit
4. Old keyset tokens are no longer tradeable or redeemable for mint quotes. They can only
   be redeemed for bitcoin payouts (ecash, lightning, or on-chain if the mint supports it)
   for a limited time before the redemption window closes.

These two capabilities are the foundation that the settlement mechanism builds on.

---

## Settlement Mechanism

### Accumulating Melt Quote

A new quote type that starts with zero balance and grows as tokens are contributed.

**Quote creation:**
```
Miner -> Mint: {
    payout_address: "bc1q...",
    ehash_unit: "HASH_epoch_42",
    fallback_method: "ecash"
}

Mint -> Miner: {
    quote_id: "abc123",
    state: "ACCUMULATING",
    amount_accumulated: 0
}
```

**Token contribution:**
```
Miner -> Mint: {
    quote_id: "abc123",
    inputs: [ehash_proofs]
}

Mint -> Miner: {
    quote_id: "abc123",
    amount_accumulated: 1500
}
```

The mint verifies the proofs, burns the tokens, and increments the quote's accumulated balance.
Contributions can happen at any frequency — every share, in batches, or however the miner
prefers.

**State machine:**
```
CREATED --> ACCUMULATING --> PAID                (address in coinbase, verified on-chain)
                         \-> FALLBACK --> SETTLED (ecash issued to miner)
```

### Epoch Close and Settlement

When the pool finds a block:

1. Pool sends `BlockFound` to mint (via SV2 connection)
2. Mint rotates keyset: closes current epoch's keyset, opens new one
3. Mint checks blockchain: which payout addresses appeared in the coinbase?
4. Quotes with paid addresses → PAID (obligation cleared by coinbase)
5. Quotes without paid addresses → FALLBACK (mint issues ecash for accumulated balance)

### On-Chain Verification

The mint verifies coinbase outputs using on-chain payment verification (NUT-XX / BDK payment
processor from CDK). This is the same machinery used for standard on-chain Cashu deposits,
adapted to verify third-party payments (the pool paid, not the mint).

**Coinbase confirmation:** The mint configures how many confirmations are required before
updating the quote status to CONFIRMED. There is no need to wait for the full 100-block
coinbase maturity period — the miners own those outputs as soon as the block is found, even
though the outputs are locked for 100 blocks. Intermediate state: PENDING_CONFIRMATION.

### Accounting

Block reward = X sats. Coinbase is split:
```
Output 0: Miner A → a sats    (quote PAID)
Output 1: Miner B → b sats    (quote PAID)
...
Output N: Mint    → X - (a + b + ...) sats
```

Mint's BTC reserve increases by `X - direct_payouts`. This exactly covers:
- Ecash redemptions (miners who kept their ehash tokens)
- Fallback obligations (miners whose on-chain quotes weren't included)

No surplus, no deficit.

### Coinbase Construction

The pool must know which addresses and amounts to include in the coinbase. Flow:

1. Pool queries mint for accumulated quote balances per payout address
2. Pool selects top N addresses by balance (within coinbase size limits)
3. Pool builds coinbase with selected addresses + mint address for remainder
4. Pool updates coinbase on each new template from the Template Provider

The snapshot of balances at template creation time determines coinbase outputs. Any tokens
contributed between the snapshot and block found are settled via the fallback path (ecash) for
the delta amount.

## Mint and Pool Separation

The mint and pool are separate services with distinct responsibilities:

**Mint:** Manages quotes, verifies token contributions, tracks accumulated balances, verifies
on-chain settlement, executes fallback payments. The mint is a financial service.

**Pool:** Validates shares, issues ehash (via mint quote requests), queries mint for accumulated
balances, constructs block templates and coinbase transactions, mines blocks, notifies mint
when blocks are found. The pool is a mining coordinator.

**Protocol between them** (extends existing SV2 mint-quote subprotocol):
- Pool → Mint: `BlockFound` (block hash, keyset ID, coinbase tx)
- Mint → Pool: Accumulated balances per payout address (for coinbase construction)
- Existing: Pool → Mint: `MintQuoteRequest` (for ehash token issuance per share)

## Future Work

### Auto-Rollover (Ocean Model)

When an epoch closes and a miner's quote is not paid on-chain, one of two things happens
depending on whether the miner wants to manage an ecash wallet:

**No ecash wallet (default for simple miners):** The accumulated balance is automatically
rolled into a new quote for the next epoch with identical settings (same payout address).
No ecash is issued — the balance carries forward directly as a "floor balance" denominated
in satoshis. New ehash contributions from the next epoch add speculative value above the
floor. The miner mines on autopilot until the balance crosses the coinbase payout threshold.

**Ecash wallet (opt-in):** The quote closes and the miner receives ecash for the accumulated
balance. No auto-rollover occurs. The miner manages their own ecash and decides when and
how to spend it.

If a miner stops contributing shares, their accumulated floor balance persists and the pool
will eventually include their output in a coinbase when space allows.

**Ark payouts:** This model naturally lends itself to Ark-based settlement, where the miner
holds an offchain VTXO instead of the mint custodying their bitcoin balance. The mint
coordinates an Ark round at epoch close, granting miners self-custodial offchain UTXOs
without requiring on-chain coinbase space for each payout.

### Open Coinbase Marketplace

Anyone with ecash at the mint (not just miners) can purchase a coinbase output by creating a
melt quote with a payout address and contributing ecash tokens. This opens coinbase space to
non-miners who want specific UTXO properties (e.g., coinbase origin, UTXO consolidation).

### Additional Payout Methods

- **Lightning fallback:** Mint pays via BOLT11/BOLT12 instead of ecash
- **On-chain melt fallback:** Mint sends a standard on-chain transaction (not coinbase)
- **Miner choice:** Fallback method specified at quote creation or chosen at settlement time

### Proxy Pool Support

When hashpool acts as a proxy for an upstream pool:
- Upstream pool pays hashpool via lightning when they win a block
- Lightning payment receipt triggers epoch close and keyset rotation
- No coinbase control (upstream pool controls the coinbase)
- Hashpool's mint settles all miners via ecash or lightning (no direct coinbase payouts)

### Ehash-to-Ecash Swaps (HTLC Locking)

When an epoch closes and the mint holds BTC backing the old epoch's ehash tokens, miners need
a mechanism to atomically swap their ehash tokens for BTC-backed ecash. This swap must be
trustless: neither the miner nor the pool/mint should be able to cheat the other.

**HTLC (Hash Time-Lock Contract) locking** (Cashu NUT-14) is a candidate mechanism:
- Miner and pool agree on a swap: miner's ehash tokens for pool's ecash tokens
- HTLC ensures atomicity: either both sides complete or neither does
- The pool collects ehash (proof of miner contributions) and the miner receives ecash (BTC claim)

**Design decisions:**
- Anyone can buy ehash — the pool does not need to be the swap counterparty.
- The TTL for swap availability after epoch close is configurable by the mint, based on how
  aggressively it wants to close epochs and issue proofs (relevant for future verifiable
  mining features).
- After the swap window closes, unredeemed ehash is lost. This is effectively a donation
  of free work to the rest of the miners in that epoch.
- The mint burns ehash tokens at two points: when they are accumulated into a quote, and
  when they are redeemed for ecash tokens.

### Coinbase Keyset Identification

The V2 keyset ID must be included in the coinbase tag (the data miners place in the
coinbase transaction's scriptSig, after the BIP 34 block height). This is crucial for
functionality: it proves that miner templates are using the correct coinbase for the
current epoch. Without this, there is no way to verify that a miner's template commits
to the correct keyset.

- Enables verification that block templates reference the active epoch's keyset
- Also provides on-chain auditability: links mined blocks to the pool and epoch that
  produced them

**Space constraint:** The coinbase scriptSig has a hard consensus limit of 100 bytes.
After accounting for the BIP 34 block height (~4 bytes) and the extra nonce used by
mining software (~8-12 bytes), roughly 84-88 bytes remain. A V2 keyset ID is 32 bytes
(1 version byte + 31 bytes of truncated SHA-256). Hex-encoding it to 64 ASCII characters
would not leave room for a pool identifier, so the keyset ID must be embedded as raw
bytes rather than hex-encoded ASCII. A raw 32-byte keyset ID plus a short pool tag
(e.g., 10 bytes for `/hashpool/`) fits comfortably within the available space.
