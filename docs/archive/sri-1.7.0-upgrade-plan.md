Now I have all the information I need. Let me write the comprehensive plan:

Based on my thorough exploration of the codebase, I now have a clear picture of all the dependencies, coupling points, and risks. Here is the
complete phased upgrade plan.

---
Hashpool SRI 1.5.0 to 1.7.0 Upgrade Plan

Codebase Summary (What Was Found)

Current vendored protocol versions (in protocols/v2/):
- binary_sv2: 4.0.0
- codec_sv2: 3.0.1
- framing_sv2: 5.0.1
- noise_sv2: 1.4.0
- channels_sv2: 1.0.2
- mining_sv2: 5.0.1 (with hashpool-added MintQuoteNotification/MintQuoteFailure)
- roles_logic_sv2: 4.0.0 (thin facade re-exporting: channels_sv2, handlers_sv2, parsers_sv2, codec_sv2, bitcoin, vardiff)
- handlers_sv2: 0.1.0 (custom crate, not deprecated)
- parsers_sv2: 0.1.1 (custom crate, not deprecated)
- job_declaration_sv2: 5.0.1 (field: tx_ids_list)
- template_distribution_sv2: 4.0.1
- buffer_sv2: 2.0.0

Hashpool-only custom crates (do not exist upstream):
- protocols/v2/subprotocols/mint-quote/ - MintQuoteRequest/Response/Error
- protocols/v2/subprotocols/stats-sv2/ - Stats messages
- protocols/ehash/ - Ehash utility (depends on CDK + binary_sv2)
- mining_sv2::MintQuoteNotification and MintQuoteFailure (injected into upstream mining_sv2)

Critical trait naming observation: The old-style sync traits (ParseTemplateDistributionMessagesFromServer, ParseMiningMessagesFromDownstream) live
 in roles_logic_sv2::handlers and are used by pool and jd-server. The new async traits (HandleTemplateDistributionMessagesFromServerAsync,
HandleMiningMessagesFromServerAsync) live in handlers_sv2 and are used by jd-client. Both exist simultaneously in the current codebase. This is
the migration frontier.

---
Answering the Key Design Questions

Question 1: Phase order - SRI protocol upgrade vs. Bitcoin node switch

These are independent but have a coupling constraint:

The TDP (Template Distribution Protocol) wire format is what passes between bitcoind's sv2 server (port 8442) and both the pool's TemplateRx and
jd-server's listen_tp_address. The current Sjors fork speaks the SRI 1.5.0 wire format. The SRI 1.7.0 protocol crates speak the updated wire
format. If you upgrade the protocol crates first, the Sjors fork may stop communicating correctly.

The safe order is: Switch Bitcoin node first, then upgrade SRI protocol crates. Bitcoin Core v30.2 + bitcoin-core-sv2 targets SRI 1.7.0 wire
format, so you need the SRI 1.7.0 crates to speak to it anyway. This means the node switch and protocol upgrade are tightly linked and should
happen in the same phase.

Question 2: TDP compatibility risk

The TDP messages themselves (NewTemplate, SetNewPrevHash, SubmitSolution, CoinbaseOutputConstraints) have not changed names or semantics between
1.5.0 and 1.7.0 based on the release notes. The breaking changes in template_distribution_sv2 come from the version bump of its dependency chain
(binary_sv2 4→5, codec_sv2 3→4). The wire encoding for TDP messages should be identical if the same field layouts are preserved. However the Noise
 handshake and frame format changes in framing_sv2 (5→6) and codec_sv2 (3→4) in v1.6.0 could break compatibility at the connection level with
Sjors' fork. This is the primary coupling risk.

Question 3: roles_logic_sv2 deprecation strategy

Since hashpool vendors this locally, it does NOT have to remove it. The current roles_logic_sv2 in hashpool is already a thin facade that
re-exports the component crates. The cleanest approach is to:
1. Update the component crates in place (bump versions in vendored copies)
2. Keep roles_logic_sv2 as the facade it already is
3. Migrate individual services to new traits opportunistically (not all at once)

Question 4: sv2-apps integration strategy

Cherry-pick specific features only. The sv2-apps bitcoin-core-sv2 crate is the key target - it replaces the Sjors fork dependency at the system
level. The hashpool roles (pool, translator, mint, jd-server, jd-client) cannot be wholesale replaced because of deep ecash integration.

---
Phased Development Plan

---
Phase 0: Preparation and Compatibility Audit (1-2 days, independent)

Goal: Understand exactly what will break before touching any code.

