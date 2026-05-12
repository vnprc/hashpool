# Upstream PR Plan: Clamp `max_target` Instead of Erroring

## Summary

When a pool's vardiff computes a target easier than the client's declared `max_target`,
`channels_sv2` currently returns `RequestedMaxTargetOutOfRange`. This leaves the channel
stuck at its current difficulty — vardiff never succeeds again for that channel.

The fix: clamp the computed target to `max_target` instead of erroring. The client declared
`max_target` as an acceptable difficulty floor, so assigning exactly that difficulty is always
protocol-correct.

**Target repo:** `https://github.com/stratum-mining/stratum`
**Crate:** `channels_sv2` (version 3.0.0 at time of writing)
**Files:** `protocols/sv2/channels-sv2/src/server/extended.rs`, `standard.rs`

---

## Root Cause

The SV2 `OpenExtendedMiningChannel` message includes a `max_target` field. Per the spec:

> `max_target`: Maximum target difficulty the pool is allowed to set for this channel.
> A larger value means easier (lower difficulty).

The pool's vardiff continuously adjusts difficulty toward a target share rate. For low-hashrate
miners, the vardiff-calculated target will naturally exceed `max_target` (easier than the floor).
The library currently errors in this case, which:

1. Prevents any further vardiff updates for that channel
2. Logs a `WARN` every ~60 seconds per affected channel
3. Leaves the miner stuck at whatever difficulty was last successfully set

---

## Exact Diff

### `src/server/extended.rs`

**Change 1: channel creation (in `fn new`)**

```diff
-        if target > max_target {
-            println!("target: {:?}", target.to_be_bytes());
-            println!("max_target: {:?}", max_target.to_be_bytes());
-            return Err(ExtendedChannelError::RequestedMaxTargetOutOfRange);
-        }
+        // Clamp to max_target rather than error. The client declared max_target as
+        // an acceptable difficulty floor, so using it when the initial target would
+        // otherwise exceed it is always valid.
+        let target = if target > max_target { max_target } else { target };
```

**Change 2: `update_channel`**

```diff
-        let new_target: Target = target;
-
-        if new_target > *requested_max_target {
-            return Err(ExtendedChannelError::RequestedMaxTargetOutOfRange);
-        }
+        // Clamp to max_target rather than error. The client declared max_target as
+        // an acceptable difficulty floor, so using it when vardiff would otherwise
+        // exceed it is always valid.
+        let new_target: Target = if target > *requested_max_target {
+            *requested_max_target
+        } else {
+            target
+        };
```

**Change 3: `test_update_channel`**

```diff
-        // Try to update with a hashrate that would result in a target exceeding the max_target
-        // new target: 2492492492492492492492492492492492492492492492492492492492492491
-        // max target: 00ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
         let very_small_hashrate = 0.1;
         let result =
             channel.update_channel(very_small_hashrate, Some(not_so_permissive_max_target));
-        assert!(result.is_err());
-        assert!(matches!(
-            result,
-            Err(ExtendedChannelError::RequestedMaxTargetOutOfRange)
-        ));
+        // Update with a hashrate that would compute a target exceeding max_target.
+        // The channel should clamp to not_so_permissive_max_target instead of erroring.
+        // calculated target: 2492492492492492492492492492492492492492492492492492492492492491
+        // max target:        00ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
+        assert!(result.is_ok());
+        assert_eq!(channel.get_target(), &not_so_permissive_max_target);
```

---

### `src/server/standard.rs`

**Change 1: channel creation (in `fn new`)**

```diff
-        let target: Target = calculated_target;
-
-        if target > requested_max_target {
-            return Err(StandardChannelError::RequestedMaxTargetOutOfRange);
-        }
+        // Clamp to max_target rather than error. The client declared max_target as
+        // an acceptable difficulty floor, so using it when the initial target would
+        // otherwise exceed it is always valid.
+        let target: Target = if calculated_target > requested_max_target {
+            requested_max_target
+        } else {
+            calculated_target
+        };
```

**Change 2: `update_channel`**

```diff
-        let new_target: Target = target;
-
-        if new_target > requested_max_target {
-            return Err(StandardChannelError::RequestedMaxTargetOutOfRange);
-        }
+        // Clamp to max_target rather than error. The client declared max_target as
+        // an acceptable difficulty floor, so using it when vardiff would otherwise
+        // exceed it is always valid.
+        let new_target: Target = if target > requested_max_target {
+            requested_max_target
+        } else {
+            target
+        };
```

**Change 3: `test_update_channel`**

```diff
-        // Try to update with a hashrate that would result in a target exceeding the max_target
-        // new target: 2492492492492492492492492492492492492492492492492492492492492491
-        // max target: 00ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
         let very_small_hashrate = 0.1;
         let result =
             channel.update_channel(very_small_hashrate, Some(not_so_permissive_max_target));
-        assert!(result.is_err());
-        assert!(matches!(
-            result,
-            Err(StandardChannelError::RequestedMaxTargetOutOfRange)
-        ));
+        // Update with a hashrate that would compute a target exceeding max_target.
+        // The channel should clamp to not_so_permissive_max_target instead of erroring.
+        // calculated target: 2492492492492492492492492492492492492492492492492492492492492491
+        // max target:        00ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
+        assert!(result.is_ok());
+        assert_eq!(channel.get_target(), &not_so_permissive_max_target);
```

---

## PR Description (suggested)

**Title:** `channels_sv2: clamp target to max_target instead of erroring`

**Body:**

When a pool's vardiff computes a target that is easier than the client's declared `max_target`,
`update_channel` currently returns `Err(RequestedMaxTargetOutOfRange)`. This silently breaks
vardiff: once triggered, no further difficulty adjustments succeed for that channel.

The `max_target` field (from `OpenExtendedMiningChannel`) means "the easiest difficulty I will
accept." If the pool's calculation produces something easier, the correct response is to assign
exactly `max_target` — not to error. The client explicitly said this difficulty is acceptable.

This fix applies the same clamping logic to both `ExtendedChannel` and `StandardChannel`, in
both `new()` (channel creation) and `update_channel()` (vardiff updates). The corresponding
tests are updated to assert clamping behavior.

Also removes two stray `println!` debug statements in `ExtendedChannel::new`.

---

## Notes

- `RequestedMaxTargetOutOfRange` error variants in both error enums can remain for the
  `UpdateChannel` message handler path (where the client itself sends a bad max_target), though
  that path also arguably benefits from clamping rather than sending an error response.
- This is a pure behavior change with no API surface changes.
- Hashpool is running this fix as a `[patch.crates-io]` override pending the upstream release.
