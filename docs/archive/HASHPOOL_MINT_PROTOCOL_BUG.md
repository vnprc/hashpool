# Hashpool Mint Protocol Parsing Bug - Investigation Report

## Summary
The translator service crashes when receiving certain messages from the mint, causing shares to stop being accepted despite the miner remaining connected. This is a critical protocol parsing/framing issue.

## Symptoms
1. **Shares stop being received** - No error logged, just silent failure
2. **Miner remains connected** - The upstream connection stays active, no disconnection
3. **No logged errors in share handling** - The vardiff loop continues but share submissions aren't processed
4. **Translator crashes with panic** - Eventually crashes on malformed message parsing
5. **Pool dashboard shows sparse data** - Shares before crash are recorded, then gap in data

## Error Messages

### Primary Error (from proxy.log)
```
ERROR: Received frame with invalid payload or message type:
  Sv2Frame {
    header: Header {
      extension_type: 32768 (0x8000),
      msg_type: 31 (0x1f),
      msg_length: U24(437)
    },
    payload: None,
    serialized: Some(...)
  }
```

### Secondary Error (Panic Stack)
```
thread 'tokio-runtime-worker' panicked at
/home/evan/work/hashpool/protocols/v2/subprotocols/mining/src/mint_quote_notification.rs:6:21:
range start index 243 out of range for slice of length 49
```

Stack trace shows the panic originates from:
- `mining_sv2::mint_quote_notification::impl_parse_decodable_mintquotenotification`
- Called from: `binary_codec_sv2::codec::decodable::Decodable::from_bytes`
- Called from: `binary_codec_sv2::from_bytes`
- Called from: `translator_sv2::sv2::channel_manager::channel_manager::ChannelManager::handle_upstream_message`

## Root Cause Analysis

### Message Type Identification
- **msg_type 31 (0x1f)**: `NewExtendedMiningJob` (defined in `const-sv2/src/lib.rs`)
- **Extension type 32768 (0x8000)**: Appears to be malformed or incorrect
- **Message length**: 437 bytes
- **Actual payload received**: Only 49 bytes

### The Mismatch
1. Frame header says msg_type 31 = `NewExtendedMiningJob`
2. But the parser is attempting to parse as `MintQuoteNotification` (msg_type 0xC0)
3. `MintQuoteNotification` structure expects:
   - channel_id (4 bytes)
   - quote_id (1 byte length + variable string)
   - amount (8 bytes)
4. Parser is trying to read byte 243 from a 49-byte message
5. This indicates the **frame boundaries are wrong** or **message type routing is incorrect**

### Relevant Code Locations
- **Message definition**: `protocols/v2/subprotocols/mining/src/mint_quote_notification.rs` (lines 1-46)
- **Message parsing**: `protocols/v2/parsers-sv2/src/lib.rs` - Mining enum
- **Frame handling**: `translator_sv2/src/lib/sv2/channel_manager/channel_manager.rs:212`
- **Error handling**: `translator_sv2/src/lib/utils.rs` - "Received frame with invalid payload" error

## Data Flow
```
mint → [SV2 frame with msg_type 31] → translator upstream handler
         ↓
    Parsed as NewExtendedMiningJob? OR MintQuoteNotification?
         ↓
    Frame boundaries corrupted or payload incomplete
         ↓
    Parser tries to read beyond available bytes
         ↓
    PANIC
```

## Known Issues from Logs

### Session Timeline (from proxy.log)
- 00:31:46 - Share submitted successfully (SubmitSharesExtended)
- 00:31:48 - More shares submitted successfully
- 00:31:49 - "Creating local record for mining share quote" (CDK operations)
- **00:32:22 - ERROR: Invalid frame received**
- 00:32:22 - "All downstreams removed from sv1 server as upstream reconnected"
- 00:33:22+ - "Channel id is none for downstream_id" errors (cascading failures)

The upstream connection received a malformed frame at 00:32:22, causing:
1. Translator to reject the frame
2. Upstream connection to reset
3. All downstream channels to be invalidated
4. Subsequent shares to fail channel lookup

## Troubleshooting Steps

### Step 1: Enable Frame Logging
Modify `translator_sv2/src/lib/utils.rs` to log raw frame bytes before parsing:

```rust
// Where error occurs:
// "Received frame with invalid payload or message type"

// Add logging of raw payload:
if let Some(serialized) = &frame.serialized {
    tracing::warn!("Raw frame bytes (first 100): {:?}", &serialized[..100.min(serialized.len())]);
    tracing::warn!("Frame header: ext_type={}, msg_type={}, len={}",
        frame.header.extension_type,
        frame.header.msg_type,
        frame.header.msg_length);
}
```

