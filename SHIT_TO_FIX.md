# Technical Debt & Future Improvements

Issues and improvements deferred during development to maintain focus on core functionality.

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

### Dashboard Connection Type Heuristics

**Status:** Using activity-based detection as workaround
**Priority:** Medium - User Experience

**Current Issue:**
Connection type identification uses heuristics in `pool-stats/src/web.rs:142-154`:
- If has shares/channels → "Translator"
- Else if work_selection flag → "Job Declarator"
- Else if quotes → "Mint"

**Problem:**
- Relies on activity patterns rather than proper metadata
- The mint workaround (using port as ID) is hacky
- Can misidentify services before they've done any work

**Better Solution:**
Once mint uses proper SV2 connection:
1. Add a `connection_type` field to `SetupConnection` or use dedicated message
2. Or use proper protocol discrimination (Mining Protocol vs Job Declaration Protocol)
3. Store connection type explicitly in database from handshake

---

## Medium Priority

### Stats Service Architecture - Push vs Pull

**Status:** Currently push-based with no health monitoring
**Priority:** Medium - Operational Visibility

**Current Architecture:**
- Pool **pushes** stats events to pool-stats via newline-delimited JSON over TCP
- Translator **pushes** stats to proxy-stats (same protocol)
- Mint and JD don't report stats anywhere
- No service health monitoring or heartbeats
- Can't detect if a service crashes

**Problems:**
1. If pool crashes, pool-stats shows stale data (no "last seen" timestamp)
2. No way to query current state of a service on demand
3. Mint and JD status invisible to monitoring
4. Only pool knows about downstream connections
5. No service discovery mechanism

**Architectural Questions:**

*Option A: Keep Push, Add Heartbeats*
- Services continue pushing events
- Add periodic heartbeat messages
- Stats marks service "down" if no heartbeat in N seconds
- Simple, minimal change

*Option B: Pull-Based Health Checks*
- Stats service polls each service on interval
- Each service exposes /health endpoint (HTTP?)
- Events still pushed for real-time data
- Better failure detection

*Option C: Full SV2 Integration*
- Replace JSON-over-TCP with SV2 custom message extensions
- Stats service is SV2 server, services connect as clients
- Use SV2 Ping/Pong for health
- Noise encryption for security
- More complex but consistent with SV2-everywhere philosophy

**Research Needed:**
- Should internal services use HTTP or SV2?
- HTTP is stateless (good for health checks), SV2 is connection-oriented (good for events)
- Is hybrid approach (HTTP health + SV2 events) acceptable?
- What does Stratum v2 spec say about monitoring/observability?

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

### Time Series Data Not Being Collected

**Status:** Hashrate samples table exists but nothing writes to it
**Priority:** Medium - Needed for dashboard graphs

**Problem:**
- `hashrate_samples` table created but **0 rows** in database
- No code inserts hashrate samples periodically
- `get_hashrate_history()` function exists but returns empty results
- Can't graph hashrate over time without data

**What Needs To Happen:**
1. Add periodic sampling task to pool-stats (every 5 minutes?)
2. Sample current hashrate from `current_stats` and insert into `hashrate_samples`
3. Implement proper hashrate calculation (not just share count)
4. Add data retention policy (delete samples older than 7 days?)

**Example implementation:**
```rust
// In pool-stats main loop or separate task
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 min
    loop {
        interval.tick().await;
        if let Err(e) = db.sample_current_hashrate().await {
            error!("Failed to sample hashrate: {}", e);
        }
    }
});
```

**Current State:**
- `quote_history`: ✅ Being populated (works correctly)
- `hashrate_samples`: ❌ Empty (not being written)

### Stale Data and Reconnection Issues

**Status:** Current stats can show stale/incorrect data
**Priority:** Medium - Affects dashboard accuracy

**Problems Identified:**

**1. Reconnection Resets Counters:**
When a downstream reconnects, `record_downstream_connected` resets all counters:
```rust
INSERT INTO current_stats (...) VALUES (?1, 0, 0, 0, '[]', ?2, ?3)
ON CONFLICT(downstream_id) DO UPDATE SET
    connected_at = ?2,
    -- This resets shares, quotes, ehash to 0! ❌
```

**Impact:** If translator disconnects/reconnects, all share counts reset to 0 even though it's the same session.

**Fix:** Preserve counters on reconnect:
```rust
ON CONFLICT(downstream_id) DO UPDATE SET
    connected_at = ?2,
    is_work_selection_enabled = ?3,
    address = ?4,
    service_type = ?5
    -- Don't touch shares_submitted, quotes_created, ehash_mined
```

**2. No "Last Activity" Timestamp:**
- We track `connected_at` (static) and `last_share_time` (only for miners)
- If pool crashes, stats service has no way to detect stale connections
- Services without shares (mint, JD) have no activity indicator

**Fix:** Add `last_activity` column, update on ANY message from that downstream.

**3. Unbounded Counter Growth:**
- Counters increment forever if connection stays up
- After days/weeks, numbers lose meaning
- No daily/hourly reset mechanism

**Options:**
- A: Reset counters daily at midnight
- B: Show rates (shares/min) instead of totals
- C: Add "session stats" vs "lifetime stats"

**4. Timestamp Units Inconsistent:**
- Pool sends milliseconds (13 digits: `1759516673146`)
- Some code expects seconds (10 digits)
- Fixed in display code but underlying inconsistency remains

**Fix:** Standardize on either milliseconds or seconds throughout.

**5. No Data Retention Policy:**
Time series tables grow forever:
```sql
-- Need periodic cleanup
DELETE FROM hashrate_samples WHERE timestamp < (NOW() - 7 days);
DELETE FROM quote_history WHERE timestamp < (NOW() - 7 days);
```

---

## Low Priority

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

---

## Notes

These issues were discovered during the Phase 3 stats refactor but deferred to avoid scope creep. The mint connection issue is the most critical and should be addressed before production deployment.
