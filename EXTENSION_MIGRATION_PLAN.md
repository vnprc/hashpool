# Hashpool to SRI Extension Migration Plan (Fast Track)

## Goal
Transform Hashpool from an SRI fork into a clean SRI extension as quickly as possible, accepting breaking changes to accelerate development.

## Phase 1: TLV Implementation (Week 1)
**Goal:** Replace struct modifications with TLV fields immediately

### Day 1-2: TLV Infrastructure
- [ ] Create `protocols/v2/extensions/cashu/` module
- [ ] Implement TLV encoder/decoder for Cashu fields:
  - Type 0x0003_01: `locking_pubkey` (33 bytes)
  - Type 0x0003_02: `hash` (32 bytes)
- [ ] Add TLV parsing to message handlers

### Day 3-4: Rip Out Struct Modifications
- [ ] Remove `hash` and `locking_pubkey` from `SubmitSharesExtended` struct
- [ ] Remove `hash` from `SubmitSharesSuccess` struct
- [ ] Update all serialization/deserialization to use TLV
- [ ] Fix compilation errors throughout codebase

### Day 5: Basic Testing
- [ ] Update integration tests for TLV format
- [ ] Test on testnet4
- [ ] Break compatibility with old format (that's OK!)

## Phase 2: Extension Negotiation (Week 2)
**Goal:** Implement minimal viable extension negotiation

### Day 1-2: Negotiation Protocol
- [ ] Implement `RequestExtensions` and response messages
- [ ] Add extension tracking to connection state
- [ ] Hardcode Cashu extension (0x0003) as always-on for now

### Day 3-4: Message Routing
- [ ] Route `MintQuoteNotification` (0xC0) through extension handler
- [ ] Route `MintQuoteFailure` (0xC1) through extension handler
- [ ] Remove `SV2_MINT_QUOTE_PROTOCOL_DISCRIMINANT` - use extension negotiation

### Day 5: Integration
- [ ] Update pool to advertise Cashu extension
- [ ] Update translator to request Cashu extension
- [ ] Test end-to-end flow on testnet4

## Phase 3: Clean Separation (Week 3)
**Goal:** Extract all Cashu logic from core SRI protocols

### Day 1-2: Move Code
- [ ] Create `roles/cashu-extension/` crate
- [ ] Move all mint quote logic from `protocols/v2/subprotocols/`
- [ ] Move extension handler from translator to new crate
- [ ] Delete `protocols/v2/subprotocols/mint-quote/` entirely

### Day 3-4: Define Extension API
```rust
// Simple trait for now, enhance later
trait CashuExtension {
    fn append_tlv_to_share(&self, share: &mut Vec<u8>, locking_pubkey: &[u8], hash: &[u8]);
    fn extract_tlv_from_share(&self, payload: &[u8]) -> Result<(Vec<u8>, Vec<u8>)>;
    fn handle_mint_quote_notification(&self, payload: &[u8]) -> Result<()>;
}
```

### Day 5: Wire It Up
- [ ] Pool uses CashuExtension trait
- [ ] Translator uses CashuExtension trait
- [ ] Mint communication through extension only

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