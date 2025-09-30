# Hashpool to SRI Extension Migration Plan (Fast Track)

## Goal
Transform Hashpool from an SRI fork into a clean SRI extension as quickly as possible, accepting breaking changes to accelerate development.

## Phase 1: External Extension Infrastructure (Week 1)
**Goal:** Create external extension that intercepts messages without modifying SRI core

### Day 1-2: External Extension Crate
- [x] ~~Create `protocols/v2/extensions/cashu/` module~~ **CHANGED: Keep as external crate**
- [x] Implement TLV encoder/decoder for Cashu fields:
  - Type 0x0003_01: `locking_pubkey` (33 bytes)
  - ~~Type 0x0003_02: `hash` (32 bytes)~~ **REMOVED - hash computed from share fields**
- [ ] **NEW APPROACH:** Implement Message Interceptor Pattern
  - [ ] Create `ehash/` external extension crate
  - [ ] Implement `MessageInterceptor` trait for byte-level message processing
  - [ ] Add message type detection (identify SubmitSharesExtended from raw bytes)
  - [ ] Add TLV appending to outgoing messages (translator side)
  - [ ] Add TLV extraction from incoming messages (pool side)
- [x] **COMPLETED:** Implement hash computation from share fields
  - [x] Add `compute_share_hash()` function using job_id, nonce, ntime, version, extranonce
  - [x] Replace all `m.hash` references with computed hash calls

### Day 3-4: Minimal SRI Integration Hooks
- [x] Remove `hash` and `locking_pubkey` from `SubmitSharesExtended` struct  
- [x] Remove `hash` from `SubmitSharesSuccess` struct
- [ ] **NEW:** Add minimal message interceptor hooks to SRI
  - [ ] Add optional `MessageInterceptor` trait to network layer
  - [ ] Add hooks in translator upstream message sending
  - [ ] Add hooks in pool message receiving
  - [ ] Keep hooks optional (SRI works without extension)
- [x] **COMPLETED:** Fix compilation errors throughout codebase

### Day 5: Basic Testing
- [ ] Test extension loading and message interception
- [ ] Test TLV round-trip (append + extract)
- [ ] Test on testnet4 with external extension enabled

## Phase 2: Extension Negotiation (Week 2)  
**Goal:** Implement extension negotiation through external interceptor

### Day 1-2: Extension-Based Negotiation
- [x] ~~Implement `RequestExtensions` in SRI core~~ **CHANGED: Move to external extension**
- [ ] **NEW:** Implement negotiation in external extension
  - [ ] Extension handles RequestExtensions messages via interceptor
  - [ ] Track negotiated extensions in extension state
  - [ ] Hardcode Cashu extension (0x0003) as always-on for now

### Day 3-4: External Message Routing
- [x] ~~Route mint quote messages in SRI core~~ **CHANGED: External routing**
- [ ] **NEW:** Route extension messages in external crate
  - [ ] Route `MintQuoteNotification` (0xC0) through external handler
  - [ ] Route `MintQuoteFailure` (0xC1) through external handler  
  - [ ] Remove `SV2_MINT_QUOTE_PROTOCOL_DISCRIMINANT` - use extension negotiation

### Day 5: Integration
- [ ] External extension advertises Cashu capability
- [ ] External extension handles negotiation
- [ ] Test end-to-end flow on testnet4

## Phase 3: Complete External Separation (Week 3)
**Goal:** Move ALL Cashu logic to external extension repository

### Day 1-2: Create External Repository
- [ ] **NEW:** Create `ehash` external extension repository
- [ ] Move all mint quote logic from `protocols/v2/subprotocols/` to external repo
- [ ] Delete `protocols/v2/subprotocols/mint-quote/` entirely from SRI fork
- [ ] External extension depends on compiled SRI binaries only

