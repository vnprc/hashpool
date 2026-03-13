# Technical Debt & Future Improvements

Issues and improvements deferred during development to maintain focus on core functionality.

## Critical / Production Bugs

### Vardiff Update Causes Mass Miner Disconnection Loop

**Status:** Reproduces on production (2025-11-21 16:02 UTC)
**Priority:** CRITICAL - Causes total pool hashrate collapse
**Impact:** All connected miners disconnect simultaneously, causing cascading reconnection attempts

**Root Cause:**
Pool vardiff (variable difficulty) update logic attempts to set `maximum_target` values that violate SV2 channel constraints, triggering `RequestedMaxTargetOutOfRange` errors. When the pool fails to send valid `SetTarget` messages to downstream channels, those channels become unable to operate. Miners immediately receive channel receiver errors and disconnect. Upon reconnect, they encounter the same failing vardiff update, creating an infinite disconnect loop.

**Original Error Message:**
```
[WARN] pool_sv2::mining_pool: Failed to update extended channel channel_id=3 during vardiff RequestedMaxTargetOutOfRange
[WARN] pool_sv2::mining_pool: Failed to update extended channel channel_id=8 during vardiff RequestedMaxTargetOutOfRange
[WARN] pool_sv2::mining_pool: Failed to update extended channel channel_id=7 during vardiff RequestedMaxTargetOutOfRange
```

Followed by:
```
[ERROR] translator_sv2::sv1::downstream::downstream: Error receiving downstream message: RecvError
[ERROR] translator_sv2::sv1::downstream::downstream: Downstream 462: error in downstream message handler: ChannelErrorReceiver(RecvError)
[WARN] translator_sv2::status: Downstream [462] shutting down due to error: ChannelErrorReceiver(RecvError)
```

**Evidence:**
- **pool.log (2025-11-21T16:02:25):** Repeated `RequestedMaxTargetOutOfRange` for channels 3, 7, 8
- **proxy.log (2025-11-21T16:02:00+):** Downstream IDs 462-466+ connect, immediately receive `RecvError`, disconnect in rapid succession
- **Symptom:** Reported hashrate drops to 0, "connected miners" stat keeps incrementing as same miners re-register on reconnect

**Affected Code:**
- `roles/pool/src/lib/mining_pool/mod.rs` - Vardiff update logic (location unknown, needs investigation)
- The code that calculates `maximum_target` for `SetTarget` messages

**What Needs To Happen:**
1. Identify the exact vardiff calculation that produces invalid `maximum_target` values
2. Understand SV2 channel max_target constraints and how they're negotiated
3. Add validation before attempting `SetTarget` to fail gracefully instead of crashing channels
4. Add integration test that simulates various network difficulty conditions to catch this regression

**Notes:**
- Need to trace how `maximum_target` is calculated in vardiff logic
- Check if constraint comes from channel negotiation parameters or SV2 spec
- Consider whether this is a difficulty spike issue or calculation bug

---

## Blocked / Cannot Proceed

### Mint Service Missing SRI Spec APIs

**Status:** Cannot compile - blocked by SRI specification gaps
**Priority:** Critical - Blocking Phase 3 completion

**Current Issue:**
Mint service fails to compile with 5 errors:
1. Missing `plain_connection_tokio` module in `network_helpers_sv2`
2. Missing `PoolMessages` enum in `roles_logic_sv2::parsers_sv2`
3. Missing `Minting` variant in `parsers_sv2`

**Error Details:**
```
error[E0432]: unresolved import `network_helpers_sv2::plain_connection_tokio::PlainConnection`
error[E0432]: unresolved import `roles_logic_sv2::parsers_sv2::PoolMessages`
error[E0433]: failed to resolve: could not find `Minting` in `parsers_sv2`
```

**Locations:**
- `roles/mint/src/lib/sv2_connection/connection.rs:5` - PlainConnection import
- `roles/mint/src/lib/sv2_connection/message_handler.rs:3` - PoolMessages import
- `roles/mint/src/lib/sv2_connection/quote_processing.rs:8,81,120` - PoolMessages and Minting

**Why This Blocks Development:**
The SRI 1.5.0 specification doesn't yet include:
- Plain TCP connection APIs for non-Noise connections (needed for quote protocol)
- Pool-level messages for quote handling (Mint-specific extensions)
- Message variants for Mint Quote request/response

**What Needs To Happen:**
1. SRI specification must be updated to include PoolMessages and plain_connection_tokio APIs
2. Stratum V2 spec must clarify Mint Quote protocol message flow
3. Once spec APIs are available, uncomment "mint" in roles/Cargo.toml
4. Fix mint/src/lib/sv2_connection imports to use proper APIs

**Current Workaround:**
- Mint is commented out in `roles/Cargo.toml` workspace members
- Mint process is disabled in `devenv.nix`
- Mint config files exist but cannot be used

**Owner:**
SRI (Stratum V2 Reference Implementation) maintainers - awaiting spec updates

---

## High Priority

### Mint SV2 Connection Not Using Proper Protocol

