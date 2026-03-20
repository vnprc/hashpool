# Dev Plan: mining.notify Keepalive

## Problem

The ESP-Miner firmware (and many other SV1 miners) implement a "stale work" timer that
disconnects when no new `mining.notify` is received within ~3 minutes. Bitcoin blocks average
10 minutes, so every session that spans a long inter-block period causes the miner to
disconnect and reconnect.

Current proxy behavior: no keepalive. The proxy only sends `mining.notify` when the pool
sends a new job (`NewExtendedMiningJob`) or a new block (`SetNewPrevHash` + job). Between
blocks, silence.

**Evidence**: warIsGay disconnects exactly 3 minutes after the last received `mining.notify`
(not 3 minutes after connection, and not "no accepted share" timer):
- Channel 140: last notify 23:36:02 → disconnect 23:39:03
- Channel 141: last notify 23:43:56 → disconnect 23:46:56

---

## Goal

Re-broadcast the most recent `mining.notify` every ~2 minutes when no new job has arrived.
This refreshes the miner's stale-work timer without requiring a new block.

---

## Approach

Add a periodic `tokio::time::Interval` arm to the `Sv1Server::start()` select loop. On each
tick, check how long ago the last `mining.notify` was broadcast. If it exceeds the keepalive
threshold, re-broadcast the last known notify with `clean_jobs = false`.

Store the last sent notify and its timestamp in `Sv1ServerData`.

---

## Implementation

### 1. `roles/translator/src/lib/sv1/sv1_server/data.rs`

Add two fields to `Sv1ServerData`:

```rust
use std::time::Instant;

pub struct Sv1ServerData {
    // ... existing fields ...

    /// The last notify that was broadcast to all miners, for keepalive re-broadcast.
    /// Stored as (channel_id, notify) in non-aggregated mode.
    pub last_broadcast_notify: Option<server_to_client::Notify<'static>>,

    /// Timestamp of the last time a notify was broadcast (real or keepalive).
    pub last_notify_sent_at: Option<Instant>,
}

impl Sv1ServerData {
    pub fn new(aggregate_channels: bool) -> Self {
        Self {
            // ... existing fields ...
            last_broadcast_notify: None,
            last_notify_sent_at: None,
        }
    }
}
```