Step 0.1: Create a diff of upstream SRI 1.5.0 vs 1.7.0 vendored protocol crates

Check out SRI 1.7.0 tag locally and compare each vendored crate against the hashpool version. Focus on:
- job_declaration_sv2: find tx_ids_list → wtxid_list rename (confirmed in current code at declare_mining_job.rs)
- mining_sv2: find all message type changes (7.0.0 vs 5.0.1 - two major bumps)
- channels_sv2: understand GroupChannel API changes and JobStore trait changes (get_active_job returning owned vs reference)
- buffer_sv2: find 2.0.0 → 3.0.0 breaking changes
- framing_sv2/codec_sv2: find wire-level changes that affect Sjors fork compatibility

Step 0.2: Map every file that imports from roles_logic_sv2

Already done during exploration - the full list is in 50+ .rs files across pool, translator, jd-client, jd-server, mint, and test-utils.
Categorize them by which sub-crate they actually use (via roles_logic_sv2::{channels_sv2, handlers_sv2, parsers_sv2, mining_sv2, ...}).

Step 0.3: Establish a test baseline

Run cargo test --lib --workspace from roles/ and document which tests pass. This is the regression baseline for all subsequent phases.

Decision point after Phase 0: If the TDP wire format changed in framing_sv2 5→6, then the Sjors fork switch must happen before or simultaneously
with the protocol upgrade. If wire format is compatible, then the two can be done independently.

---
Phase 1: Bitcoin Node Switch (1-2 weeks, semi-independent)

Goal: Replace Sjors' sv2-tp-0.1.17 fork with Bitcoin Core v30.2 + bitcoin-core-sv2.

This phase is independent of SRI protocol upgrades ONLY IF the Phase 0 audit shows that Sjors' fork speaks a wire format compatible with current
vendored protocol crates.

Step 1.1: Integrate bitcoin-core-sv2 into devenv