**Status:** Currently using PlainConnection workaround
**Priority:** High - Security & Protocol Compliance

**Current Implementation:**
The mint service connects to the pool using `PlainConnection` which:
- Skips Noise encryption handshake (no authentication)
- Skips `SetupConnection` protocol negotiation (no capability flags)
- Is basically raw framed TCP with no security

**Location:** `roles/mint/src/lib/sv2_connection/connection.rs:24`

**What Needs To Happen:**
1. Use proper `Connection::new()` with `HandshakeRole::Initiator` like translator/JD do
2. Send `SetupConnection` message with appropriate flags after handshake
3. Handle `SetupConnectionSuccess/Error` response
4. Store negotiated capabilities
5. Determine if mint should open a mining channel or if quote protocol is channel-less (currently shows port number in channel ID column)

**Reference Implementation:**
- Good example: `roles/translator/src/lib/upstream_sv2/upstream.rs:167`
- Good example: `roles/jd-server/src/lib/job_declarator/mod.rs:467`

**Security Implications:**
- Mint quote requests/responses currently sent in plaintext
- No authentication of pool server
- Vulnerable to MITM attacks
- Not production-ready

---

## Medium Priority

### Shared Config Sections Leaking Service Connection Details

**Status:** Confusing architectural design + inconsistent parsing
**Priority:** Medium - Clean up config organization

**Current Problem:**
1. **Conceptual confusion:** Shared configs contain sections that exist purely for cross-service connections (e.g., `[mint]`, `[pool]`, `[proxy]` in miner.toml that only translator reads). These aren't truly "shared".
2. **Inconsistent parsing:** Roles parse shared configs differently:
   - `pool` and `mint` use `PoolGlobalConfig` struct (type-safe)
   - `web-pool` reads `config/shared/pool.toml` directly via toml parsing (manual extraction)
   - `stats-proxy` and `web-proxy` read `config/shared/miner.toml` directly via toml parsing (manual extraction)

**What Needs To Happen:**
1. Rename `config/shared/miner.toml` → `config/shared/proxy.toml` (clearer intent)
2. Rename translator config files: `tproxy.config.toml` → `proxy.config.toml` (both dev and prod)
3. Create `ProxyGlobalConfig` struct in `roles/roles-utils/config/src/lib.rs` (parallel to `PoolGlobalConfig`)
4. Update all miner-side roles to use `ProxyGlobalConfig` instead of manual toml parsing:
   - `stats-proxy`: load via `ProxyGlobalConfig::from_path()`
   - `web-proxy`: load via `ProxyGlobalConfig::from_path()`
5. Update `pool`-side `web-pool` to use `PoolGlobalConfig` instead of manual toml parsing
6. Update systemd services and deployment docs to use new filenames

**Files to Rename:**
- `config/shared/miner.toml` → `config/shared/proxy.toml`
- `config/tproxy.config.toml` → `config/proxy.config.toml`
- `config/prod/tproxy.config.toml` → `config/prod/proxy.config.toml`
- Update `roles/translator/src/args.rs` default value from `proxy-config.toml`

**Current Code Locations (need refactoring):**
- `roles/roles-utils/config/src/lib.rs:100` - Add `ProxyGlobalConfig` struct here
- `roles/stats-proxy/src/config.rs` - Replace manual toml parsing with struct
- `roles/web-proxy/src/config.rs` - Replace manual toml parsing with struct
- `roles/web-pool/src/config.rs` - Replace manual toml parsing with `PoolGlobalConfig`
- `roles/translator/src/args.rs` - Update default config filename
- Systemd services that reference `tproxy.config.toml`

---

## Medium Priority

### Intermittent SV2 Frame Parse Failure After MintQuoteNotification

**Status:** Intermittent; not reproducible as of 2026-03-13 (resolved after restarting services)
**Priority:** Medium - Can crash translator upstream channel manager

**Symptoms:**
Translator receives malformed MintQuoteNotification `quote_id` (non‑UTF8 hex) and then fails to parse a subsequent upstream frame, causing ChannelManager shutdown and cascading disconnects.

**Full Error Message (proxy.log):**
```
WARN  mint_quote_state_mining_share{quote_id="0x00000001aa54b4690000002001004600020000" mint_url=http://localhost:3338}: cdk::wallet::mint_connector::transport: Http Response error: missing field `quote` at line 1 column 94
WARN  cdk::wallet::issue::issue_mining_share: failed to fetch mining share quote state quote_id="0x00000001aa54b4690000002001004600020000" mint_url=http://localhost:3338 error=Unknown error response: `code: 4040, detail: Mining share quote 0x00000001aa54b4690000002001004600020000 not found`
WARN  translator_sv2::sv2::channel_manager::channel_manager: Translator: failed to persist mint quote 0x00000001aa54b4690000002001004600020000 to wallet: UnknownErrorResponse("code: 4040, detail: Mining share quote 0x00000001aa54b4690000002001004600020000 not found")
ERROR translator_sv2::sv2::channel_manager::channel_manager: Failed to parse upstream frame into AnyMessage: BinaryError(ReadError(180, 207))
ERROR translator_sv2::status: Error in ChannelManager(Sender { .. }): General("Failed to parse AnyMessage")
WARN  translator_sv2::status: ChannelManager shutting down due to error: General("Failed to parse AnyMessage")
```