### Day 3-4: Define Clean Extension API
```rust
// External extension trait - no SRI internal dependencies
pub trait MessageInterceptor {
    fn intercept_outgoing(&self, msg_bytes: &mut Vec<u8>) -> Result<(), Error>;
    fn intercept_incoming(&self, msg_bytes: &[u8]) -> Result<(Vec<u8>, ExtensionData), Error>;
}

// Cashu-specific implementation
impl MessageInterceptor for CashuExtension {
    fn intercept_outgoing(&self, msg_bytes: &mut Vec<u8>) -> Result<(), Error> {
        // Detect SubmitSharesExtended and append TLV fields
    }
    fn intercept_incoming(&self, msg_bytes: &[u8]) -> Result<(Vec<u8>, ExtensionData), Error> {
        // Extract TLV fields and return core message + extension data
    }
}
```

### Day 5: External Integration
- [ ] SRI loads external extension via dynamic linking or feature flag
- [ ] All Cashu functionality works through external interceptor only
- [ ] SRI fork contains zero Cashu-specific code

## Phase 4: Update to Latest SRI (Week 4)
**Goal:** Get on latest SRI version now that we're not forking core files

### Day 1-2: Update Dependencies
- [ ] Update Cargo.toml to latest SRI
- [ ] Fix breaking changes from SRI updates
- [ ] Ensure extension still works with new version

### Day 3-5: Validate & Fix
- [ ] Run full test suite
- [ ] Fix any integration issues
- [ ] Deploy to testnet4
- [ ] Verify mining and token generation works

## External Extension Message Flow Architecture

### How External Extension Intercepts SRI Messages
```
Old Approach (Internal):
SRI Translator → SRI Message → SRI Pool
              ↳ (TLV fields embedded in SRI code)

New Approach (External):
SRI Translator → Extension Interceptor → SRI Pool
                 ↳ TLV append          ↳ TLV extract

Detailed Flow:
1. SRI creates SubmitSharesExtended { job_id, nonce, ntime, version, extranonce }
2. SRI serializes to bytes with binary_sv2
3. External extension intercepts bytes
4. Extension appends TLV fields: [0x00, 0x03, 0x01, 0x00, 0x21, <33 bytes locking_pubkey>]
5. Extended bytes sent to pool
6. Pool receives extended bytes
7. External extension extracts TLV fields first
8. Core message bytes passed to SRI for normal processing
9. Extension provides extracted data to business logic
```

### External Extension Integration Points
- **Network Layer Hooks**: Minimal interceptor points in SRI
- **Byte-Level Processing**: Extension works with serialized messages
- **Zero SRI Core Changes**: Extension logic completely external
- **Dynamic Loading**: Extension loaded via feature flag or dynamic linking
- **Clean Separation**: SRI works without extension loaded

## Key Differences from Conservative Approach

1. **No Parallel Implementation** - Directly replace struct fields with TLV
2. **No Backward Compatibility** - Break things and fix forward
3. **No Feature Flags** - One code path only
4. **No Gradual Rollout** - Deploy all changes at once
5. **Minimal Testing** - Just enough to verify it works
6. **No Database Migration** - Wipe and restart if needed

## Fast Development Tips

### What to Delete Immediately
```bash
# Remove all mint-quote protocol files
rm -rf protocols/v2/subprotocols/mint-quote/

# Remove custom fields from submit_shares.rs
# Just revert to upstream version
```

### What to Build First
1. TLV encoder/decoder (few hundred lines)
2. Extension negotiation (basic version ~100 lines)
3. Message router updates (modify existing handlers)

### What to Defer
- Formal specification (document after it works)
- Performance optimization (TLV overhead is fine for now)
- Comprehensive error handling (fail fast during development)
- Database compatibility (wipe testnet4 data as needed)

## Success Criteria (End of Week 4)
- ✅ Running on latest SRI version
- ✅ All Cashu logic in separate extension module
- ✅ No modifications to core SRI protocol files
- ✅ Working on testnet4 with new architecture
- ✅ Can pull SRI updates without merge conflicts

## Next Steps After Migration
1. Optimize TLV encoding if performance issues
2. Write formal extension specification
3. Improve error handling and edge cases
4. Consider proposing to upstream SRI
5. Add configuration for extension parameters

## Immediate Action Items (Today)
1. Create `protocols/v2/extensions/cashu/tlv.rs` with encoder/decoder
2. Remove custom fields from `SubmitSharesExtended`
3. Update one message handler to use TLV fields
4. Test that single flow end-to-end
5. Iterate from there

This approach trades stability for speed - perfect for pre-production development.