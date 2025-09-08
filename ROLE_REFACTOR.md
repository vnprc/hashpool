# Role Refactor Status

## Overview
This document tracks the refactoring of communication between roles from Redis to direct SV2 TCP messaging.

## Completed Work

### ✅ Phase 1: SV2 Message Types
- Created `protocols/v2/subprotocols/mint-quote/` with:
  - `MintQuoteRequest`: Pool → Mint quote requests
  - `MintQuoteResponse`: Mint → Pool quote responses  
  - `MintQuoteError`: Error responses
- Added message constants to `protocols/v2/const-sv2/`

### ✅ Phase 2: Direct TCP Communication
- **Removed MintPoolMessageHub abstraction** - It was the wrong approach
- Implemented direct TCP connection between pool and mint
- Pool connects to mint on startup and maintains connection
- Messages flow directly via SV2 frames over TCP

### ✅ Phase 3: Redis Removal
- **Commit `0d1b4781`**: Completely removed Redis
- **Commit `0e018639`**: Removed keyset from all messages and structs
- Eliminated Redis-based quote creation logic
- All mint quote communication now uses TCP

## Current Issues

### 🔴 Critical: Keyset Distribution Broken
**Problem**: Translator/wallet cannot mint ecash tokens because it has no keyset information.

**Error**: `No keysets available in wallet - skipping mint attempt`

**Root Cause**: 
1. Redis previously distributed keyset from mint → pool → translator
2. In commit `0e018639`, keyset was removed from all SV2 messages
3. Redis was deleted in commit `0d1b4781`
4. No replacement mechanism was implemented

**Impact**: Complete failure of ecash token minting

## Required Fix: Keyset Distribution

### Option 1: Add Keyset to MintQuoteResponse (Recommended)
**Implementation**:
1. Extend `MintQuoteResponse` to include `keyset_id` field
2. Mint includes active keyset in every quote response
3. Pool forwards keyset to translator/wallet
4. Translator stores keyset for minting tokens

**Pros**:
- Simple, follows existing message flow
- No new message types needed
- Keyset updated with every quote

**Cons**:
- Slightly increases message size
- Redundant if keyset doesn't change often

### Option 2: New Keyset Broadcast Message
**Implementation**:
1. Create new `SetActiveKeyset` message type
2. Mint broadcasts when keyset changes
3. Pool forwards to all connected translators

**Pros**:
- Clean separation of concerns
- Only sent when keyset changes

**Cons**:
- Requires new message type
- More complex implementation

### Option 3: HTTP API Query
**Implementation**:
1. Translator queries mint's HTTP endpoint for keysets
2. Uses existing CDK mint HTTP API

**Pros**:
- Uses existing infrastructure
- No SV2 protocol changes

**Cons**:
- Breaks the SV2-only communication model
- Requires translator to know mint's HTTP address

## Implementation Plan

### Step 1: Restore Keyset Communication
1. Add `keyset_id` field to `MintQuoteResponse`:
   - File: `protocols/v2/subprotocols/mint-quote/src/mint_quote_response.rs`
   - Add: `pub keyset_id: U256<'decoder>`

2. Include keyset in mint's response:
   - File: `roles/mint/src/lib/sv2_connection/quote_processing.rs`
   - Get active keyset from mint
   - Include in SV2 response

3. Forward keyset from pool to translator:
   - File: `roles/pool/src/lib/mining_pool/message_handler.rs`
   - Extract keyset from response
   - Forward to translator (needs investigation of how)

4. Store keyset in translator:
   - File: `roles/translator/src/lib/upstream_sv2/upstream.rs`
   - Receive keyset updates
   - Update wallet's keyset information

### Step 2: Test End-to-End
1. Verify mint includes keyset in responses
2. Confirm pool forwards keyset
3. Check translator receives and stores keyset
4. Test ecash token minting works

## Architecture Notes

### Current Flow
```
Pool ←→ Mint (Direct TCP, SV2 protocol)
  ↓
Translator (Needs keyset info)
```

### Missing Link
Pool receives `MintQuoteResponse` but has no way to communicate keyset to Translator.

### Investigation Needed
- How does Pool communicate with Translator?
- Is there an existing channel for Pool → Translator messages?
- Should Translator connect directly to Mint for keyset info?

## Next Steps
1. Investigate Pool → Translator communication mechanism
2. Implement chosen keyset distribution solution
3. Test complete flow from share submission to token minting
4. Update documentation with new architecture