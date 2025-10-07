# Stats Transport Protocol Decision

## Question

Should we use **raw TCP with JSON** or **HTTP POST** for sending stats snapshots from hub services (pool/translator) to stats services?

## Context

Hub services (pool, translator) need to push snapshot data every 5 seconds to stats services (stats-pool, stats-proxy). This is:
- **Unidirectional** - hub → stats only, no response needed
- **Fire-and-forget** - if one snapshot is lost, next arrives in 5s
- **Low-stakes** - stats are informational, not critical
- **Localhost only** - all services run on same machine

## Options Considered

### Option A: JSON over Raw TCP (Newline-Delimited)

**Implementation:**
```rust
// In roles-utils (pure hashpool)
pub struct StatsClient<T> {
    stream: Arc<Mutex<Option<TcpStream>>>,
    server_address: String,
}

impl<T: Serialize> StatsClient<T> {
    async fn send_snapshot(&self, snapshot: T) {
        let json = serde_json::to_vec(&snapshot)?;
        stream.write_all(&json).await?;
        stream.write_all(b"\n").await?;  // Newline delimiter
    }
}
```

**Hub service (pool/translator) overhead:**
- Code: 2 lines in main.rs
- Dependencies: **ZERO** (uses existing tokio, serde_json)
- Binary size: +0 bytes
- Compile time: +0 seconds

**Pros:**
- ✅ Zero dependency overhead in SRI code
- ✅ Minimal binary size impact
- ✅ Simple implementation (~100 lines in roles-utils)
- ✅ Easy to debug with `nc` and `tcpdump`
- ✅ Direct connection management

**Cons:**
- ❌ Manual TCP connection handling
- ❌ Need newline framing protocol
- ❌ Slightly less standard than HTTP

---

### Option B: JSON over HTTP POST

**Implementation:**
```rust
// In roles-utils (pure hashpool)
pub struct StatsClient<T> {
    client: reqwest::Client,
    endpoint_url: String,
}

impl<T: Serialize> StatsClient<T> {
    async fn send_snapshot(&self, snapshot: T) {
        self.client
            .post(&self.endpoint_url)
            .json(&snapshot)
            .send()
            .await?;
    }
}
```

**Hub service (pool/translator) overhead:**
- Code: 2 lines in main.rs
- Dependencies: **reqwest + deps** (~15 crates)
- Binary size: +1-2 MB
- Compile time: +30-60 seconds (first build)

**Pros:**
- ✅ Standard HTTP protocol
- ✅ Automatic connection pooling
- ✅ Built-in timeout/retry handling
- ✅ Easy to test with `curl`
- ✅ Stats services already have HTTP servers

**Cons:**
- ❌ Adds reqwest dependency to pool/translator
- ❌ Larger binary size
- ❌ Longer compile times
- ❌ More overhead per request (HTTP headers)

---

## Decision: Use Raw TCP with JSON

**Rationale:**

### Primary Reason: Minimize SRI Code Dependencies

Per REBASE_SRI.md, our goal is to minimize changes and dependencies in SRI fork code (pool, translator).

**TCP approach:**
- Pool/Translator: Zero new dependencies
- All complexity in roles-utils (pure hashpool code)
- Minimal binary size impact on SRI services

**HTTP approach:**
- Pool/Translator: Must add reqwest dependency
- Increases SRI binary size by ~2MB
- Adds 30-60s to SRI compile time

### Secondary Reasons:

1. **Connection semantics match use case**
   - TCP: Long-lived connection, periodic sends
   - HTTP: Request/response model (not needed here)

2. **Simplicity**
   - Don't need HTTP's features (headers, methods, status codes)
   - Just need: serialize → send → done

3. **Debugging**
   - Can use `nc localhost 8082` to see raw JSON
   - Can use `tcpdump` to inspect traffic
   - Same debugging capability as HTTP

4. **Performance**
   - No HTTP parsing overhead
   - No header overhead (saves ~200 bytes/request)
   - Direct socket write

## Implementation Details

### Framing Protocol

Use newline-delimited JSON (NDJSON):
```
{"ehash_balance":1000,...}\n
{"ehash_balance":1001,...}\n
```

**Why newlines work:**
- JSON spec forbids unescaped newlines in strings
- Simple to parse: read until `\n`
- Industry standard (JSONLines, NDJSON)

### Connection Management

**Hub side:**
- Create TCP connection on startup
- If connection fails, retry on next poll (5s later)
- No complex reconnection logic needed

**Stats side:**
- Accept TCP connections
- Read newline-delimited messages
- Store each snapshot in DB

### Error Handling

**Send failures:**
- Log warning
- Drop connection
- Retry on next poll (5s later)

**Parse failures:**
- Log error with malformed JSON
- Skip message
- Continue reading stream

## Alternative Considered: Unix Domain Sockets

We also considered UDS instead of TCP:

**Pros:**
- Faster (no network stack)
- File permissions for security

**Cons:**
- Can't use `nc` for testing
- Harder to debug remotely
- Windows compatibility concerns

**Decision:** Stick with TCP for better debugging experience. Performance difference negligible for 5s interval.

## Testing Strategy

**Unit tests:**
- Test JSON serialization/deserialization
- Test TCP client can send data
- Mock server receives correct JSON

**Smoke tests:**
```bash
# Start stats service
cargo run --bin stats-proxy

# Send test snapshot
echo '{"ehash_balance":500,"upstream_pool":null,"downstream_miners":[],"timestamp":123}' | nc localhost 8082

# Verify received
curl http://localhost:8082/api/stats
```

**Integration tests:**
- Full devenv stack
- Verify snapshots flow through
- Test reconnection after service restart

## Summary

**Use TCP with newline-delimited JSON** because:
1. **Zero dependency overhead in SRI code** (primary driver)
2. Simple, direct implementation
3. Easy to debug
4. Sufficient for localhost, fire-and-forget use case

HTTP would be fine too, but the dependency overhead in pool/translator (SRI fork code) makes TCP the better choice for this architecture.
