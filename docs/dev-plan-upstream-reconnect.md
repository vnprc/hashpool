# Dev Plan: Upstream Reconnect Without Kicking Miners

## Problem

When the JDC↔pool SV2 connection drops and reconnects, the proxy sends
`ShutdownMessage::UpstreamReconnectedResetAndShutdownDownstreams` over the broadcast channel.
Every connected SV1 downstream task receives this signal and calls `break`, dropping their TCP
connection to the miner.

**Evidence**: Both miners disconnected 7ms apart (23:21:56.542 / 23:21:56.549), proving the
disconnect was proxy-triggered, not independent firmware behavior.

---

## Current Share Response Architecture — Existing Problem

Before addressing reconnect, there is a pre-existing architectural issue that directly
affects how we must design the reconnect share handling.

**The current share flow:**
```
1. Miner sends mining.submit
2. handle_submit() validates locally, returns true/false
3. v1 library immediately sends mining.submit result: true/false to miner  ← HERE
4. pending_share is forwarded to sv1_server
5. sv1_server forwards SubmitSharesExtended to pool
6. Pool sends SubmitSharesSuccess / SubmitSharesError

handle_submit_shares_success() { info!("✅"); Ok(()) }  ← pool response is discarded
handle_submit_shares_error()   { warn!("❌"); Ok(()) }  ← pool response is discarded
```

The miner receives its `result: true/false` in step 3, before the share even reaches the
pool. Pool-level acceptance or rejection is never communicated back to the miner. This means
the miner cannot currently verify that its accepted shares actually counted for payout.

This is a pre-existing deficiency that needs to be fixed independently of the reconnect work.
It is noted here because it constrains design choices for reconnect share handling.

**What we must NOT do in the reconnect design**: add another "optimistic ack" path where we
send `true` and then later fail to follow through. The miner's `result: true` must mean
"this share will be counted," not "this share passed local validation and we'll try our best."

---

## Goal

Keep SV1 TCP connections alive during an upstream reconnect. For shares submitted during the
reconnect window, **hold the response** until the outcome is known, then send an honest result.
Miners stay connected and their work is handled honestly.

---

## Share Handling During Reconnect — Correct Design

When the proxy is reconnecting:
1. Validate the share locally (against `d.target` and still-cached jobs)
2. **Do NOT send any response yet**
3. Buffer the share with its JSON-RPC request `id`
4. After reconnect + new job from pool:
   - Same block: translate job_id/channel_id, submit to pool, wait for pool response
   - New block: send `mining.submit result: false` with a stale-share error to the miner

Pool response routing for replayed shares:
- Pool sends `SubmitSharesSuccess` → proxy sends `mining.submit result: true` with original
  request_id to the miner
- Pool sends `SubmitSharesError` → proxy sends `mining.submit result: false` to the miner

**Sequence number / request_id tracking**: `client_to_server::Submit` already has an `id`
field (the JSON-RPC request id). `SubmitSharesExtended` has a `sequence_number`. The proxy
must maintain a mapping from `sequence_number → (downstream_id, sv1_request_id)` to route
the pool's eventual response back to the correct miner with the correct request id.

This mapping is needed for BOTH the reconnect case AND the pre-existing issue of pool
responses not reaching miners. It should be added as part of this work.

---

## Implementation

### 1. `roles/translator/src/lib/utils.rs`

Add `UpstreamReconnected` to `ShutdownMessage`:

```rust
pub enum ShutdownMessage {
    ShutdownAll,
    DownstreamShutdownAll,
    DownstreamShutdown(u32),
    UpstreamReconnectedResetAndShutdownDownstreams,  // keep existing (kick all miners)
    UpstreamReconnected,                              // NEW: keep miners, buffer shares
}
```

---

### 2. `roles/translator/src/lib/mod.rs`

Change signal sent after successful upstream restart (line 368):

```rust
// Before:
let _ = notify_shutdown_clone.send(ShutdownMessage::UpstreamReconnectedResetAndShutdownDownstreams);

// After:
let _ = notify_shutdown_clone.send(ShutdownMessage::UpstreamReconnected);
```

---

### 3. `roles/translator/src/lib/sv1/sv1_server/data.rs`

Add new fields to `Sv1ServerData`:

