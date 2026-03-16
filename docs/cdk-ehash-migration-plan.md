# CDK Ehash Migration Plan

## Overview

Migrates hashpool's `mint` role from the old CDK API (which used `cdk::nuts::nutXX::MintQuoteMiningShareRequest` and `mint.create_mint_mining_share_quote()`) to the new CDK API (which uses `cdk-ehash::EhashPaymentProcessor`, `cdk::mint::MintQuoteRequest::Custom`, and `mint.get_mint_quote()`).

**Root blocker**: `cdk-redb` is listed as a direct dep in `roles/mint/Cargo.toml` but is completely unused (only `cdk-sqlite` is used). Removing it also removes `redb 3.1.1` from the dependency tree, which eliminates the Rust 1.89 MSRV requirement. No Rust upgrade needed.

---

## API Compatibility Summary

| Old API | New API | Notes |
|---|---|---|
| `cdk::nuts::nutXX::MintQuoteMiningShareRequest` | `cdk_common::nuts::nut04::MintQuoteCustomRequest` | Extra fields go into `serde_json::Value extra` |
| `mint.create_mint_mining_share_quote(req)` | `mint.get_mint_quote(MintQuoteRequest::Custom { method, request })` | Returns `MintQuoteResponse` enum |
| Response type: `MintQuote` | Response type: `MintQuoteResponse::Custom { response: MintQuoteCustomResponse<QuoteId>, .. }` | Destructure the Custom variant |
| `PaymentMethod::MiningShare` | `PaymentMethod::Custom("ehash".to_string())` | MiningShare variant removed from CDK |
| `Mint::new(info, sig, db, ln)` | `Mint::new(info, sig, db, ln, max_inputs, max_outputs)` | Use `usize::MAX` for no limits |
| No payment processor registered | `EhashPaymentProcessor` inserted into `ln` HashMap with key `(HASH, Custom("ehash"))` | Required for `get_mint_quote` to find the processor |
| Quote marked paid by CDK internally | `mint.pay_mint_quote_for_request_id(WaitPaymentResponse {...})` called after `get_mint_quote` | Direct DB call; no need to thread `EhashPaymentProcessor` beyond `setup.rs` |
| `mint_quote_response_from_cdk(hash, MintQuote)` | `mint_quote_response_from_cdk(hash, MintQuoteCustomResponse<QuoteId>)` | Second arg type changes |

---

## Order of Execution

Execute in this order to minimize broken intermediate states:

1. **Step 1** (remove `cdk-redb`, add `cdk-ehash`) — fixes the compile blocker and adds new dep
2. **Step 2** (update `protocols/ehash`) — protocol-layer API changes; `EhashPaymentProcessor` stays in `setup.rs` only
3. **Step 3** (update `setup.rs`) — create and register `EhashPaymentProcessor`, fix `Mint::new()` args
4. **Step 4** (update `quote_processing.rs`) — replace old CDK call with `get_mint_quote` + `pay_mint_quote_for_request_id`; no threading needed
5. **Step 5** (verify `mint-pool-messaging`) — re-export compatibility check
6. **Step 6** (handle `cdk-mintd` config compat)
7. **Step 7** (cleanup) — dead code removal
8. **Step 8** (testing) — verification

---

## Step 1 — Upgrade Rust Version

In `roles/mint/Cargo.toml`:

```toml
# Remove (unused — only cdk-sqlite is used; this pulls in redb 3.1.1 which requires Rust 1.89):
# cdk-redb = { path = "/home/vnprc/work/cdk/crates/cdk-redb" }   <-- DELETE

# Add:
cdk-ehash = { path = "/home/vnprc/work/cdk/crates/cdk-ehash" }
```

That's it for the Rust version issue. `cdk-redb` was the only dep requiring Rust 1.89. The CDK workspace itself declares `rust-version = "1.85.0"` and all other CDK crates are compatible with the current 1.86.0.

**Verify before moving on:**
```bash
cd /home/vnprc/work/hashpool/roles
cargo build -p mint 2>&1 | tail -20
```

