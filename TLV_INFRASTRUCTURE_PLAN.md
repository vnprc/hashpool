# TLV Infrastructure Development Plan

## Problem Statement
Current implementation is brittle, untested, and specific to ehash messages. We need a generic, well-tested infrastructure for handling TLV extensions on SV2 messages that can support multiple extension types.

## Core Design Principles
1. **Generic** - Work with any extension type, not just ehash
2. **Tested** - Comprehensive unit tests for all TLV operations
3. **Type-safe** - Use Rust's type system to prevent errors
4. **Zero-copy where possible** - Minimize allocations
5. **Standards-compliant** - Follow SV2 extension specifications

## Architecture

### 1. Core TLV Module (`tlv_core`)

#### Data Structures
```rust
// Generic TLV field representation
pub struct TlvField {
    pub extension_id: u16,  // 0x0003 for Cashu/ehash
    pub field_type: u16,    // Field type within extension
    pub data: Vec<u8>,      // Raw field data
}

// Extension registry for different extension types
pub trait Extension {
    const EXTENSION_ID: u16;
    fn encode_fields(&self) -> Vec<TlvField>;
    fn decode_fields(fields: &[TlvField]) -> Result<Self, TlvError>;
}

// Frame with extension data
pub struct ExtendedFrame {
    pub header: FrameHeader,
    pub core_payload: Vec<u8>,
    pub tlv_fields: Vec<TlvField>,
}
```

#### Core Functions
```rust
// Generic TLV operations
pub fn extend_frame(
    frame_bytes: &[u8], 
    extensions: &[Box<dyn Extension>]
) -> Result<Vec<u8>, TlvError>

pub fn extract_extensions(
    frame_bytes: &[u8]
) -> Result<(Vec<u8>, Vec<TlvField>), TlvError>

pub fn parse_tlv_fields(
    tlv_bytes: &[u8]
) -> Result<Vec<TlvField>, TlvError>

pub fn serialize_tlv_fields(
    fields: &[TlvField]
) -> Vec<u8>
```

### 2. Frame Parser Module (`frame_parser`)

```rust
// Parse SV2 frame structure
pub fn parse_frame_header(bytes: &[u8]) -> Result<FrameHeader, FrameError>
pub fn get_payload_bounds(bytes: &[u8]) -> Result<(usize, usize), FrameError>
pub fn rebuild_frame(header: FrameHeader, payload: &[u8]) -> Vec<u8>
pub fn update_frame_length(frame: &mut [u8], new_length: u32)
```

### 3. Extension Implementations

#### Ehash Extension (`extensions/ehash`)
```rust
pub struct EhashExtension {
    pub locking_pubkey: Option<[u8; 33]>,
    // Future fields...
}

impl Extension for EhashExtension {
    const EXTENSION_ID: u16 = 0x0003;
    
    fn encode_fields(&self) -> Vec<TlvField> {
        // Encode locking_pubkey as TLV field
    }
    
    fn decode_fields(fields: &[TlvField]) -> Result<Self, TlvError> {
        // Decode TLV fields into EhashExtension
    }
}
```

### 4. Message-Specific Handlers

```rust
// Message type registry
pub trait ExtendableMessage {
    const MESSAGE_TYPE: u8;
    fn supports_extension(ext_id: u16) -> bool;
}

impl ExtendableMessage for SubmitSharesExtended {
    const MESSAGE_TYPE: u8 = 0x1b;
    fn supports_extension(ext_id: u16) -> bool {
        ext_id == 0x0003 // Supports ehash
    }
}
```

## Implementation Steps

### Phase 1: Core Infrastructure (Week 1)
- [ ] Create `tlv_core` module with basic types
- [ ] Implement frame parser utilities
- [ ] Write comprehensive unit tests for TLV parsing
- [ ] Test with hand-crafted message bytes

### Phase 2: Extension Framework (Week 1-2)
- [ ] Define Extension trait and registry
- [ ] Implement generic extend/extract functions
- [ ] Create test extensions for validation
- [ ] Unit test all edge cases

### Phase 3: Ehash Migration (Week 2)
- [ ] Implement EhashExtension using new framework
- [ ] Migrate existing interceptor to use new infrastructure
- [ ] Integration tests with real SV2 messages
- [ ] Performance benchmarks

### Phase 4: Integration (Week 2-3)
- [ ] Update pool and translator to use new system
- [ ] End-to-end testing with mining operations
- [ ] Documentation and examples

## Testing Strategy

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    // Test TLV field encoding/decoding
    #[test]
    fn test_tlv_field_roundtrip() {}
    
    // Test frame extension with single field
    #[test]
    fn test_extend_frame_single_field() {}
    
    // Test frame extension with multiple fields
    #[test]
    fn test_extend_frame_multiple_fields() {}
    
    // Test extraction from extended frame
    #[test]
    fn test_extract_extensions() {}
    
    // Test with corrupted data
    #[test]
    fn test_corrupted_tlv_data() {}
    
    // Test frame length updates
    #[test]
    fn test_frame_length_calculation() {}
    
    // Test empty extensions
    #[test]
    fn test_empty_extension() {}
    
    // Test maximum size limits
    #[test]
    fn test_max_tlv_size() {}
}
```

### Integration Tests
```rust
#[test]
fn test_submit_shares_extended_with_ehash() {
    // Create real SubmitSharesExtended message
    // Add ehash extension
    // Verify pool can parse it
}

#[test]
fn test_multiple_extensions() {
    // Test with multiple extension types
}

#[test]
fn test_unsupported_extension() {
    // Ensure graceful handling of unknown extensions
}
```

## Example Usage

```rust
// Extending a frame
let frame_bytes = get_submit_shares_frame();
let ehash = EhashExtension {
    locking_pubkey: Some(pubkey),
};
let extended = extend_frame(&frame_bytes, &[Box::new(ehash)])?;

// Extracting extensions
let (core_frame, tlv_fields) = extract_extensions(&extended)?;
let ehash = EhashExtension::decode_fields(&tlv_fields)?;
```

## Benefits

1. **Reusable** - Same infrastructure for all extension types
2. **Testable** - Each component can be unit tested in isolation  
3. **Maintainable** - Clear separation of concerns
4. **Extensible** - Easy to add new extension types
5. **Performant** - Optimized for common cases
6. **Debuggable** - Clear error messages and logging

## Migration Path

1. Build new infrastructure alongside existing code
2. Write comprehensive tests to verify correctness
3. Gradually migrate interceptor to use new system
4. Remove old ad-hoc implementation once verified

## Future Extensions

- Payment channel updates
- Hashrate derivatives
- Custom mining templates
- Pool-specific metadata
- Performance telemetry

## Success Criteria

- [ ] Zero panics in production
- [ ] 100% unit test coverage for TLV operations
- [ ] Successfully process 1M+ shares with extensions
- [ ] Sub-microsecond TLV processing time
- [ ] Support for at least 3 different extension types