```rust
pub struct Sv1ServerData {
    // ... existing fields ...

    /// Shares buffered during upstream reconnect window, pending replay and response.
    /// Each entry contains the share data and the JSON-RPC request id needed to
    /// send a delayed, honest response to the miner.
    pub buffered_shares: Vec<BufferedShare>,

    /// The prevhash that was active when the upstream reconnect began.
    /// Used to detect block changes during the reconnect window.
    pub prev_hash_at_reconnect: Option<SetNewPrevHash<'static>>,

    /// True while upstream is reconnecting and channels are being re-opened.
    /// When true, incoming shares are buffered rather than forwarded.
    pub reconnecting: bool,
}

/// A share buffered during the upstream reconnect window.
pub struct BufferedShare {
    pub share: SubmitShareWithChannelId,
    /// The JSON-RPC request id from the miner's mining.submit message.
    /// Used to send the delayed response once the outcome is known.
    pub sv1_request_id: serde_json::Value,
}
```

Update `Sv1ServerData::new`:
```rust
buffered_shares: Vec::new(),
prev_hash_at_reconnect: None,
reconnecting: false,
```

---

### 4. `roles/translator/src/lib/sv2/channel_manager/channel_manager.rs`

#### 4a. Add sequence number → miner tracking

Add a mapping to route pool responses back to the correct miner:

```rust
pub struct ChannelManagerData {
    // ... existing fields ...

    /// Maps SV2 sequence_number → (downstream_id, sv1_request_id) for pending shares.
    /// Used to route SubmitSharesSuccess/Error back to the originating miner.
    pub pending_share_responses: HashMap<u32, (u32, serde_json::Value)>,
}
```

#### 4b. Handle UpstreamReconnected

```rust
Ok(ShutdownMessage::UpstreamReconnected) => {
    info!("ChannelManager: upstream reconnected, resetting channel state.");
    self.channel_manager_data.super_safe_lock(|data| {
        data.reset_for_upstream_reconnection();
        data.pending_share_responses.clear();  // stale sequence numbers
    });
    // Do NOT break.
}
```

#### 4c. Route pool responses to miners

Implement `handle_submit_shares_success` and `handle_submit_shares_error` to route the pool's
response back to the originating miner via the sv1_server broadcast channel:

```rust
async fn handle_submit_shares_success(
    &mut self,
    m: SubmitSharesSuccess,
) -> Result<(), Self::Error> {
    info!("Received: {} ✅", m);
    if let Some((downstream_id, sv1_request_id)) = self
        .channel_manager_data
        .safe_lock(|d| d.pending_share_responses.remove(&m.last_seq_num))?
    {
        // Build mining.submit result: true with the original request id
        let response = build_sv1_submit_result(sv1_request_id, true, None);
        let channel_id = /* look up channel_id for downstream_id */;
        let _ = self.channel_state.sv1_server_sender
            .send(Mining::SubmitSharesResult { downstream_id, channel_id, response })
            .await;
    }
    Ok(())
}

async fn handle_submit_shares_error(
    &mut self,
    m: SubmitSharesError<'_>,
) -> Result<(), Self::Error> {
    warn!("Received: {} ❌", m);
    if let Some((downstream_id, sv1_request_id)) = self
        .channel_manager_data
        .safe_lock(|d| d.pending_share_responses.remove(&m.sequence_number))?
    {
        let response = build_sv1_submit_result(
            sv1_request_id,
            false,
            Some(m.error_code.as_str()),
        );
        let channel_id = /* look up channel_id for downstream_id */;
        let _ = self.channel_state.sv1_server_sender
            .send(Mining::SubmitSharesResult { downstream_id, channel_id, response })
            .await;
    }
    Ok(())
}
```

**Note**: This requires a new `Mining::SubmitSharesResult` message variant (or a different
routing mechanism) to carry the delayed response back to the sv1_server, which then broadcasts
it to the correct downstream. Alternatively, the channel manager can broadcast directly on the
sv1_server broadcast channel if a handle to it is available.

**Routing options**: The cleanest way to route the delayed response back to the miner:
- Store a clone of `sv1_server_to_downstream_sender` in the channel manager (currently the
  channel manager only has `sv1_server_sender: Sender<Mining>`, not the downstream broadcast)
- Or: use the existing sv1_server_sender to carry a wrapper message type that the sv1_server
  then re-broadcasts to the downstream