The MSRV error (`rustc 1.86.0 is not supported`) must be gone. The build will fail for other reasons (nutXX, missing APIs) but those are addressed in later steps. Step 1 is not done until the MSRV error is absent.

---

## Step 2 — Protocol Layer Changes (`protocols/ehash`)

### 2.1 Update `protocols/ehash/Cargo.toml`

The `cdk` dep was only needed for `cdk::nuts::nutXX`. After Step 2.2, that is gone. Remaining CDK types (`Amount`, `CurrencyUnit`, `PublicKey`, `MintQuoteCustomRequest`) are all available from `cdk-common`.

```toml
# Add:
serde_json = "1"
hex = "0.4"

# Remove (nutXX is gone, cdk-common covers remaining types):
# cdk = { path = "/home/vnprc/work/cdk/crates/cdk" }   <-- DELETE

# Keep:
cdk-common = { path = "/home/vnprc/work/cdk/crates/cdk-common" }
```

Note: verify that `cdk_common` re-exports `MintQuoteCustomRequest`. If not at the top level, import from `cdk_common::nuts::nut04::MintQuoteCustomRequest` directly.

### 2.2 Update `protocols/ehash/src/quote.rs`

**Remove the old import block:**
```rust
// REMOVE:
use cdk::{
    nuts::{nutXX::MintQuoteMiningShareRequest, CurrencyUnit, PublicKey},
    secp256k1::hashes::Hash as CdkHash,
    Amount,
};
use cdk_common::mint::MintQuote;
```

**Add new imports:**
```rust
use cdk_common::{
    nuts::{CurrencyUnit, MintQuoteCustomRequest},
    Amount, PublicKey,
};
use serde_json::json;
```

**3.2a — Change `to_cdk_request` return type and body:**

```rust
// Before:
pub fn to_cdk_request(&self) -> Result<MintQuoteMiningShareRequest, QuoteConversionError>

// After:
pub fn to_cdk_request(&self) -> Result<MintQuoteCustomRequest, QuoteConversionError> {
    let amount = Amount::from(self.request.amount);
    let unit = CurrencyUnit::Custom("HASH".to_string());

    let header_hash_hex = hex::encode(self.share_hash.as_bytes());

    let pubkey = PublicKey::from_slice(self.request.locking_key.inner_as_ref())
        .map_err(|e| QuoteConversionError::InvalidLockingKey(e.to_string()))?;

    Ok(MintQuoteCustomRequest {
        amount,
        unit,
        description: None,
        pubkey: Some(pubkey),
        extra: json!({ "header_hash": header_hash_hex }),
    })
}
```

Key points:
- `CdkHash::from_slice` is no longer needed; header hash goes into `extra` as a hex string
- `description` is dropped (None is safe for now; can be restored if needed)

**3.2b — Update `mint_quote_response_from_cdk`:**

Current signature takes `MintQuote`. Change to accept `MintQuoteCustomResponse<QuoteId>` (what `get_mint_quote` returns after destructuring the Custom variant):

```rust
use cdk_common::nuts::MintQuoteCustomResponse;
use cdk::mint::QuoteId;

// Before:
pub fn mint_quote_response_from_cdk(
    share_hash: ShareHash,
    quote: MintQuote,
) -> Result<MintQuoteResponse<'static>, QuoteConversionError>

// After:
pub fn mint_quote_response_from_cdk(
    share_hash: ShareHash,
    custom_response: MintQuoteCustomResponse<QuoteId>,
) -> Result<MintQuoteResponse<'static>, QuoteConversionError> {
    let quote_id = Str0255::try_from(custom_response.quote.to_string())
        .map_err(QuoteConversionError::InvalidQuoteId)?;

    let header_hash = share_hash.into_u256()?;

    Ok(MintQuoteResponse {
        quote_id,
        header_hash,
    })
}
```

