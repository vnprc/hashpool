# Sv2 Messaging Layer Development Plan

## Goal
Develop an Sv2 messaging layer between the mint role and the pool role using MPSC broadcast streams and Sv2 style encoding/data structures. This messaging layer will operate concurrently with the existing Redis flow for mint quotes.

## Current Architecture Analysis

**Current Redis Communication Flow:**
1. **Pool → Mint**: Pool creates `MintQuoteMiningShareRequest` and pushes to Redis `create_quote_prefix` queue via `RPUSH`
2. **Mint**: Polls Redis with `BRPOP` to get quote requests, processes them, and creates mint quotes
3. **Mint → Pool**: Publishes active keyset to Redis `active_keyset_prefix` key, pool reads it on startup

**Existing Sv2 Infrastructure:**
- Comprehensive Sv2 protocol implementation in `protocols/v2/`
- Binary encoding/decoding with `binary-sv2`, `serde-sv2`, `codec-sv2`
- Framing with `framing-sv2`
- Network helpers for both tokio and async-std
- Message types defined in `subprotocols/`
- MPSC broadcast patterns already used throughout the codebase

## Development Plan

### **Phase 1: Define Sv2 Message Types** ✅ **COMPLETED**
**Goal**: Create new Sv2 message types for mint quote operations

#### 1.1 Create Mint Quote Messages (`protocols/v2/subprotocols/mint-quote/`) ✅
- **New message types**:
  - `MintQuoteRequest` - Pool requests quote from mint
  - `MintQuoteResponse` - Mint responds with quote details
  - `MintQuoteError` - Error response from mint
- **Message fields**:
  - Request: `amount`, `unit`, `header_hash`, `description`, `locking_key` (NUT-20), `keyset_id` 
  - Response: `quote_id`, `amount`, `unit`, `expires_at`, `state`
  - Error: `error_code`, `error_message`
- **Binary encoding/decoding** using existing Sv2 codec infrastructure
- **Fixed**: Used `Sv2Option` for optional fields, `CompressedPubKey` for locking key

#### 1.2 Add Message Type Constants ✅
- Update `protocols/v2/const-sv2/src/lib.rs` with new message type IDs (0x80-0x82)
- Define mint-quote subprotocol constants  
- Added protocol discriminant (SV2_MINT_QUOTE_PROTOCOL_DISCRIMINANT = 3)
- Set channel bits to `true` for inter-role communication

**Build & Test**: ✅ Protocols workspace compiles successfully, mint-quote protocol builds without errors

### **Phase 2: Implement Core Messaging Infrastructure** ✅ **COMPLETED**
**Goal**: Create the foundational messaging layer using MPSC channels

#### 2.1 Create Mint-Pool Message Hub (`roles/roles-utils/mint-pool-messaging/`) ✅
- **MintPoolMessageHub** - Central coordinator for mint-pool communication
- **MPSC broadcast channels**:
  - `quote_request_tx/rx` - Pool → Mint quote requests
  - `quote_response_tx/rx` - Mint → Pool quote responses  
  - `quote_error_tx/rx` - Mint → Pool error responses
- **Channel management**: Connection tracking, role-based registration (Pool/Mint)
- **Configuration**: Configurable buffer sizes, timeouts, retry policies

#### 2.2 Supporting Components ✅
- **ChannelManager** - Handles connection lifecycle and channel management
- **MessageCodec** - Message type handling and SV2 constants integration  
- **Role enum** - Pool/Mint role identification
- **MessagingConfig** - Configurable messaging parameters
- **Error handling** - Custom error types with proper error propagation

#### 2.3 Key Features Implemented ✅
- Async/await support with Tokio runtime
- Broadcast channels for 1-to-many messaging patterns
- Connection registration/unregistration with activity tracking
- Timeout handling for message operations
- Statistics and monitoring capabilities
- Configurable buffer sizes and retry mechanisms

**Build & Test**: ✅ All components compile successfully, mint-pool messaging crate builds without errors

### **Phase 3: Integrate with Pool Role** ✅ **COMPLETED**
**Goal**: Add Sv2 messaging to pool without breaking existing Redis functionality