The bitcoin-core-sv2 crate from sv2-apps lives at https://github.com/stratum-mining/sv2-apps. It is a translation bridge: Bitcoin Core v30.2 IPC
(Cap'n Proto over Unix socket) → SV2 Template Distribution Protocol.

In bitcoind.nix: Replace the Sjors fork download with Bitcoin Core v30.2 official binaries. Bitcoin Core v30.0 was released October 2025 and
includes experimental IPC mining support.

In the devenv process configuration: Add a new bitcoin-core-sv2 process that bridges Bitcoin Core IPC → TDP port 8442. This replaces the built-in
sv2 support in Sjors' fork.

Architecture before:
bitcoind (Sjors fork, built-in sv2) :8442 → jd-server/pool TemplateRx

Architecture after:
bitcoind (official Core v30.x) :IPC socket → bitcoin-core-sv2 bridge → :8442 → jd-server/pool TemplateRx

Step 1.2: Update devenv.nix

Add bitcoin-core-sv2 as a new process in devenv.nix. It needs to start after bitcoind and before jd-server/pool. The bridge reads from Bitcoin
Core's IPC socket (configured by ipcbind in bitcoin.conf) and exposes TDP on port 8442.

Update config/bitcoin.conf: Remove sv2=1 and sv2port=8442 (Sjors-specific options). Add the IPC configuration needed by Bitcoin Core v30.x. The
specific flags depend on the bitcoin-core-sv2 crate's configuration API.

Step 1.3: Version pin bitcoin-core-sv2

Add bitcoin-core-sv2 to the roles workspace or as a process-binary dependency in devenv. Since it's published to crates.io from sv2-apps v0.2.0,
it can be referenced as a binary crate:

[build-dependencies]
bitcoin-core-sv2 = { version = "0.2.0" }

Or compiled and installed via the devenv tasks."build:bitcoin-core-sv2" pattern already used for cdk-cli.

Step 1.4: MSRV consideration

sv2-apps v0.2.0 requires Rust 1.85.0 MSRV. The current environment has Rust 1.86.0 from nixpkgs, so this is satisfied.

Step 1.5: Validate

- Start bitcoind (official Core v30.x)
- Start bitcoin-core-sv2 bridge
- Verify jd-server and pool TemplateRx connect successfully on port 8442
- Verify NewTemplate messages flow through
- Run a full devenv session with mining

Risk: If the bitcoin-core-sv2 bridge requires SRI 1.7.0 protocol crates (which it likely does), then this phase cannot be completed independently
- it forces the protocol upgrade. Treat this as a decision fork:
- If bitcoin-core-sv2 v0.2.0 uses SRI 1.7.0 crates from crates.io, the upgrade must happen simultaneously
- If it can be configured to speak the 1.5.0 wire format, they can be decoupled

---
Phase 2: SRI Protocol Crates Upgrade (2-4 weeks, the hardest phase)

Goal: Update all vendored protocol crates from SRI 1.5.0 versions to SRI 1.7.0 versions.

This phase follows Phase 1 (or is done simultaneously if the bitcoin-core-sv2 requirement forces it).

Step 2.1: Update the leaf crates (no application code impact)

These crates have no hashpool customizations and can be updated by copying from SRI 1.7.0:

- binary_sv2 4.0.0 → 5.0.0 (binary_codec_sv2 merged in, path structure changes)
- framing_sv2 5.0.1 → 6.0.0
- noise_sv2 1.4.0 → latest
- const_sv2 3.0.0 → latest
- buffer_sv2 2.0.0 → 3.0.0 (breaking change)

Validate: cargo build -p binary_sv2 -p framing_sv2 -p noise_sv2 -p codec_sv2

Step 2.2: Update subprotocol crates

These must be updated while preserving hashpool customizations:

common_messages_sv2 - Copy from upstream 1.7.0. No hashpool changes here.

template_distribution_sv2 - Copy from upstream 1.7.0. No hashpool changes here. Verify field layouts preserved.

job_declaration_sv2 - Copy from upstream 1.7.0. The critical change: DeclareMiningJob.tx_ids_list was renamed to wtxid_list. This field is used
in:
- /home/vnprc/work/hashpool/roles/jd-server/src/lib/job_declarator/message_handler.rs (line 87, 237, 238): message.tx_ids_list.inner_as_ref()
- /home/vnprc/work/hashpool/roles/jd-client/src/lib/channel_manager/template_message_handler.rs (line 338): tx_ids_list: tx_ids

After updating the crate, these 3 sites need mechanical rename to wtxid_list.

mining_sv2 5.0.1 → 7.0.0 - This is the most risky subprotocol update because hashpool has injected custom messages into this upstream crate:
- MintQuoteNotification (message type 0xC0)
- MintQuoteFailure (message type 0xC1)

These are defined in protocols/v2/subprotocols/mining/src/mint_quote_notification.rs and exported from mining_sv2::lib.rs. They are also
referenced in parsers_sv2 (the MintQuoteNotification/MintQuoteFailure variants in the Mining enum parser).

Strategy for custom mining_sv2 messages: Do NOT inject into the upstream crate. Instead:
1. Copy the upstream mining_sv2 7.0.0 cleanly
2. Keep the custom messages in the existing mint_quote_notification.rs file
3. Re-add the pub use mint_quote_notification::{...} line to mining_sv2::lib.rs
4. Update parsers_sv2 to re-add the MintQuoteNotification/MintQuoteFailure variants

This is the same pattern that already exists - just forward-port the customization.

Step 2.3: Update codec_sv2

codec_sv2 3.0.1 → 4.0.0. This is the Noise encryption layer and will have API changes. Update the vendored copy, fix compilation errors. The main
consumers are network_helpers_sv2 and all roles that do handshakes.

Step 2.4: Update channels_sv2

channels_sv2 1.0.2 → 3.0.0 (two major version jumps). This is the most API-disruptive update for application code.

Key changes from release notes:
- JobStore trait methods now return owned types instead of references
- Group Channel support for Extended Channels (this already exists in the current codebase - the hashpool 1.5.0 already includes GroupChannel
support per the code exploration)
- channels_sv2 2.0.0 → 3.0.0 in v1.7.0

Since the current hashpool already has GroupChannel and DefaultJobStore working, the main work is:
1. Update the method signatures in the JobStore trait to return owned types
2. Fix all callsites in pool's mining_pool/mod.rs and mining_pool/message_handler.rs where get_active_job() or similar is called

The pool is the primary consumer of channels_sv2::server::*. The translator uses channels_sv2::client::extended::ExtendedChannel. Both need to be
updated.

Step 2.5: Update handlers_sv2 and parsers_sv2

These are hashpool-custom crates (not from upstream SRI 1.5.0 directly but already in the codebase at protocols/v2/). They need to be updated to
match the new subprotocol message types.

Specifically:
- handlers_sv2 handlers need to be updated for any new message types in mining_sv2 7.0.0
- parsers_sv2 needs updating for the wtxid_list rename in DeclareMiningJob and any new mining message types

Step 2.6: Update roles_logic_sv2

roles_logic_sv2 is already a thin facade in this codebase. Update its Cargo.toml to depend on the new versions of all component crates. The src/
files (errors.rs, handlers/, utils.rs, vardiff/) should not need major changes - they mostly re-export from component crates.

Check roles_logic_sv2::utils.rs: it contains target_to_difficulty, Id, and Mutex - these are utilities not from upstream crates. These stay as-is.

Step 2.7: Fix compilation errors top-down

After updating crates, compile the full workspace and fix errors in dependency order:
1. protocols/v2/ workspace (all protocol crates)
2. common/ (stratum-common)
3. roles/roles-utils/network-helpers/
4. roles/roles-utils/rpc/
5. roles/mint/ (uses roles_logic_sv2 directly)
6. roles/pool/
7. roles/translator/
8. roles/jd-server/
9. roles/jd-client/

Validate: Full cargo build --workspace from roles/, all tests pass.

---
Phase 3: Handler Trait Migration (1-2 weeks, can be done incrementally)

Goal: Migrate the remaining services using the old-style sync handler traits to the new async handler traits. This is a code quality improvement,
not strictly required for functionality.

What uses old-style sync traits (needs migration):
- pool/src/lib/template_receiver/message_handler.rs: implements ParseTemplateDistributionMessagesFromServer
- pool/src/lib/mining_pool/message_handler.rs: implements ParseMiningMessagesFromDownstream
- jd-server/src/lib/job_declarator/message_handler.rs: implements ParseJobDeclarationMessagesFromDownstream

What already uses new async traits (no change needed):
- jd-client/src/lib/channel_manager/mod.rs: implements HandleJobDeclarationMessagesFromServerAsync, HandleMiningMessagesFromClientAsync, etc.
- jd-client/src/lib/template_receiver/: implements HandleTemplateDistributionMessagesFromServerAsync

Migration strategy: Convert pool's ParseTemplateDistributionMessagesFromServer to HandleTemplateDistributionMessagesFromServerAsync. The primary
difference is the return type changes from Result<SendTo, Error> (with the old SendTo_ enum) to Result<(), Error> (with the new trait where the
handler owns the sending logic). This requires refactoring how messages are forwarded.

This phase can be done one service at a time and is not coupled to the Bitcoin node switch. It cleans up deprecated patterns.

---
Phase 4: sv2-apps Cherry-Picking (optional, ongoing)

Goal: Pull in specific improvements from sv2-apps v0.2.0 that benefit hashpool without replacing its ecash integration.

Items worth cherry-picking:

HTTP monitoring APIs: sv2-apps adds Swagger UI-based HTTP monitoring to pool, jd-server, jd-client. Since hashpool already has web-pool and
web-proxy services with custom stats, evaluate whether the sv2-apps HTTP APIs can augment (not replace) the existing dashboard. Likely worth
adding to jd-server and jd-client which currently have no web UI.

Performance monitoring (hotpath): sv2-apps v0.2.0 adds hotpath performance monitoring. Worth cherry-picking into pool and translator.

What NOT to cherry-pick: The sv2-apps versions of pool, translator, jd-client, jd-server as complete replacements. These would eliminate all
ecash/Cashu integration and would require full reintegration of the mint protocol, MintQuoteNotification messages, ehash logic, and CDK
dependencies.

---
Risk Register and Mitigation

Risk 1: TDP wire format incompatibility with Sjors fork during Phase 2
- Severity: High
- Detection: Phase 0 audit comparing framing_sv2/codec_sv2 wire format between versions
- Mitigation: Keep a devenv branch running the Sjors fork until Phase 1 (node switch) validates the bitcoin-core-sv2 bridge

Risk 2: mining_sv2 MintQuoteNotification message type conflicts
- Severity: Medium
- Detection: Check message type byte 0xC0 against any new messages added in mining_sv2 7.0.0
- Mitigation: If 0xC0/0xC1 are taken by upstream, renumber hashpool's custom messages. The spec allows extension message types in the 0xC0+ range,
 so conflicts are unlikely but should be verified.

Risk 3: channels_sv2 JobStore owned-return breaking pool job management
- Severity: Medium
- Detection: Compile errors during Step 2.4
- Mitigation: The change is mechanical - replace &T with T at callsites, clone where necessary. The hashpool already uses
DefaultJobStore<ExtendedJob<'static>> with explicit lifetime bounds, so lifetime changes may cascade.

Risk 4: CDK dependency and Rust MSRV constraints
- Severity: Low-Medium
- Detection: The current home@0.5.11 pin and time@0.3.41 pin are already applied
- Mitigation: sv2-apps requires Rust 1.85.0; current environment is 1.86.0, so no issue. Watch for new transitive deps introduced by updated SRI
crates.

Risk 5: bitcoin-core-sv2 requires SRI 1.7.0 crates from crates.io, creating version conflicts
- Severity: High
- Detection: When adding bitcoin-core-sv2 as a dependency, Cargo will try to resolve its SRI crate deps against crates.io, conflicting with the
path-based vendored crates
- Mitigation: Add crates.io version aliases in Cargo.toml to resolve conflicts, or run bitcoin-core-sv2 as a standalone binary process (not a
library dependency) to avoid any Cargo-level version conflicts. The latter is the cleaner approach.

---
What Can Be Done Independently vs. Coupled

Fully independent (can start anytime, no risk to existing functionality):
- Phase 3 handler trait migration (pure code cleanup, no wire changes)
- Phase 4 cherry-picking of HTTP monitoring APIs from sv2-apps
- Phase 0 audit (read-only)

Tightly coupled (must be done together or in strict order):
- Phase 1 (node switch) and Phase 2 (protocol upgrade) are coupled if bitcoin-core-sv2 requires SRI 1.7.0 crates. The recommended approach: do
them in the same feature branch.
- Within Phase 2, the crate updates must follow the dependency graph: leaf crates first (binary_sv2, noise_sv2), then codec/framing, then
subprotocols (mining, job_declaration, template_distribution), then channels_sv2, then roles_logic_sv2, then application code.

Sequencing recommendation:
Phase 0 (audit, 1-2 days)
    ↓
Phase 1 + Phase 2 together (node switch + protocol upgrade, 3-6 weeks)
    ↓
Phase 3 (handler migration, 1-2 weeks, can overlap with Phase 4)
    ↓
Phase 4 (sv2-apps cherry-picks, ongoing)

---
Handling TDP Compatibility During the Transition

The safest approach during the transition window (when Phase 1 and Phase 2 are in progress):

1. Maintain two devenv configurations: The existing devenv.nix pointing at Sjors fork as the stable configuration, and a new devenv-upgrade.nix
(or a separate git branch) for the upgrade work.
2. Use the bitcoin-core-sv2 bridge as a protocol adapter: Since bitcoin-core-sv2 runs as a separate process, it can be tested against the upgraded
 protocol crates without touching the Sjors fork. If the bridge proves to speak SRI 1.7.0 wire format correctly, it is the validation that the
switch works.
3. Test point: The critical validation is that NewTemplate and SetNewPrevHash messages from the bitcoin-core-sv2 bridge are correctly decoded by
the upgraded pool/template_receiver and jd-server code. Write a specific integration test for this handshake.
4. Rollback plan: The vendored-crate approach means rollback is clean - the old versions are preserved in git history. If Phase 2 fails at any
step, reverting to the last working commit restores the Sjors-compatible state.

---
Critical Files for Implementation

- /home/vnprc/work/hashpool/protocols/v2/subprotocols/mining/src/lib.rs - Contains hashpool's custom MintQuoteNotification injection; must be
carefully forward-ported when upgrading mining_sv2 5.0.1 → 7.0.0 to preserve ecash integration
- /home/vnprc/work/hashpool/protocols/v2/subprotocols/job-declaration/src/declare_mining_job.rs - Contains the tx_ids_list field that must be
renamed to wtxid_list when upgrading job_declaration_sv2, with callsite fixes in jd-server and jd-client
- /home/vnprc/work/hashpool/roles/pool/src/lib/mining_pool/mod.rs - Primary consumer of channels_sv2 server-side APIs (GroupChannel,
DefaultJobStore, ExtendedChannel); will have the most callsite changes from channels_sv2 1.0.2 → 3.0.0 owned-return changes
- /home/vnprc/work/hashpool/bitcoind.nix - The Bitcoin node configuration that must be replaced when switching from Sjors sv2-tp-0.1.17 to Bitcoin
 Core v30.x + bitcoin-core-sv2 bridge
- /home/vnprc/work/hashpool/devenv.nix - The process orchestration that must add the bitcoin-core-sv2 bridge process, update startup sequencing,
and remove the Sjors-specific sv2 configuration

Sources:
- GitHub - stratum-mining/sv2-apps: Stratum V2 pool and miner applications
- Stratum v2 via IPC Mining Interface tracking issue
- Bitcoin Core v30.0