A new `Mining` message variant is the most explicit approach, though it requires adding to the
Mining enum or creating a side-channel message type. This is a design decision that needs to
be made during implementation.

---

### 5. `roles/translator/src/lib/sv1/downstream/downstream.rs`

#### 5a. Handle UpstreamReconnected without breaking

```rust
Ok(ShutdownMessage::UpstreamReconnected) => {
    info!("Downstream {downstream_id}: upstream reconnecting — holding submit responses");
    self.downstream_data.super_safe_lock(|d| {
        // Clear stale difficulty/notify caches; new ones arrive after channel re-opens.
        d.cached_set_difficulty = None;
        d.cached_notify = None;
        d.pending_target = None;
        d.pending_hashrate = None;
        // Do NOT clear: channel_id (needed for job validation during reconnect window)
        // Do NOT clear: target, extranonce1
    });
    // Do NOT break — keep TCP connection alive.
}
```

#### 5b. Intercept mining.submit during reconnect before sending any response

In `handle_downstream_message`, before calling `data.handle_message`, check if we are in the
reconnect window and this is a `mining.submit`:

```rust
// Intercept mining.submit during reconnect window — hold response until outcome known
if let v1::json_rpc::Message::StandardRequest(ref request) = message {
    if request.method == "mining.submit" {
        let reconnecting = self
            .downstream_data
            .super_safe_lock(|d| d.sv1_server_data.super_safe_lock(|s| s.reconnecting));

        if reconnecting {
            // Validate locally but send NO response yet.
            // Buffer the share + request_id; response will be sent after replay.
            self.buffer_share_during_reconnect(request.id.clone(), message).await?;
            return Ok(());  // No response to miner
        }
    }
}

// Normal path: process and respond immediately
let response = self.downstream_data.super_safe_lock(|data| data.handle_message(message.clone()));
// ... rest of existing handler ...
```

Add `buffer_share_during_reconnect` helper:

```rust
async fn buffer_share_during_reconnect(
    self: &Arc<Self>,
    sv1_request_id: serde_json::Value,
    message: v1::json_rpc::Message,
) -> Result<(), TproxyError> {
    // Run through handle_message to perform validation and populate pending_share.
    // We discard the response — we will send our own delayed response later.
    let _ = self.downstream_data.super_safe_lock(|data| data.handle_message(message));

    let pending_share = self.downstream_data.super_safe_lock(|d| d.pending_share.take());

    match pending_share {
        Some(share) => {
            // Validation passed — buffer with request_id
            info!(
                "Downstream {}: buffering share during reconnect window (sv1 request_id={:?})",
                share.downstream_id, sv1_request_id
            );
            self.downstream_channel_state
                .sv1_server_sender
                .send(DownstreamMessages::BufferShare(BufferedShare {
                    share,
                    sv1_request_id,
                }))
                .await
                .map_err(|_| TproxyError::ChannelErrorSender)?;
        }
        None => {
            // Validation failed (handle_submit returned false, pending_share not set).
            // Send explicit rejection immediately.
            let response = build_sv1_submit_result(sv1_request_id, false, Some("Job not found"));
            self.downstream_channel_state
                .downstream_sv1_sender
                .send(response)
                .await
                .map_err(|_| TproxyError::ChannelErrorSender)?;
        }
    }

    Ok(())
}
```

This requires adding `BufferShare(BufferedShare)` to `DownstreamMessages` in `mod.rs`.

---

### 6. `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs`

#### 6a. UpstreamReconnected handler in main loop

```rust
Ok(ShutdownMessage::UpstreamReconnected) => {
    info!("SV1 Server: upstream reconnecting — entering share buffering mode");

    let existing_downstreams = self.sv1_server_data.super_safe_lock(|d| {
        d.prev_hash_at_reconnect = d.prevhash.clone();
        d.reconnecting = true;
        d.pending_target_updates.clear();

        if self.config.downstream_difficulty_config.enable_vardiff {
            for vardiff_state in d.vardiff.values() {
                if let Ok(mut v) = vardiff_state.write() {
                    *v = VardiffState::new().expect("Failed to reset VardiffState");
                }
            }
        }

        // DO NOT clear: d.prevhash, valid jobs (needed for share validation during window)
        // DO NOT clear: d.downstreams
        d.downstreams.clone()
    });

    // Re-open SV2 channels for all currently connected miners
    for downstream in existing_downstreams.values() {
        if let Err(e) = self.open_extended_mining_channel(downstream.clone()).await {
            error!("Failed to re-open channel after upstream reconnect: {e:?}");
        }
    }
}
```

