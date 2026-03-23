# SRI PR: Add extension hook for vendor-specific SV2 Mining messages (0xC0–0xFF)

**Target repo:** `stratum-mining/stratum`
**File:** new file, suggested `protocols/stratum-apps/src/extensions.rs` (or similar)
**Status:** Pending — can be filed independently of other work

---

## Summary

Add a `VendorMessageHandler` trait (name TBD with SRI) to the stratum-apps channel manager
that provides an extension hook for Mining message types in the 0xC0–0xFF range, which the
SV2 specification explicitly reserves for vendor-specific use.

---

## Motivation

The SV2 specification defines the following message type ranges for the Mining protocol:

| Range | Purpose |
|-------|---------|
| 0x00–0x1F | Standard Mining subprotocol messages |
| 0x20–0x7F | Reserved for future standard use |
| 0xC0–0xFF | **Vendor-specific / non-standard** |

The vendor range exists so that operators can extend the protocol for their own use cases
without conflicting with the standard message space and without requiring a spec change.

The stratum-apps `ChannelManager` currently has no mechanism to handle messages in this
range. Unknown message types are presumably dropped or produce an error, making the vendor
extension mechanism in the spec impossible to use without forking the framework.

**This PR makes the spec's vendor extension range operational in the reference implementation.**

---

## Proposed interface

```rust
/// Extension hook for handling vendor-specific SV2 Mining message types.
///
/// The SV2 specification reserves message types 0xC0–0xFF for vendor-specific use.
/// Implement this trait to handle those messages without modifying the channel manager.
///
/// A no-op default implementation is provided via [`NoopVendorMessageHandler`].
pub trait VendorMessageHandler: Send + Sync + fmt::Debug {
    /// Handle a Mining message with a vendor-specific type byte (0xC0–0xFF).
    ///
    /// `msg_type` is the raw message type byte.
    /// `payload` is the raw frame payload (after the SV2 header).
    ///
    /// Return `Ok(())` to continue normal operation.
    /// Return `Err(_)` to signal that the channel should be closed.
    fn handle_vendor_message(
        &self,
        msg_type: u8,
        payload: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// No-op implementation. Silently ignores all vendor messages.
#[derive(Debug)]
pub struct NoopVendorMessageHandler;

impl VendorMessageHandler for NoopVendorMessageHandler {
    fn handle_vendor_message(&self, _msg_type: u8, _payload: &[u8])
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        Ok(())
    }
}
```

Key differences from the current local version (`CustomMiningMessageHandler`):

- **Name**: `VendorMessageHandler` — matches spec language ("vendor-specific")
- **Sync not async**: avoids the `async_trait` dependency; vendor message handling is
  expected to be fast decode-and-dispatch, not async I/O
- **Error type**: `Box<dyn std::error::Error + Send + Sync>` instead of `anyhow::Error` —
  SRI avoids `anyhow` in library crates
- **Location**: framework layer, not inside a payment/CDK module

---

## Integration point

The `ChannelManager` should accept an optional `Arc<dyn VendorMessageHandler>` at
construction time, defaulting to `Arc::new(NoopVendorMessageHandler)`. When the message
dispatch loop encounters a type byte ≥ 0xC0, it routes to the handler instead of returning
an unknown-message error.

---

## Alignment with SRI goals

SRI's stated goal is to provide a reference implementation that other teams can build SV2
applications on top of. The vendor message range exists precisely to enable this ecosystem
extensibility without requiring changes to the spec or the framework.

Without this hook, every team that needs vendor messages (payment systems, operator
notifications, custom telemetry) must fork stratum-apps. That runs counter to SRI's goal
of maintaining a shared, stable base.

This PR closes the gap with a minimal, non-opinionated interface that imposes no behavior
on teams that don't use vendor messages (the default is a no-op).

---

## What hashpool uses this for

Hashpool sends CDK payment notifications from the pool to the translator using message types
0xC0 (`MintQuoteNotification`) and 0xC1 (`MintQuoteFailure`), defined in the
`mint_quote_sv2` protocol crate. These messages carry ehash token minting instructions
between the pool and the proxy.

This is the concrete use case motivating the PR, but the trait itself is entirely generic.
The PR should not mention CDK, ehash, or hashpool anywhere in its diff.

---

## Async vs sync

The local version (`CustomMiningMessageHandler`) uses `async_trait`. The upstream version
should be sync for two reasons:

1. SRI avoids `async_trait` where possible (it adds overhead and the macro is non-obvious)
2. Vendor message handling should be a fast decode + channel send. If the handler needs to
   do async work (e.g. wallet operations), it should spawn a task internally rather than
   blocking the message dispatch loop.

The hashpool implementation (`CdkQuoteNotificationHandler`) does call async CDK wallet
methods. If the upstream version is sync, that implementation would change to:
- Decode the message synchronously
- Send to a `tokio::sync::mpsc` channel
- A separate task handles the async wallet call

This is a better architecture anyway — the message dispatch loop should not block on
wallet I/O.

---

## Local changes when filing

1. Move trait definition from `roles/translator/src/lib/payment/custom_handler.rs` to a
   new file in stratum-apps source
2. Rename `CustomMiningMessageHandler` → `VendorMessageHandler` throughout translator
3. Change `handle_custom_message` → `handle_vendor_message`
4. Remove `async_trait` dep from trait signature
5. Update `CdkQuoteNotificationHandler` to be sync: decode message, send to mpsc channel,
   spawn separate async task for wallet operations
6. Update `NoopCustomMiningMessageHandler` → `NoopVendorMessageHandler`

---

## Unblocking condition

This PR can be filed independently of all other work. It does not require the Sv1ClientInfo
PR to land first, and it does not require un-vendoring stratum-apps first. It can be opened
against the current upstream immediately.
