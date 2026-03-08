 Hashpool SRI 1.7.0 Upgrade Plan (v2.3)

 Context

 Hashpool runs on a fork of the Stratum V2 Reference Implementation (SRI) 1.5.0 with vendored protocol crates and a Sjors-fork bitcoind as the template
 provider. The goal is to get ehash minting working on SRI 1.7.0, which requires:
 1. Migrating vendored SRI protocol crates from path dependencies to crates.io imports at 1.7.0 versions (eliminates maintenance burden of vendored copies)
 2. Preserving the hashpool-custom ecash layer (ehash, mint-quote, stats-sv2 protocol crates + the MintQuoteNotification messages)
 3. Eventually switching the Bitcoin template provider from the Sjors fork to sv2-tp + Bitcoin Core 30 (deferred — see Phase 1 note below)

 Key architectural insight: The only vendored SRI crate with hashpool modifications is mining_sv2 (injected MintQuoteNotification/MintQuoteFailure at
 0xC0/0xC1) and the dependent parsers_sv2/handlers_sv2 crates. All other SRI crates can be replaced with crates.io imports, reducing the vendored codebase to
  ~1,500 LOC of custom hashpool code.

 Execution approach: Fork to a feature branch before Phase 2. Validate at each step before proceeding. The Sjors fork remains as the TP throughout Phase 2.

 ---
 SRI Version History

 | Crate                 | SRI 1.5.0 | SRI 1.6.0 (Nov 2024)  | SRI 1.7.0 (Jan 2025)  |
 |-----------------------|-----------|-----------------------|-----------------------|
 | binary_sv2            | 4.0.0     | **5.0.0** (major)     | 5.0.1 (patch)         |
 | framing_sv2           | 5.0.1     | **6.0.0** (major)     | 6.0.1 (patch)         |
 | codec_sv2             | 3.0.1     | **4.0.0** (major)     | 4.0.1 (patch)         |
 | channels_sv2          | 1.0.2     | **2.0.0** (major)     | **3.0.0** (major)     |
 | mining_sv2            | 5.0.1     | **6.0.0** (major)     | **7.0.0** (major)     |
 | parsers_sv2           | 0.1.1     | 0.2.0                 | 0.2.1 (patch)         |
 | handlers_sv2          | 0.1.0     | 0.2.0                 | 0.2.1 (patch)         |
 | roles_logic_sv2       | 4.0.0     | **DEPRECATED**        | removed from repo     |

 Why 1.7.0 directly (not a 1.6 intermediate):
 - Most breaking changes are concentrated in 1.5→1.6 (binary, framing, codec, channels 1→2, mining 5→6). A 1.6
   stop would not reduce the total API surface changed — the same compile errors need fixing either way.
 - channels_sv2 has two distinct major bumps (1→2 at 1.6, 2→3 at 1.7). They are isolated in Tier D (Step 2.6)
   so they are debugged separately without needing a 1.6 version stop.
 - roles_logic_sv2 was deprecated at 1.6.0. Stopping there means dealing with deprecation noise that 1.7.0
   has already resolved.
 - 1.6→1.7 is mostly patches. The only non-patch changes (channels_sv2 3.0.0, mining_sv2 7.0.0 wtxid rename)
   are already explicitly addressed in Steps 2.5–2.7.
 - A 1.6 intermediate would require editing every Cargo.toml twice (once to 1.6 versions, once to 1.7).
   The tier-based checkpoints in Steps 2.2–2.7 provide fine-grained isolation without that overhead.

 ---
 Phase 0: Audit (Day 1, prerequisite)

 Goal: Establish a baseline and confirm what is modified vs unmodified in the vendored crates.

 Step 0.1 — Confirm which vendored crates are unmodified

 Run a diff of each protocols/v2/ crate against the corresponding SRI 1.5.0 tag. Expected result:
 - Unmodified: binary_sv2, const_sv2, framing_sv2, noise_sv2, buffer_sv2, common_messages_sv2, template_distribution_sv2, job_declaration_sv2, channels_sv2,
 roles_logic_sv2
 - Modified: mining_sv2 (added mint_quote_notification.rs, exports MintQuoteNotification/MintQuoteFailure), parsers_sv2 (added Mining enum variants),
 handlers_sv2 (handlers for custom messages)

 Step 0.2 — Confirm message type byte conflicts

 Check SRI 1.7.0 mining_sv2 message type definitions. Confirm that 0xC0 and 0xC1 are not taken by any new upstream messages. (Extension range 0xC0+ is
 reserved for custom use in the spec, so conflicts are unlikely.)

 Step 0.3 — Establish compile baseline

 cd roles && cargo build --workspace 2>&1 | tee /tmp/baseline-build.log
 cd roles && cargo test --lib --workspace 2>&1 | tee /tmp/baseline-test.log
 Document any pre-existing warnings/failures to distinguish from regressions.

 Step 0.4 — Research Bitcoin Core 30 + bitcoin-core-sv2 (COMPLETED)

 Findings (Phase 0 audit, 2026-03-06):
 - bitcoin-core-sv2 is a LIBRARY crate (no binary targets), not a standalone bridge process.
   The sv2-apps pool embeds it directly and has pool-config-bitcoin-core-ipc-example.toml for
   direct IPC mode. There is no standalone TDP bridge binary in the SRI 1.7.0 ecosystem.
 - Bitcoin Core 28/30 IPC is enabled with: -ipcbind=unix:<socket_path>
   Socket path for regtest: <datadir>/regtest/node.sock (conventional)
 - capnproto is required at build time for the bitcoin-core-sv2 library (add pkgs.capnproto to devenv)
 - The sv2-apps pool's direct IPC mode requires SRI 1.7.0 crates, which the current hashpool
   pool role does not have (SRI 1.5.0). Direct IPC in the pool is a Phase 2 item.
 - Phase 1 solution: write a thin standalone TP binary in a new tp/ mini-workspace that uses
   bitcoin-core-sv2 as a library dep and serves standard SV2 TDP over TCP on port 8443.
   This keeps Phase 1 non-invasive to existing roles and enables end-to-end validation before
   the Phase 2 crate migration.

 Critical files for this phase:
 - protocols/v2/subprotocols/mining/src/mint_quote_notification.rs — custom messages to preserve
 - protocols/v2/subprotocols/mining/src/lib.rs — injection point in mining_sv2

 ---