#### 6b. Handle BufferShare in handle_downstream_message

Add arm for `DownstreamMessages::BufferShare`:

```rust
DownstreamMessages::BufferShare(buffered) => {
    self.sv1_server_data.super_safe_lock(|d| {
        d.buffered_shares.push(buffered);
    });
    info!("Buffered share during reconnect window (total buffered: {})",
        self.sv1_server_data.super_safe_lock(|d| d.buffered_shares.len()));
}
```

#### 6c. Replay buffered shares after new job

In `handle_upstream_message`, in the `Mining::NewExtendedMiningJob(m)` arm, after building
and broadcasting the notify, attempt replay:

```rust
self.replay_buffered_shares(m.channel_id, m.job_id).await;
```

```rust
async fn replay_buffered_shares(&self, new_channel_id: u32, new_job_id: u32) {
    let (buffered, prev_hash_at_reconnect, current_prevhash) =
        self.sv1_server_data.super_safe_lock(|d| {
            let buffered = std::mem::take(&mut d.buffered_shares);
            let prev = d.prev_hash_at_reconnect.take();
            let current = d.prevhash.clone();
            d.reconnecting = false;
            (buffered, prev, current)
        });

    if buffered.is_empty() {
        return;
    }

    // Determine if block changed during reconnect window
    let same_block = match (&prev_hash_at_reconnect, &current_prevhash) {
        (Some(old), Some(new)) => old.prev_hash == new.prev_hash,
        _ => false,
    };

    if !same_block {
        warn!(
            "Block changed during reconnect — {} buffered share(s) are stale. \
             Sending mining.submit result: false to miners.",
            buffered.len()
        );
        for buffered_share in buffered {
            let channel_id = buffered_share.share.channel_id;
            let downstream_id = buffered_share.share.downstream_id;
            let response = build_sv1_submit_result(
                buffered_share.sv1_request_id,
                false,
                Some("Stale share — block changed during proxy reconnect"),
            );
            // Broadcast to specific downstream via existing channel
            let _ = self
                .sv1_server_channel_state
                .sv1_server_to_downstream_sender
                .send((channel_id, Some(downstream_id), response));
        }
        return;
    }

    info!(
        "Same block during reconnect — replaying {} share(s) with new channel_id={} job_id={}",
        buffered.len(), new_channel_id, new_job_id
    );

    for buffered_share in buffered {
        let downstream_id = buffered_share.share.downstream_id;
        let sv1_request_id = buffered_share.sv1_request_id;

        let job_version = match buffered_share.share.job_version {
            Some(v) => v,
            None => {
                warn!("Buffered share missing job_version — sending false to miner");
                let response = build_sv1_submit_result(sv1_request_id, false, Some("Internal error"));
                let _ = self.sv1_server_channel_state.sv1_server_to_downstream_sender
                    .send((new_channel_id, Some(downstream_id), response));
                continue;
            }
        };

        // Translate: replace old job_id with new job_id; old channel_id with new channel_id.
        // job_id is a u32 used verbatim as the SV1 job_id string.
        let mut replay_share = buffered_share.share.share.clone();
        replay_share.job_id = new_job_id.to_string();

        let seq_num = self.sequence_counter.load(Ordering::SeqCst);

        match build_sv2_submit_shares_extended_from_sv1_submit(
            &replay_share,
            new_channel_id,
            seq_num,
            job_version,
            buffered_share.share.version_rolling_mask,
        ) {
            Ok(submit) => {
                // Register pending response tracking BEFORE sending to pool
                // (channel_manager needs to correlate seq_num → downstream response)
                // This registration is sent via channel_manager_sender as a side-band
                // message, or stored in a shared structure accessible by both the
                // sv1_server and channel_manager.
                //
                // See design note in section 4 regarding response routing mechanism.
                self.register_pending_share_response(seq_num, downstream_id, sv1_request_id.clone());

                if let Err(e) = self
                    .sv1_server_channel_state
                    .channel_manager_sender
                    .send(Mining::SubmitSharesExtended(submit))
                    .await
                {
                    error!("Failed to replay buffered share: {e:?}");
                    // Deregister and send false
                    self.deregister_pending_share_response(seq_num);
                    let response = build_sv1_submit_result(sv1_request_id, false, Some("Internal error"));
                    let _ = self.sv1_server_channel_state.sv1_server_to_downstream_sender
                        .send((new_channel_id, Some(downstream_id), response));
                } else {
                    self.sequence_counter.fetch_add(1, Ordering::SeqCst);
                    info!("Replayed buffered share seq={seq_num} → new channel_id={new_channel_id}");
                }
            }
            Err(e) => {
                error!("Failed to build SubmitSharesExtended for replay: {e:?}");
                let response = build_sv1_submit_result(sv1_request_id, false, Some("Internal error"));
                let _ = self.sv1_server_channel_state.sv1_server_to_downstream_sender
                    .send((new_channel_id, Some(downstream_id), response));
            }
        }
    }
}
```

