# Translator Rejected Share Error Forwarding

## Overview

When the pool validates shares and rejects them due to insufficient difficulty, the translator currently receives the error but does not forward it back to the SV1 miner that submitted the share. This document captures the design for implementing this feature.

## Current Behavior

1. **Pool-side validation** (implemented in Phase 1):
   - Pool receives SV2 share submission from translator
   - Pool validates share difficulty using `validate_share_difficulty()`
   - If share fails validation, pool returns `SubmitSharesError` with error code "share-difficulty-too-low"
   - Translator receives error in `handle_submit_shares_error()` but only logs it with a warning

2. **Translator-side handling** (current state):
   ```rust
   async fn handle_submit_shares_error(
       &mut self,
       m: SubmitSharesError<'_>,
   ) -> Result<(), Self::Error> {
       warn!("Received: {} ❌", m);  // Just logs, no forwarding
       Ok(())
   }
   ```

3. **Miner impact**:
   - SV1 miner receives no feedback on rejected shares
   - Miner has no way to know their difficulty is insufficient
   - Miner continues sending low-difficulty shares indefinitely

## Design Goals

1. **Immediate feedback**: Miners should receive rejection messages for low-difficulty shares
2. **Proper error mapping**: SV2 error codes should map to appropriate SV1 rejection reasons
3. **Correct routing**: Error must be sent back to the specific downstream connection that submitted the share
4. **Minimal performance impact**: Error forwarding should not add significant overhead

## Implementation Requirements

### 1. Share Sequence Tracking

The translator needs to correlate SV2 shares back to their SV1 originators:

**Current challenge**:
- SV1 miner sends `submit_work` message with implicit sequence (order on connection)
- Translator converts to SV2 `SubmitShareWithChannelId`
- Pool validates and returns error with sequence information
- Translator must map SV2 sequence back to original SV1 connection + request ID

**Required data structures**:
- Maintain a mapping: `(channel_id, sequence) → (downstream_connection_id, sv1_request_id)`
- This tracking must be done when shares are translated in `handle_submit_shares()`
- Mapping must be cleared after error response is sent to avoid memory leaks

### 2. Error Response Routing

**SV1 Protocol**:
- Look for rejection message type in stratum v1 protocol
- Likely candidates: `mining.submit` response with error field or separate error message
- Need to identify the correct way to send async errors to mining connections

**Required investigation**:
```rust
// Questions to answer:
// 1. What SV1 message type is used for share rejection?
// 2. How are responses routed back to specific mining connections?
// 3. Can we send unsolicited error responses, or must they be request-reply?
// 4. How is the sv1_request_id used in response routing?
```

### 3. Error Code Mapping

**Pool error codes** (from SubmitSharesError):
- `"share-difficulty-too-low"` → SV1 equivalent error reason

**Mapping table needed**:
```rust
fn pool_error_to_sv1_rejection(error_code: &str) -> &'static str {
    match error_code {
        "share-difficulty-too-low" => "low difficulty",  // or equivalent SV1 code
        // Add other pool errors as they're implemented
        _ => "unknown error",
    }
}
```

### 4. Handle Unwanted Side Effects

**Potential issues**:
- **Connection cleanup**: If a downstream connection drops, its tracked shares in the mapping must be cleaned up
- **Memory growth**: The mapping could grow unbounded if shares timeout without receiving errors
- **Timeout handling**: What happens if pool never sends error for a share? Mapping needs cleanup strategy

**Solutions**:
- Implement timeout mechanism with TTL on mapping entries (e.g., 30-60 seconds)
- Clean mapping entries when downstream connection closes
- Log warnings for shares with missing error responses

## Architecture Decision: JDC vs Translator Proxy

### Why This is Currently Low Priority

The user identified a key architectural insight: **translator proxy with this feature might be replaced by Job Declarator Client (JDC)**.

**JDC advantages for error forwarding**:
1. JDC constructs block templates, so it has all share information locally
2. Can validate shares immediately without round-trip to pool
3. Can send errors directly to miners with minimal latency
4. No need for cross-connection error mapping

**Current Translator approach**:
1. Requires complex error mapping between SV2 and SV1
2. Adds latency (error must round-trip from pool to translator to miner)
3. More complex error tracking and cleanup logic

### Implementation Priority

**Deferred** pending JDC implementation. When JDC is integrated:
1. Translator proxy may be deprecated for this use case
2. JDC will handle all share validation and error responses natively
3. If translator proxy continues to be used, this feature can be implemented then with better understanding of error handling requirements

## Implementation Path (When Needed)

If this feature is needed before JDC integration:

1. **Phase 1**: Implement share sequence tracking
   - Add mapping structure to translator channel state
   - Track downstream_id and sv1_request_id with each SV2 submission
   - Add cleanup on connection close

2. **Phase 2**: Add error response routing
   - Research SV1 protocol for proper error message format
   - Implement message send logic to specific connections
   - Handle async error responses

3. **Phase 3**: Error code mapping
   - Create mapping table for pool errors to SV1 rejections
   - Test with actual pool error scenarios

4. **Phase 4**: Robustness
   - Add timeout-based cleanup for mapping entries
   - Implement metrics for error response latency
   - Add tests for connection cleanup scenarios

## Code References

### Current implementation locations:

**Pool-side validation** (DONE):
- `/home/evan/work/hashpool/roles/pool/src/lib/share_validation.rs` - Validation logic
- `/home/evan/work/hashpool/roles/pool/src/lib/mining_pool/message_handler.rs` - Validation checks

**Translator-side** (INCOMPLETE):
- `/home/evan/work/hashpool/roles/translator/src/lib/sv2/channel_manager/message_handler.rs` - `handle_submit_shares_error()` function (just logs currently)
- `/home/evan/work/hashpool/roles/translator/src/lib/sv1/sv1_server/sv1_server.rs` - SV1 share handling (TODO comment for validation)

**Configuration**:
- `config/shared/miner.toml` - `[validation].minimum_share_difficulty_bits`
- `config/shared/pool.toml` - `[validation].minimum_share_difficulty_bits`

## Testing Considerations

When/if implemented:

1. **Unit tests**:
   - Sequence mapping with concurrent submissions
   - Mapping cleanup on connection close
   - Error code translation accuracy

2. **Integration tests**:
   - Pool rejects share → Error reaches miner
   - Multiple miners, only correct one receives error
   - Connection drop cleans up tracking data

3. **Load testing**:
   - Memory usage with thousands of pending shares
   - Impact on error response latency
   - Cleanup performance

## Related Issues

- Share validation now rejects low-difficulty shares at pool level ✅
- Translator loads validation config ✅
- **Error forwarding** (this design) - Not yet implemented
- JDC integration - Future architectural change