**Hypothesis:**
Upstream frames become desynchronized, possibly due to a malformed MintQuoteNotification payload (quote_id bytes not UTF‑8 / wrong length), causing subsequent frames to be misread.

**Evidence:**
- Quote_id shows up as hex `0x00000001aa54b469...` instead of UUID.
- Immediately followed by `BinaryError(ReadError(180, 207))`.

**What Needs To Happen:**
1. Instrument pool + translator to log MintQuoteNotification payload lengths + quote_id bytes (info level).
2. Confirm whether pool is emitting malformed frames or corruption occurs in transit.
3. Add validation in pool to reject non‑UTF8 quote_id before sending, or log + drop.
4. Consider hardening translator to drop only the bad message instead of crashing the channel manager.

---

### Stats Protocol: Newline-Delimited JSON

**Status:** Custom ad-hoc protocol
**Priority:** Low-Medium - Works but fragile

**Current Implementation:**
```rust
// Sender (pool):
let json = serde_json::to_vec(&msg)?;
buffer.extend_from_slice(&json);
buffer.push(b'\n');
stream.write_all(&buffer).await?;

// Receiver (pool-stats):
while let Some(newline_pos) = leftover.iter().position(|&b| b == b'\n') {
    let line = &leftover[..newline_pos];
    handler.handle_message(line).await?;
    leftover.drain(..=newline_pos);
}
```

**Issues:**
- No proper framing (relies on newlines which could appear in JSON strings)
- Manual buffer management is error-prone
- No authentication or encryption
- Text encoding overhead vs binary
- Not self-describing (no version field)

**Options:**
1. **Keep it** - It works, easy to debug with `nc`
2. **Length-prefixed JSON** - Add 4-byte length header, proper framing
3. **MessagePack** - Binary JSON, faster/smaller, still serde compatible
4. **SV2 Custom Messages** - Migrate to SV2 protocol extensions (ties into architecture question above)

**Recommendation:** Keep for now, revisit when we settle on stats architecture

### Mint Quote Plumbing Cleanups (from rework plan)

**Status:** Functional but carrying technical debt
**Priority:** Medium – keeps future maintenance simple

1. **Unify quote tracking in the hub.** `QuotePoller` still maintains its own `pending_quotes` map (`roles/pool/src/lib/mining_pool/quote_poller.rs:28`). Move it to rely on `MintPoolMessageHub::pending_quote` / `PendingQuoteContext` so there is one source of truth for outstanding quotes.
2. **Use typed frame builders.** The mint forwarding path still hand-builds frames with `StandardSv2Frame::from_bytes_unchecked()` (`roles/pool/src/lib/mining_pool/mint_connection.rs:315`). Switch to the helpers in `roles/roles-utils/mint-pool-messaging/src/sv2_frames.rs` to remove manual header math.
3. **Decide who owns channel context.** `MintIntegrationManager` only proxies locking-key lookups (`roles/pool/src/lib/mining_pool/mint_integration.rs:14`). Fold that state into `MintPoolMessageHub` (or document why we can't) so quote routing doesn't depend on duplicate maps.

Follow-up: After the hub owns pending state, the poller can just iterate `hub.pending_quotes()` and we can delete the bespoke tracker entirely.

---

## Low Priority

### Systemd Service Environment Configuration Scattered

**Status:** Works but repetitive
**Priority:** Low - Operational convenience

**Current Problem:**
Environment variables like `BITCOIND_NETWORK=testnet4` and `RUST_LOG=info` are hardcoded in each systemd service file, duplicated across multiple services. Makes it hard to change globally (e.g., switching from testnet4 to mainnet).

**What Needs To Happen:**
1. Create `/etc/hashpool/hashpool.env` with centralized environment variables:
```ini
BITCOIND_NETWORK=testnet4
RUST_LOG=info
```

2. Update all systemd services to source this file:
```ini
EnvironmentFile=/etc/hashpool/hashpool.env
```

3. Remove duplicate `Environment=` directives from individual service files

**Services affected:**
- hashpool-pool.service
- hashpool-proxy.service
- (any others that add common env vars)

**Benefit:** Single point to change network/log level across all services

---

### Stats Service Connection Identity

**Current:** Mint uses `address.port()` as downstream_id
**Better:** Use proper ID allocation like other downstreams

The mint connection doesn't go through the normal downstream ID allocation because it doesn't use the standard connection path. Once it uses proper SV2, it should get a proper downstream_id.

---

## Documentation Needed

- Document the mint quote protocol flow
- Document why we need both pool-stats and proxy-stats services
- Architecture diagram showing all service connections
- SV2 message flow diagrams

---

## Testing Needed

- Integration test for mint connection failure/reconnection
- Test mint behavior when pool restarts
- Test what happens when mint and JD both connect simultaneously
- Load testing with multiple mints