---

### 7. `roles/translator/src/lib/sv1/downstream/downstream.rs` — receive delayed responses

The `handle_sv1_server_message` already has a "default path" that forwards any
non-notify/non-set_difficulty message directly to the miner:

```rust
// Default path: forward all other messages
self.downstream_channel_state
    .downstream_sv1_sender
    .send(message.clone())
    .await ...
```

A `mining.submit result` JSON-RPC response sent via the broadcast channel will arrive here
and be forwarded to the miner automatically. **No change needed** to `handle_sv1_server_message`
as long as the response is a `json_rpc::Message` with the correct structure.

---

### 8. `roles/translator/src/lib/sv1/sv1_server/difficulty_manager.rs`

Add arm for `UpstreamReconnected`:

```rust
Ok(ShutdownMessage::UpstreamReconnected) => {
    sv1_server_data.super_safe_lock(|d| {
        d.pending_target_updates.clear();
        for vardiff_state in d.vardiff.values() {
            if let Ok(mut v) = vardiff_state.write() {
                *v = VardiffState::new().expect("Failed to reset VardiffState");
            }
        }
        // DO NOT clear d.downstreams or d.vardiff keys
    });
    // Do NOT break.
}
```

---

## Key Design Note: Response Routing Mechanism

The biggest open design question is how to route the pool's `SubmitSharesSuccess/Error`
back to the right miner. Two options:

**Option A: Shared pending-response map**

A `HashMap<u32, (u32, serde_json::Value)>` (seq_num → downstream_id + sv1_request_id) lives
in a new `Arc<Mutex<PendingShareResponses>>` shared between the sv1_server and channel_manager.
The sv1_server registers entries before sending to the pool. The channel_manager looks them up
when responses arrive and sends the reply via a clone of `sv1_server_to_downstream_sender`.

**Option B: Extended Mining message**

Add a new message variant to the sv1_server's internal `Mining` channel:
`Mining::ShareResponseForMiner { downstream_id, channel_id, response }`. The channel_manager
sends this back to the sv1_server, which then broadcasts to the correct downstream.

Option A is cleaner architecturally (avoids extending the Mining enum). Option B avoids
adding another shared data structure. Either works; this is an implementation-time decision.

**Important**: This routing mechanism is also the fix for the pre-existing bug where pool
share responses are discarded. Once implemented for the reconnect case, it should be
activated for all share submissions, not just replayed ones.

---

## Reconnect Sequence (Honest Timeline)

