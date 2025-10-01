# Rebase Notes

## SRI Extension Feasibility Findings

- The Sv2 spec advertises TLV-based extensions, but the reference implementation was built assuming a one-to-one mapping between a frame and its core message. Once SRI deserializes a frame, any TLV payload is discarded.
- There is no mechanism to surface extension data in the existing codec/handler pipeline; the only option today is to intercept raw frames and mutate them before SRI consumes them.
- That approach forces fragile global caches or patched interceptors inside the fork and reintroduces coupling/race conditions. It is not viable for a standalone extension crate.
- A sustainable extension model would first require reworking SRI so parsed messages can carry companion `ExtensionData` alongside the core struct. Only then could Cashu logic live entirely out of tree.
- Given these constraints, the pragmatic route is to keep our fork, isolate Cashu functionality into small adapters, and rebase only after upstream support exists (or we contribute it).