Note: `QuoteId` is re-exported as `cdk::mint::QuoteId`. The `.to_string()` on `QuoteId` gives the UUID string that `Str0255::try_from` accepts.

---

## Step 3 — Mint Manager Changes (`roles/mint/src/lib/mint_manager/setup.rs`)

`setup_mint` **return type does not change** — `EhashPaymentProcessor` stays internal to this function.

### 3.1 Add Import
```rust
use cdk_ehash::EhashPaymentProcessor;
```

### 3.2 Create `EhashPaymentProcessor` and Insert into `ln` HashMap

Replace the empty HashMap construction with:

```rust
let ehash_processor = Arc::new(EhashPaymentProcessor::new(hash_currency_unit.clone()));

let mut ln: HashMap<
    PaymentProcessorKey,
    Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
> = HashMap::new();

ln.insert(
    PaymentProcessorKey::new(
        hash_currency_unit.clone(),
        PaymentMethod::Custom("ehash".to_string()),
    ),
    Arc::clone(&ehash_processor) as Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
);
```

`EhashPaymentProcessor` is only needed here — `Mint` holds it via the `payment_processors` map, and `quote_processing.rs` triggers payment through `Mint` directly (see Step 4).

### 3.3 Replace `PaymentMethod::MiningShare` with `PaymentMethod::Custom("ehash")`

```rust
let ehash_method_settings = MintMethodSettings {
    method: PaymentMethod::Custom("ehash".to_string()),
    unit: hash_currency_unit.clone(),
    min_amount: Some(Amount::from(1)),
    max_amount: Some(Amount::from(u64::MAX)),
    options: None,
};
```

### 3.4 Fix `Mint::new()` Call

The new signature requires `max_inputs` and `max_outputs`:

```rust
// Before:
let mint = Arc::new(Mint::new(mint_info, signatory, db, ln).await.unwrap());

// After:
let mint = Arc::new(
    Mint::new(mint_info, signatory, db, ln, usize::MAX, usize::MAX)
        .await
        .unwrap(),
);
```

Use `usize::MAX` (not `0`) — CDK checks `if output_len > self.max_outputs` so `0` would block all minting.

---

## Step 4 — Update `quote_processing.rs` to Use New CDK API

`quote_processing.rs` already has `Arc<Mint>`. The `Mint` exposes `pay_mint_quote_for_request_id` which marks a quote paid directly in the database — no need to thread `EhashPaymentProcessor` through the call stack at all.

**New imports:**
```rust
use cdk::mint::{MintQuoteRequest, MintQuoteResponse as CdkMintQuoteResponse};
use cdk_common::payment::{PaymentIdentifier, WaitPaymentResponse};
use cdk_common::Amount;
```

**Replace the CDK API call block:**
```rust
let cdk_custom_request = parsed_request
    .to_cdk_request()
    .map_err(|e| anyhow::anyhow!("Failed to convert MintQuoteRequest: {e}"))?;

let mint_quote_request = MintQuoteRequest::Custom {
    method: "ehash".to_string(),
    request: cdk_custom_request,
};

match mint.get_mint_quote(mint_quote_request).await {
    Ok(cdk_response) => {
        let custom_response = match cdk_response {
            CdkMintQuoteResponse::Custom { response, .. } => response,
            _ => {
                return Err(anyhow::anyhow!(
                    "Expected Custom mint quote response, got different variant"
                ));
            }
        };

        let quote_id_str = custom_response.quote.to_string();
        info!(
            "Successfully created mint quote: quote_id={} share_hash={} amount={}",
            quote_id_str, share_hash, amount,
        );

        // Mark quote as paid immediately — pool validated the share before sending this message.
        // Call pay_mint_quote_for_request_id directly on Mint (which already holds
        // EhashPaymentProcessor in its payment_processors map via setup_mint).
        let header_hash_hex = hex::encode(share_hash.as_bytes());
        let amount_with_unit = Amount::new(amount, CurrencyUnit::Custom("HASH".to_string()));

        mint.pay_mint_quote_for_request_id(WaitPaymentResponse {
            payment_identifier: PaymentIdentifier::CustomId(header_hash_hex.clone()),
            payment_amount: amount_with_unit,
            payment_id: header_hash_hex,
        })
        .await
        .map_err(|e| anyhow::anyhow!("Failed to pay ehash quote: {e}"))?;

        // Convert CDK custom response to SV2 wire format
        let sv2_response = mint_quote_response_from_cdk(share_hash, custom_response)
            .map_err(|e| anyhow::anyhow!("Failed to convert mint quote response: {e}"))?;

        send_quote_response_to_pool(sv2_response, sender).await?;
        Ok(())
    }
    Err(e) => {
        // ... existing error handling logic unchanged ...
    }
}
```