```
1. JDC connection drops → UpstreamShutdown → mod.rs reconnects → sends UpstreamReconnected

2. Downstream tasks: clear stale caches, keep TCP alive, keep channel_id for job validation
3. sv1_server: save prev_hash_at_reconnect, set reconnecting=true, keep valid jobs
4. channel_manager: reset channel state, clear pending_share_responses
5. sv1_server: re-open SV2 channels for all connected miners

--- Reconnect window: miners still connected on last known job ---

6. Miner submits a share:
   - downstream intercepts mining.submit (before handle_message)
   - validates locally (channel_id still valid, jobs still cached)
   - sends BufferShare to sv1_server with sv1_request_id
   - sends NO response to miner yet

7. Pool responds with OpenExtendedMiningChannelSuccess (new channel_id)
8. Pool sends NewExtendedMiningJob (new job for new channel):
   - sv1_server broadcasts new mining.notify to miners
   - sv1_server calls replay_buffered_shares()

9a. Same block:
   - translate channel_id + job_id in each buffered share
   - register seq_num → (downstream_id, sv1_request_id) in pending response map
   - forward SubmitSharesExtended to pool
   - set reconnecting = false

9b. New block:
   - broadcast mining.submit result: false (stale) to each affected miner
   - set reconnecting = false

10a. Pool sends SubmitSharesSuccess:
    - channel_manager looks up seq_num in pending map
    - sends mining.submit result: true to miner via broadcast or shared mechanism

10b. Pool sends SubmitSharesError:
    - channel_manager looks up seq_num
    - sends mining.submit result: false (with pool error code) to miner

11. Normal operation resumes. Miner was never disconnected.
    Miner only received honest submit results.
```

---

## Timeout Protection

If the upstream takes too long to reconnect (catastrophic failure), miners are waiting for
submit responses indefinitely. Add a timeout to `replay_buffered_shares`: if `reconnecting`
is still true after (e.g.) 120 seconds, drain the buffer with `false` responses.

This can be implemented as a secondary timer arm in the sv1_server main `select!` loop:

```rust
_ = tokio::time::sleep(Duration::from_secs(120)) => {
    let still_reconnecting = self.sv1_server_data.super_safe_lock(|d| d.reconnecting);
    if still_reconnecting {
        warn!("Reconnect timed out — sending false to {} buffered shares", ...);
        self.drain_buffer_with_rejection("Proxy reconnect timed out").await;
    }
}
```

In practice this timeout fires only if the JDC can't come back up within 2 minutes, at which
point a full `ShutdownAll` is likely imminent anyway.

---

## Relationship to Pre-Existing Share Response Bug

The `pending_share_responses` map and the `SubmitSharesSuccess/Error` routing implemented
here should be the foundation for fixing the pre-existing bug where ALL pool share responses
are discarded. Once this infrastructure exists:

1. In `handle_submit_shares` (normal, non-reconnect path), register the seq_num → (downstream_id, sv1_request_id) mapping before forwarding to the pool
2. Remove the "immediate ack" from `handle_submit` — return nothing, or return a separate "pending" indicator
3. The miner gets its `result` only when the pool responds

This is a more substantial architecture change (modifying the `IsServer::handle_submit`
return behavior) and should be done as a separate follow-on PR once the reconnect handling
is in place and tested. The reconnect work lays the necessary groundwork.

---

## Files Changed

| File | Change |
|------|--------|
| `roles/translator/src/lib/utils.rs` | Add `UpstreamReconnected` variant |
| `roles/translator/src/lib/mod.rs` | Send `UpstreamReconnected` on successful reconnect |
| `roles/translator/src/lib/sv1/sv1_server/data.rs` | Add `BufferedShare`, `buffered_shares`, `prev_hash_at_reconnect`, `reconnecting` |
| `roles/translator/src/lib/sv1/downstream/mod.rs` | Add `BufferShare` to `DownstreamMessages`; add `BufferedShare` struct |
| `roles/translator/src/lib/sv1/downstream/downstream.rs` | Handle `UpstreamReconnected` without breaking; intercept submit during reconnect; add `buffer_share_during_reconnect` |
| `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs` | `UpstreamReconnected` handler; handle `BufferShare`; `replay_buffered_shares`; `drain_buffer_with_rejection` |
| `roles/translator/src/lib/sv2/channel_manager/channel_manager.rs` | Handle `UpstreamReconnected`; add `pending_share_responses` map; implement `SubmitSharesSuccess/Error` routing |
| `roles/translator/src/lib/sv1/sv1_server/difficulty_manager.rs` | Handle `UpstreamReconnected` without breaking |

---

## Testing

1. Connect a miner, confirm share submissions in pool logs
2. Trigger JDC reconnect mid-block (kill and restart JDC while miner is active)
3. Submit a share during the reconnect window
4. **Same block case**: verify `mining.submit result: true` arrives after pool confirms
5. **New block case**: verify `mining.submit result: false` (stale) arrives promptly
6. Verify miner never disconnected throughout
7. Verify no `result: true` is sent for shares that didn't get pool confirmation