### Step 2: Identify Message Source
Check which message source is sending the malformed frame:
- Is it from `pool` (upstream mining work)?
- Is it from `mint` (quote notifications)?
- Is it from `jds` (template distribution)?

Add context to the error:
```rust
tracing::error!("Invalid frame from upstream: ext_type={}, msg_type={}, payload_len={}, expected_type={:?}",
    frame.header.extension_type,
    frame.header.msg_type,
    frame.serialized.as_ref().map(|s| s.len()).unwrap_or(0),
    parse_message_type(frame.header.msg_type)
);
```

### Step 3: Verify Frame Encoding
Check if the mint is encoding frames correctly:
- Are SV2 frame headers properly formatted?
- Is the msg_length field correct?
- Are extension_type values valid?

From the error, extension_type 32768 (0x8000) is suspicious - this looks like:
- High bit set: Could indicate protocol version or flag
- But this field should contain valid message routing info

### Step 4: Check Message Routing
The error suggests the translator is trying to parse a NewExtendedMiningJob (msg_type 31) as a MintQuoteNotification (msg_type 0xC0).

Verify the routing in `channel_manager.rs`:
```rust
// Ensure message type dispatch is correct:
match frame.header.msg_type {
    0x1f => parse as NewExtendedMiningJob,
    0xC0 => parse as MintQuoteNotification,
    // etc
}
```

### Step 5: Bounds Checking
The panic `range start index 243 out of range for slice of length 49` suggests:
- The Deserializer is trying to parse more data than exists
- Either the frame header's `msg_length` is wrong
- Or the serialized payload is incomplete

Check in `binary_codec_sv2`:
```rust
// When reading from a slice of length 49:
// If trying to access byte 243, something is very wrong with:
// 1. How the slice was created
// 2. How the length was calculated
// 3. The message definition (too many fields)
```

## Next Steps (Priority Order)

### 🔴 Critical - Prevent Crashes
1. Add try-catch around the MintQuoteNotification parsing
2. Log raw frame bytes when parsing fails
3. Don't panic on malformed frames - log and skip/disconnect gracefully

### 🟡 High - Root Cause Analysis
1. Run with frame logging enabled
2. Capture the exact raw bytes being sent by mint
3. Compare expected frame structure vs actual
4. Check if msg_length in header matches actual serialized length

### 🟢 Medium - Validation
1. Verify SV2 frame encoding in mint integration
2. Check frame boundaries in translator's upstream receiver
3. Validate message type routing logic
4. Test with different message types from mint

## Files to Investigate

1. **Frame Handling**:
   - `translator_sv2/src/lib/sv2/channel_manager/channel_manager.rs:212`
   - `translator_sv2/src/lib/utils.rs`

2. **Message Definitions**:
   - `protocols/v2/subprotocols/mining/src/mint_quote_notification.rs`
   - `protocols/v2/parsers-sv2/src/lib.rs` (Mining enum)

3. **Deserialization**:
   - `protocols/v2/binary-sv2/codec/src/codec/decodable.rs`
   - Check how Str0255 lengths are read

4. **Mint Integration**:
   - Look for where mint messages are being framed
   - Check if extension_type 0x8000 is defined anywhere
   - Verify mint protocol compliance

## Logs Captured

**Full error output:**
```
ERROR translator_sv2::utils: Received frame with invalid payload or message type:
Sv2Frame {
  header: Header {
    extension_type: 32768,
    msg_type: 31,
    msg_length: U24(437)
  },
  payload: None,
  serialized: Some(Slice { offset: 0x7abc9a55e48c, len: 443, ... })
}

thread 'tokio-runtime-worker' panicked at
protocols/v2/subprotocols/mining/src/mint_quote_notification.rs:6:21:
range start index 243 out of range for slice of length 49
```

**Timeline from logs:**
- Shares processing fine until 00:32:22
- Invalid frame error at 00:32:22
- Upstream reset triggers downstream cleanup
- Cascade of "Channel id is none" errors follow

## Hypothesis

The mint is sending a NewExtendedMiningJob frame (msg_type 31) with:
- Incorrect extension_type (32768 instead of valid value)
- Header specifies 437 bytes, but only 49 are available
- The frame is being malformed somewhere in the mint→translator transmission

Alternatively:
- Frame boundaries are being corrupted by the network layer
- The translator's frame parser is reading from the wrong offset
- There's a desync between expected and actual frame format