Phase 1: Bitcoin Core 30 + sv2-tp (Required before Step 2.13 — TP is now incompatible with SRI 1.7.0)

Decision (2026-03-06): Phase 1 was deferred, but the TP compatibility failure diagnosed on
2026-03-08 has changed this. The Sjors fork (sv2-tp-0.1.17) cannot parse SRI 1.7.0's
6-byte CoinbaseOutputConstraints — it was built against the SRI 1.5.0 era 4-byte format.
See the TP Compatibility Failure section below for root cause analysis and fix options.

Original deferral rationale (no longer valid):
- "The Sjors fork works correctly today" — INVALIDATED by 6-byte CoinbaseOutputConstraints
  parse failure diagnosed 2026-03-08.
- Official Bitcoin Core 30.2 pre-built binaries do NOT include the multiprocess binary
  (bitcoin-node). The standard bitcoind binary does not support -ipcbind or native sv2=1.
- The SRI standalone TP binary (stratum-mining/sv2-tp v1.0.6) requires a separate
  bitcoin-node binary compiled with --enable-multiprocess + capnproto. This is not in
  any pre-built release; it requires building Bitcoin Core from source in Nix.

When to address:
- Step 2.11 (compilation fixes) is independent — finish it first.
- Then resolve the TP before attempting Step 2.13 integration testing.
- See TP Compatibility Failure section for recommended option order (A→B→C).

 Future architecture (when Phase 1 is eventually done):
 - bitcoin-node: Bitcoin Core v30.2+ built from source in Nix with -DENABLE_IPC=ON
   and capnproto. Started with: bitcoind -m node -ipcbind=unix
 - sv2-tp: stratum-mining/sv2-tp v1.0.6 pre-built binary. Connects to bitcoin-node
   via Unix socket. Serves SV2 TDP on port 8442 (regtest: 18447).
 - Both replace the current Sjors fork process in devenv.nix.
 - Revert the bitcoind30.nix / config/bitcoin30.conf / devenv.nix additions added during
   Phase 1.1–1.4 exploration (those files target the wrong architecture).