**No signature change** to `process_mint_quote_message` — it already takes `Arc<Mint>`, which is sufficient.

`main.rs`, `connection.rs`, and `message_handler.rs` are unchanged.

---

## Step 5 — Verify `mint-pool-messaging` Re-export Compatibility

The `mint_quote_response_from_cdk` function is re-exported from `ehash` through `mint-pool-messaging`. After Step 2.2b, its second argument changes. The re-export line in `mint-pool-messaging/src/lib.rs` does not need a change (re-exports by name). The call site in `quote_processing.rs` (Step 4) now passes `custom_response: MintQuoteCustomResponse<QuoteId>` which matches.

Check if `mint-pool-messaging/Cargo.toml` needs `cdk-common` added for `MintQuoteCustomResponse` and `QuoteId` to be in scope for any docs/tests. If needed:

```toml
cdk-common = { path = "/home/vnprc/work/cdk/crates/cdk-common" }
```

---

## Step 6 — Handle `cdk-mintd` Config Compatibility

The current `setup.rs` uses `cdk_mintd::config::Settings`. Hashpool builds `Mint` manually (does not use `run_mintd_with_shutdown`), so the new `extra_processors` param in that function is irrelevant. Verify that `cdk_mintd::config::Settings` still has the same fields (`info`, `mint_info`) by spot-checking the CDK source. If the config shape changed, update TOML parsing in `main.rs`.

---

## Step 7 — Cleanup: Remove Dead Code

After migration compiles:
- Remove `cdk::secp256k1::hashes::Hash as CdkHash` import from `quote.rs`
- Remove any remaining references to `nutXX` or `MintQuoteMiningShareRequest`
- Remove `cdk_common::mint::MintQuote` import from `quote.rs`
- Check if the `toml` git dependency in `protocols/ehash/Cargo.toml` is still needed (it was a CDK compatibility workaround)
- Remove `QuoteConversionError::InvalidHeaderHash` variant if no longer used

---

## Step 8 — Testing

### 8.1 Unit Tests in `protocols/ehash`

Tests are gated behind `#[cfg(all(test, disabled_pending_fixes))]`. After migration, re-enable by removing the gate:

```rust
#[cfg(test)]
mod tests {
```

Update assertions in `parses_and_converts_request_payload`:
- `cdk_request.pubkey == Some(expected_pubkey)` (was direct equality)
- Add: `assert_eq!(cdk_request.extra["header_hash"], hex::encode(&hash))`

### 8.2 Compile Check

```bash
cd /home/vnprc/work/hashpool/roles
cargo build --workspace 2>&1 | tail -20
```

Expected: exit code 0, only pre-existing warnings.

### 8.3 Live Stack Test

Run `devenv up` in regtest mode. Watch `logs/mint.log` for:
- `Successfully created mint quote` — confirms `get_mint_quote` works
- No `Failed to pay ehash quote` errors — confirms `pay_ehash_quote` works

### 8.4 Token Redemption Test

After a share arrives and a quote is created, use `cdk-cli` to mint tokens:
```bash
./bin/cdk-cli mint --amount <N> --unit HASH --quote <quote_id> --mint http://localhost:3338
```
Expected: quote transitions UNPAID → PAID → ISSUED, tokens are returned.