#### 3.1 Pool-Side Integration (`roles/pool/`) ✅
- **Extended PoolSv2 struct** with SV2 messaging configuration and hub
- **Dual-path implementation**: Redis + SV2 messaging running concurrently 
- **Message sender**: `send_sv2_mint_quote()` converts mining shares to SV2 MintQuoteRequest
- **SV2 Hub Integration**: Pool creates and registers with MintPoolMessageHub on startup

#### 3.2 Pool Message Handler Updates ✅
- **Modified** `roles/pool/src/lib/mining_pool/message_handler.rs`
- **Added** `create_and_enqueue_mining_share_quote()` to send via both Redis and SV2
- **Implemented** proper data conversion: keyset ID padding, locking key handling, SV2 type creation
- **Fixed** lifetime issues by converting to static references for async tasks

#### 3.3 Configuration & Infrastructure Updates ✅
- **Added** Sv2MessagingConfig to `shared_config` with broadcast/MPSC buffer sizes, retries, timeout
- **Extended** `config/shared/pool.toml` with `[sv2_messaging]` section 
- **Updated** Pool struct to store sv2_hub and sv2_config, pass to Downstream instances
- **Enhanced** main.rs to initialize MintPoolMessageHub and register pool connection

#### 3.4 Key Implementation Details ✅
- **Data Conversion**: Fixed keyset ID padding (8→32 bytes for U256), proper SV2 type construction
- **Async Handling**: Spawned background tasks for SV2 message sending without blocking Redis
- **Error Handling**: SV2 failures don't break Redis functionality, proper error logging
- **Testing**: Redis can be disabled to test SV2-only operation

**Build & Test**: ✅ Pool compiles successfully, SV2 messages sent successfully to broadcast channels

**Current Status**: Pool successfully sends SV2 mint quote messages, but mint doesn't receive them because:
- Pool and mint are separate processes with separate MintPoolMessageHub instances
- Need to implement Phase 4 (Mint integration) or add inter-process communication mechanism

### **Phase 4: Integrate with Mint Role**  
**Goal**: Add Sv2 messaging to mint without breaking existing Redis functionality

#### 4.1 Mint-Side Integration (`roles/mint/`)
- **Extend mint main.rs** with Sv2 message listener
- **Dual-path implementation**: Keep Redis polling + add Sv2 message handling
- **Request processor**: Handle incoming Sv2 quote requests
- **Response sender**: Send Sv2 quote responses back to pool

#### 4.2 Mint Message Processing
- Create Sv2 quote request handler parallel to existing `handle_quote_payload`
- Convert between Sv2 message types and CDK types
- Maintain same quote creation logic using `mint.create_mint_mining_share_quote`

**Build & Test**: Compile mint role and fix any issues

### **Phase 5: Final Integration and Testing**
**Goal**: Ensure the complete system works with both Redis and Sv2 messaging

#### 5.1 End-to-End Functionality
- Test complete message flow: Pool → Sv2 → Mint → Sv2 → Pool
- Verify Redis continues to work unchanged
- Test error scenarios and recovery

#### 5.2 Build Validation
- Final compilation of entire workspace
- Address any remaining type mismatches or dependency issues
- Verify both messaging paths work concurrently

**Build & Test**: Full workspace compilation and basic functionality verification

## Implementation Sequence with Build Validation

1. **Phase 1** → Build and fix protocols workspace compilation
2. **Phase 2** → Build and fix roles workspace compilation  
3. **Phase 3** → Build and fix pool role compilation
4. **Phase 4** → Build and fix mint role compilation
5. **Phase 5** → Full workspace compilation and functionality test

## Key Design Principles

- **Non-Breaking**: Maintain full Redis compatibility during development
- **Minimal Changes**: Leverage existing Sv2 infrastructure extensively
- **Incremental**: Build and validate after each component
- **Concurrent**: Sv2 messaging operates alongside Redis
- **Standards-Compliant**: Follow existing Sv2 patterns and conventions
- **Config in pool.toml**: All configuration changes go in `config/shared/pool.toml`

## Commands for Development

After each phase, run:
```bash
# Build the relevant workspace
cd protocols && cargo build
cd roles && cargo build

# Or build specific components
cd roles/pool && cargo build
cd roles/mint && cargo build
```

This plan ensures systematic implementation with clear incremental goals and build validation at each step, while maintaining system reliability.