---
TP Compatibility Failure (diagnosed 2026-03-08)

Root cause: SRI 1.7.0 pool/jdc sends 6-byte CoinbaseOutputConstraints (u32 size + u16 sigops).
The Sjors fork sv2-tp-0.1.17 (SRI 1.5.0 era) expects old 4-byte CoinbaseOutputDataSize (u32 only).
Result: TP parse fails -> sv2-0 thread never starts -> no NewTemplate/SetNewPrevHash -> pool blocks.

Key files:
- roles/pool/src/lib/template_receiver/mod.rs:121 -- sends CoinbaseOutputConstraints{size, sigops}
- roles/jd-client/src/lib/template_receiver/mod.rs:357 -- sends CoinbaseOutputConstraints{size, sigops}
- roles/pool/src/lib/mod.rs:134 -- total_sigop_cost(|_| None) -> sigops=0 for P2WPKH
- bitcoind.nix -- pinned to sv2-tp-0.1.17

Options (in recommended order):

Option A -- Check if Sjors fork has post-SRI-1.7.0 commits with 6-byte support
  - Sjors/bitcoin last commit: July 2025 (6 months after SRI 1.7.0 released January 2025)
  - Latest tagged release: v0.1.19 (July 2024) -- predates SRI 1.7.0, almost certainly still 4-byte
  - Action: browse https://github.com/Sjors/bitcoin commits since January 2025 for
    CoinbaseOutputConstraints or sigops-related TP changes
  - If found: update bitcoind.nix to fetchFromGitHub at that commit hash (build from source)
  - Estimated effort: 1-2 hours to research + nix build update