**Note on aggregated vs. non-aggregated**: In non-aggregated mode (the deployed config), each
downstream has its own channel. However, in practice all downstreams receive the same job
(since they're all mining the same block template). Storing a single `last_broadcast_notify`
is sufficient — keepalive sends it to all miners via the broadcast channel with `channel_id`
matching the last job's channel, and all active downstreams receive it via the `None` downstream_id
broadcast filter.

For multi-miner deployments in non-aggregated mode where miners could be at different jobs,
a per-channel last-notify map would be more correct. This is a potential future enhancement.
The single-notify approach is correct for the single-miner or same-block scenario.

---

### 2. `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs`

#### 2a. Update `handle_upstream_message` to record last notify

In the `Mining::NewExtendedMiningJob(m)` arm, after building and broadcasting the notify,
record it in `sv1_server_data`:

```rust
Mining::NewExtendedMiningJob(m) => {
    if let Some(prevhash) = self.sv1_server_data.super_safe_lock(|v| v.prevhash.clone()) {
        let notify = build_sv1_notify_from_sv2(...)?;
        let clean_jobs = self.clean_job.load(Ordering::SeqCst);
        self.clean_job.store(false, Ordering::SeqCst);

        // ... existing job storage logic ...

        // Record last notify for keepalive
        self.sv1_server_data.super_safe_lock(|server_data| {
            server_data.last_broadcast_notify = Some(notify.clone());
            server_data.last_notify_sent_at = Some(std::time::Instant::now());
        });

        let _ = self
            .sv1_server_channel_state
            .sv1_server_to_downstream_sender
            .send((m.channel_id, None, notify.into()));
    }
}
```

#### 2b. Add keepalive interval to `start()`

In `Sv1Server::start()`, create a keepalive interval before the main loop and add a new
`select!` arm. The keepalive interval should tick frequently (e.g., every 30 seconds) to
allow timely detection, but only act when the threshold (120 seconds) has been exceeded.

```rust
pub async fn start(
    self: Arc<Self>,
    notify_shutdown: broadcast::Sender<ShutdownMessage>,
    shutdown_complete_tx: mpsc::Sender<()>,
    status_sender: Sender<Status>,
    task_manager: Arc<TaskManager>,
) -> Result<(), TproxyError> {
    // ... existing setup ...

    // Keepalive: check every 30s, re-broadcast if >120s since last notify
    const KEEPALIVE_CHECK_INTERVAL_SECS: u64 = 30;
    const KEEPALIVE_THRESHOLD_SECS: u64 = 120;
    let mut keepalive_ticker = tokio::time::interval(
        tokio::time::Duration::from_secs(KEEPALIVE_CHECK_INTERVAL_SECS)
    );
    keepalive_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // ... existing arms ...

            _ = keepalive_ticker.tick() => {
                self.maybe_send_keepalive_notify(KEEPALIVE_THRESHOLD_SECS).await;
            }
        }
    }
    // ...
}
```

#### 2c. Add `maybe_send_keepalive_notify` method

```rust
/// Re-broadcasts the last known mining.notify if no notify has been sent recently.
/// This prevents firmware's stale-work disconnect timer from firing between blocks.
async fn maybe_send_keepalive_notify(&self, threshold_secs: u64) {
    let (last_notify, elapsed) = self.sv1_server_data.super_safe_lock(|data| {
        let elapsed = data.last_notify_sent_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(u64::MAX);
        (data.last_broadcast_notify.clone(), elapsed)
    });

    if elapsed < threshold_secs {
        return; // Recent notify exists, no keepalive needed
    }

    let Some(notify) = last_notify else {
        return; // No notify ever sent (pre-first-block), nothing to repeat
    };

    // Re-broadcast the last notify with clean_jobs=false.
    // The notify already has clean_jobs=false baked into it from the original broadcast,
    // so we can re-use it directly.
    debug!("Sending keepalive mining.notify ({}s since last notify)", elapsed);

    // Use channel_id from notify's job_id to determine broadcast channel.
    // In non-aggregated mode we need a channel_id for the broadcast.
    // Extract it from the stored notify or use a sentinel value for "all channels".
    // The broadcast with channel_id=0 and downstream_id=None reaches no specific downstream
    // because no downstream has channel_id=0 assigned by the pool (pool assigns from 1+).
    // Instead, re-send with channel_id=None pattern.
    //
    // Practical approach: iterate all connected downstreams and send per-channel.
    let downstreams = self.sv1_server_data.super_safe_lock(|data| {
        data.downstreams.clone()
    });

    for downstream in downstreams.values() {
        let channel_id = downstream.downstream_data.super_safe_lock(|d| d.channel_id);
        if let Some(channel_id) = channel_id {
            // Re-send last notify tagged with this miner's current channel_id
            let _ = self
                .sv1_server_channel_state
                .sv1_server_to_downstream_sender
                .send((channel_id, None, notify.clone().into()));
        }
    }

    // Update timestamp so we don't send another keepalive for another threshold_secs
    self.sv1_server_data.super_safe_lock(|data| {
        data.last_notify_sent_at = Some(std::time::Instant::now());
    });

    info!("Keepalive mining.notify sent to {} connected miners", downstreams.len());
}
```

**Alternative for aggregated mode**: In aggregated mode all miners share one channel_id, so
a single broadcast with the aggregated channel's channel_id works. The method above handles
both modes by iterating downstreams.

---

### 3. Config (optional — make threshold configurable)

The threshold (120 seconds) can be hardcoded for now. If configurability is desired later,
add to `DownstreamDifficultyConfig` in `config.rs`:

```rust
pub struct DownstreamDifficultyConfig {
    // ... existing fields ...

    /// Seconds between keepalive mining.notify broadcasts when no new block arrives.
    /// Default: 120. Set to 0 to disable keepalives.
    #[serde(default = "default_notify_keepalive_secs")]
    pub notify_keepalive_secs: u64,
}

fn default_notify_keepalive_secs() -> u64 { 120 }
```

And in `tproxy.config.toml` / `config/prod/tproxy.config.toml`, no change needed (default
applies). If desired:

```toml
[downstream_difficulty_config]
notify_keepalive_secs = 120
```

For the initial implementation, hardcoding `120` in `sv1_server.rs` is simpler and avoids
the config struct / TOML changes.

---

## Keepalive Notify Semantics

The re-broadcast uses the **exact same notify object** that was last sent. This means:
- Same job_id, merkle root, coinbase, etc.
- `clean_jobs = false` (do not require miner to discard current work)

The miner's firmware receives an identical (or near-identical) `mining.notify` and resets its
stale-work timer without needing to switch to a new job. This is standard mining pool behavior
— many pools send keepalive notifies between blocks.

**Does this cause duplicate shares?** No. The job_id is the same. Any nonce the miner finds
will still be submitted with the same job_id. The pool accepts it (it's a valid share against
the current block template) or rejects it (stale block). No duplicate detection issue arises
because nonces are unique per ASIC search space.

---

## Interaction with UpstreamReconnected

When `UpstreamReconnected` fires (from the other fix), `Sv1ServerData.last_broadcast_notify`
and `last_notify_sent_at` should be cleared (or left as-is; both are acceptable since the
keepalive would fire quickly with the old notify, but the upstream isn't ready to accept shares
yet anyway). Safest: clear them in the `UpstreamReconnected` handler so the keepalive doesn't
fire during the reconnect window.

In the `UpstreamReconnected` arm of `sv1_server.rs`:

```rust
d.last_broadcast_notify = None;
d.last_notify_sent_at = None;
```

After the new `OpenExtendedMiningChannelSuccess` arrives and the first `NewExtendedMiningJob`
is processed, `last_broadcast_notify` and `last_notify_sent_at` are populated again.

---

## Files Changed

| File | Change |
|------|--------|
| `roles/translator/src/lib/sv1/sv1_server/data.rs` | Add `last_broadcast_notify` and `last_notify_sent_at` fields |
| `roles/translator/src/lib/sv1/sv1_server/sv1_server.rs` | Record last notify on broadcast; add keepalive interval to select loop; add `maybe_send_keepalive_notify` method |
| `config/tproxy.config.toml` _(optional)_ | Add `notify_keepalive_secs` if making configurable |
| `config/prod/tproxy.config.toml` _(optional)_ | Same |

---

## Testing

1. Start full devenv stack with a miner that has a <3-minute firmware timer (beetaxe or warIsGay)
2. Wait for a block to be found; after the notify is sent, wait >3 minutes without a new block
3. **Before fix**: miner disconnects at 3:00 mark
4. **After fix**: miner receives keepalive notify at ~2:00 mark, timer resets, miner stays connected
5. Check proxy logs for "Keepalive mining.notify sent" at the expected interval

**Success criteria**: Miner sessions extend through long inter-block periods (>5 minutes) without
disconnecting. Session duration is now bounded only by upstream reconnect events (Fix A) rather
than also by block timing.

---

## Notes

- The 120-second threshold is conservative. The firmware timer is 3 minutes (180s). 120s gives
  a 60-second safety margin. Could be reduced to 90s if tighter keepalives are desired.
- This fix does NOT help warIsGay because its hardware is broken (no valid nonces regardless
  of what job it receives). warIsGay needs physical inspection.
- This fix directly helps beetaxe during long inter-block periods on testnet4 (where blocks
  can take 10+ minutes due to lower hashrate).