Option B -- Front-load Phase 1 (sv2-tp v1.0.6 + Bitcoin Core 30 multiprocess)
  - This is the right long-term path (user's stated end goal: latest bitcoind)
  - Requires building Bitcoin Core from source in Nix with --enable-multiprocess + capnproto
  - sv2-tp v1.0.6 pre-built binary from stratum-mining/sv2-tp connects to bitcoin-node via IPC
  - Complexity is in the Nix build of bitcoin-node; everything else is straightforward
  - Strongly preferred over Option C since it aligns with end goal and makes Phase 1 unnecessary later
  - Estimated effort: half-day to full day Nix work

Option C -- 4-byte backward-compat hack (last resort)
  - In pool/jdc template receivers, manually write only coinbase_output_max_additional_size (u32)
    as a raw frame instead of using CoinbaseOutputConstraints struct serialization
  - sigops=0 for P2WPKH so functionally equivalent for current use case
  - Allows Phase 2.13 integration test to pass against old TP
  - Technical debt: MUST be removed when Phase 1 is eventually done
  - Mark all affected code with // TEMP TP COMPAT: remove in Phase 1
  - Estimated effort: 1-2 hours

Recommended path: Try Option A research first (quick). If no Sjors fork update exists,
proceed directly to Option B (front-load Phase 1). Avoid Option C -- it creates
technical debt in code that exercises the most sensitive protocol path.

 ---
 Phase 2: Crate Migration to crates.io Imports (ACTIVE — Feature branch)

 Goal: Replace all vendored SRI protocol crates with crates.io imports at 1.7.0 versions. Keep only hashpool-custom crates as local path deps.
 The Sjors fork (bitcoind) continues running unchanged throughout this phase.

 Create a new branch: git checkout -b feat/sri-1.7.0-upgrade

 Local path deps to KEEP (hashpool-custom):

 - protocols/v2/subprotocols/mint-quote/ — MintQuoteRequest/Response/Error (55 LOC)
 - protocols/v2/subprotocols/stats-sv2/ — Stats messages (97 LOC)
 - protocols/ehash/ — CDK/Cashu integration utilities (1,302 LOC)
 - protocols/v2/parsers-sv2/ — Extends upstream parsers with MintQuoteNotification parsing
 - protocols/v2/handlers-sv2/ — Custom handler traits (async)

 crates.io imports to switch (unmodified SRI crates):

 binary_sv2, const_sv2, framing_sv2, noise_sv2, buffer_sv2, common_messages_sv2, template_distribution_sv2, job_declaration_sv2, channels_sv2,
 roles_logic_sv2, mining_sv2 (after Step 2.1)

 Step 2.1 — De-inject custom messages from mining_sv2

 Move protocols/v2/subprotocols/mining/src/mint_quote_notification.rs out of mining_sv2:
 - Create protocols/v2/subprotocols/mining/src/hashpool_messages.rs in the mint-quote crate (or a new protocols/v2/subprotocols/hashpool-mining/ crate)
 - Update parsers_sv2 to import MintQuoteNotification/MintQuoteFailure from the new location instead of from mining_sv2
 - Validate this compiles on the current 1.5.0 vendored crates before proceeding

 Decision point: The cleanest approach is to move these types into mint_quote_sv2 (which already handles mint-related message types). They are logically
 related. Rename the crate to hashpool_sv2 or keep as mint_quote_sv2.

 Step 2.2 — Tier A: Leaf crates (binary/const/noise/buffer — no app code impact)

 Update protocols/Cargo.toml to switch the bottom-most dependency tier to crates.io:
 binary_sv2 = "5.0.0"         # was path = "v2/binary-sv2"
 const_sv2 = "latest"
 noise_sv2 = "latest"
 buffer_sv2 = "latest"        # was path = "v2/buffer-sv2"
 Checkpoint: cargo build -p binary_sv2 -p noise_sv2 (in protocols workspace)

 Step 2.3 — Tier B: Framing/codec layer

 framing_sv2 = "6.0.0"        # was path = "v2/framing-sv2"
 codec_sv2 = "4.0.0"
 Checkpoint: cargo build -p framing_sv2 -p codec_sv2

 Step 2.4 — Tier C: Subprotocol crates (requires Step 2.1 complete)

 common_messages_sv2 = "latest"
 template_distribution_sv2 = "latest"
 job_declaration_sv2 = "latest"       # includes wtxid_list rename
 mining_sv2 = "7.0.0"                 # now clean (Step 2.1 completed)
 Checkpoint: cargo build -p mining_sv2 -p template_distribution_sv2

 Step 2.5 — Fix job_declaration_sv2 rename (immediately after Tier C)

 Mechanical rename of tx_ids_list → wtxid_list in 3 files:
 - roles/jd-server/src/lib/job_declarator/message_handler.rs (lines 87, 237, 238)
 - roles/jd-client/src/lib/channel_manager/template_message_handler.rs (line 338)

 Step 2.6 — Tier D: channels_sv2 (isolated — highest-risk step)

 channels_sv2 = "3.0.0"
 Isolated to its own step because channels_sv2 has two distinct major bumps (1→2 at SRI 1.6, 2→3 at SRI 1.7)
 and the 3.0.0 API changes directly affect application code in pool/mod.rs and translator/.
 Checkpoint: cargo build -p channels_sv2

 Step 2.7 — Fix channels_sv2 API changes (JobStore owned-return)

 channels_sv2 1.0.2 → 3.0.0 changes JobStore trait methods to return owned types instead of references.
 This step immediately follows Tier D (Step 2.6) because the two changes are tightly coupled.
 Primary impact on:
 - roles/pool/src/lib/mining_pool/mod.rs — main consumer of server-side channels_sv2 APIs
 - roles/translator/ — uses ExtendedChannel

 Fix pattern: Replace &T with T at callsites, clone where necessary.

 Step 2.8 — Update roles_logic_sv2 to crates.io

 roles_logic_sv2 = "latest"
 This is the facade crate — it re-exports from the component crates above.

 Step 2.9 — Update parsers_sv2 and handlers_sv2 to use crates.io deps

 These stay as LOCAL crates (in protocols/v2/) but their Cargo.toml internal deps switch from path deps to
 crates.io deps. They still live in the protocols workspace as hashpool-custom code.

 Step 2.10 — Update roles/ workspace Cargo.toml files

 For each role (pool, translator, jd-client, jd-server, mint):
 - Remove all path = "../../protocols/v2/..." dependencies for upstream SRI crates
 - Add crates.io version imports (same as Steps 2.2–2.8)
 - Keep path deps only for: mint_quote_sv2, stats_sv2, ehash, parsers_sv2, handlers_sv2

 Step 2.11 — Fix compilation errors top-down

 Compile in dependency order and fix errors:
 1. protocols/ workspace (all protocol crates)
 2. roles/roles-utils/network-helpers/
 3. roles/roles-utils/rpc/
 4. roles/mint/
 5. roles/pool/
 6. roles/translator/
 7. roles/jd-server/
 8. roles/jd-client/

 Validate: cd roles && cargo build --workspace

Step 2.12 — Resolve TP compatibility and switch to updated TP (primary blocker before 2.13)

The Sjors fork (sv2-tp-0.1.17) is incompatible with SRI 1.7.0's 6-byte CoinbaseOutputConstraints.
See TP Compatibility Failure section for fix options (A→B→C). This step must complete before
Step 2.13 integration testing can succeed.

Ordering: Complete Step 2.11 compilation fixes first (independent of TP). Then:
- If Option A: update bitcoind.nix to a Sjors commit with 6-byte support
- If Option B: front-load Phase 1 (Bitcoin Core 30 + sv2-tp v1.0.6 in Nix)
- If Option C (last resort): 4-byte compat hack in pool/jdc template receivers

After TP fix: update config/pool.config.toml and config/jds.config.toml if tp_address changes.
Update devenv.nix pool/jd-server startup to depend on the new TP process. Remove old Sjors process.

 Step 2.13 — Full integration test

 Run devenv up with all processes. Verify:
 - bitcoind30 starts and syncs regtest chain
 - tp (thin TP) connects to bitcoind30 IPC and serves templates on 8443
 - Pool connects and receives templates
 - Miners submit shares
 - Ehash minting produces correct results
 - No regressions in existing test suite: cd roles && cargo test --lib --workspace

 ---
 Phase 3: Handler Trait Migration (Optional, post-Phase 2)

 Goal: Migrate remaining sync handler traits to async (pool, jd-server). jd-client already uses async traits.

 Recommendation: Do this AFTER Phase 2 is stable. The migration doesn't reduce the Phase 2 effort, and the async trait signatures may change between 1.5.0
 and 1.7.0, making it wasteful to do on the old crates.

 Services needing migration:
 - pool/src/lib/template_receiver/message_handler.rs: ParseTemplateDistributionMessagesFromServer → HandleTemplateDistributionMessagesFromServerAsync
 - pool/src/lib/mining_pool/message_handler.rs: ParseMiningMessagesFromDownstream → HandleMiningMessagesFromDownstreamAsync
 - jd-server/src/lib/job_declarator/message_handler.rs: ParseJobDeclarationMessagesFromDownstream → async equivalent

 ---
 Phase 4: sv2-apps Cherry-Picks (Optional, lowest priority)

 If Phase 2 is successful and crates.io imports are in place, evaluate:

 - HTTP monitoring APIs from sv2-apps (Swagger UI) for jd-server and jd-client
 - Hotpath performance monitoring for pool and translator
 - sv2-apps role replacement: Only worth considering if sv2-apps roles can be extended with the ecash layer. After Phase 2, maintenance burden is already
 minimal (~1,500 LOC custom code), so this is unlikely to be worth the migration cost.

 ---
 Critical Files Reference

 ┌─────────────────────────────────────────────────────────────────────┬──────────┬─────────────────────────────────────────────┐
 │                                File                                 │  Phase   │                   Change                    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ bitcoind.nix                                                        │ 1 (defer)│ Minor: update to sv2-tp-0.1.19 (race fix)  │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ protocols/v2/subprotocols/mining/src/mint_quote_notification.rs     │ 2.1      │ Move to mint_quote_sv2                      │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ protocols/v2/subprotocols/mining/src/lib.rs                         │ 2.1      │ Remove custom exports                       │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ protocols/v2/parsers-sv2/                                           │ 2.1, 2.9 │ Update import source for custom messages    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ roles/*/Cargo.toml                                                  │ 2.10     │ Switch to crates.io deps                    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ roles/jd-server/src/lib/job_declarator/message_handler.rs           │ 2.5      │ tx_ids_list → wtxid_list                    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ roles/jd-client/src/lib/channel_manager/template_message_handler.rs │ 2.5      │ tx_ids_list → wtxid_list                    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ roles/pool/src/lib/mining_pool/mod.rs                               │ 2.7      │ channels_sv2 owned-return fixes             │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ config/pool.config.toml                                             │ 2.12     │ No tp_address change (TP stays at 8442)     │
 └─────────────────────────────────────────────────────────────────────┴──────────┴─────────────────────────────────────────────┘

 ---
 Risk Register

 ┌─────────────────────────────────────────────────────────────────────────┬─────────────────────────────────────────────────────────────────────────────┐
 │                                  Risk                                   │                                 Mitigation                                  │
 ├─────────────────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────┤
 │ TDP wire format compat: Sjors fork (SRI 1.5.0 pool) vs. future sv2-tp  │ MATERIALIZED (2026-03-08): Sjors fork sv2-tp-0.1.17 incompatible with SRI  │
 │                                                                         │ 1.7.0 6-byte CoinbaseOutputConstraints. Blocks Step 2.13. See TP Compat    │
 │                                                                         │ Failure section. Resolution required before 2.13.                          │
 ├─────────────────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────┤
 │ channels_sv2 3.0.0 owned-return changes cause lifetime cascades         │ Fix mechanically in pool/mod.rs; clone where necessary                      │
 ├─────────────────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────┤
 │ MintQuoteNotification message type 0xC0 conflicts with new upstream     │ Phase 0.2 confirms 0xC0+ is extension range; unlikely conflict              │
 │ mining messages                                                         │                                                                             │
 └─────────────────────────────────────────────────────────────────────────┴─────────────────────────────────────────────────────────────────────────────┘

 ---
 Verification Checkpoints

 1. Phase 0 complete: Can build workspace cleanly; have confirmed which crates are unmodified; have bitcoin-core-sv2 config documented [DONE]
 2. Phase 2.1: mining_sv2 compiles clean without custom message types; types importable from new location in mint_quote_sv2 or dedicated crate
 3. Phase 2.11: Full cargo build --workspace succeeds from roles/
4. Phase 2.13: devenv up runs full stack with updated TP (see TP Compatibility Failure section
   for which option was chosen) + migrated crates; miner submits shares; ehash minting produces ecash tokens
5. Phase 1: bitcoin-node (multiprocess build) + sv2-tp v1.0.6 replace the Sjors fork; verified in regtest
