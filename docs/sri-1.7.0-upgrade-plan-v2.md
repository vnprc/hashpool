 Hashpool SRI 1.7.0 Upgrade Plan (v2.1)

 Context

 Hashpool runs on a fork of the Stratum V2 Reference Implementation (SRI) 1.5.0 with vendored protocol crates and a Sjors-fork bitcoind as the template
 provider. The goal is to get ehash minting working on SRI 1.7.0, which requires:
 1. Switching the Bitcoin template provider from the Sjors fork to official Bitcoin Core 30 + the bitcoin-core-sv2 bridge process
 2. Migrating vendored SRI protocol crates from path dependencies to crates.io imports at 1.7.0 versions (eliminates maintenance burden of vendored copies)
 3. Preserving the hashpool-custom ecash layer (ehash, mint-quote, stats-sv2 protocol crates + the MintQuoteNotification messages)

 Key architectural insight: The only vendored SRI crate with hashpool modifications is mining_sv2 (injected MintQuoteNotification/MintQuoteFailure at
 0xC0/0xC1) and the dependent parsers_sv2/handlers_sv2 crates. All other SRI crates can be replaced with crates.io imports, reducing the vendored codebase to
  ~1,500 LOC of custom hashpool code.

 Execution approach: Do all Phase 1 work on the main branch (devenv config only, no Rust changes). Fork to a feature branch before Phase 2. Validate at each
 step before proceeding.

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
 Phase 1: Bitcoin Core 30 + Thin TP Binary (Days 2–7, iterative)

 Goal: Add official Bitcoin Core 30 (IPC mode) + a thin standalone TP binary as new devenv
 processes. Validate that the thin TP generates templates from Core 30 and that the existing
 pool can connect to it. Do NOT remove the Sjors fork yet — keep existing processes intact.

 The thin TP binary lives in tp/ at the project root with its own Cargo.toml and Cargo.lock.
 It is NOT a member of the roles/ workspace (avoids SRI 1.5.0 vs 1.7.0 dep conflicts).
 It uses bitcoin-core-sv2 (library) from sv2-apps for IPC, and SRI 1.7.0 TDP crates for
 serving the standard wire protocol on TCP port 8443.

 Step 1.1 — Create bitcoind30.nix

 New Nix derivation (parallel to bitcoind.nix) that:
 - Downloads official Bitcoin Core v30.x pre-built binaries from bitcoin.org
 - Uses autoPatchelfHook on Linux (same pattern as current bitcoind.nix)
 - Extracts bitcoind and bitcoin-cli to $out/bin/

 File to create: bitcoind30.nix
 Pattern: Copy bitcoind.nix, update URL, version string, and platform hashes.

 Step 1.2 — Create config/bitcoin30.conf

 New config file for the Core 30 bitcoind. Differences from config/bitcoin.conf:
 - Remove sv2=1, sv2port=, debug=sv2 (Core 30 does not support these Sjors-fork flags)
 - Add IPC mining interface: -ipcbind=unix (creates <datadir>/<network>/node.sock)
 - Use a different RPC port (e.g., rpcport=18553 for regtest) to avoid conflict with existing
   bitcoind on 18443
 - Separate data directory (.devenv/state/bitcoind30/)

 Step 1.3 — Add pkgs.capnproto to devenv.nix

 bitcoin-core-sv2 requires capnproto at build time (Cap'n Proto RPC library).
 Add pkgs.capnproto to the packages list in devenv.nix.

 Step 1.4 — Add bitcoind30 process to devenv.nix

 New process entry in devenv.nix alongside existing bitcoind process:
 bitcoind30 = {
   exec = withLogging ''
     mkdir -p ${bitcoind30DataDir}
     ${bitcoind30}/bin/bitcoind \
       -datadir=${bitcoind30DataDir} \
       -chain=${config.env.BITCOIND_NETWORK} \
       -conf=${config.devenv.root}/config/bitcoin30.conf
   '' "bitcoind30-${config.env.BITCOIND_NETWORK}.log";
 };
 Add bitcoind30DataDir variable. Add bitcoind30 (from bitcoind30.nix) to the packages list.

 Step 1.5 — Validate bitcoind30 starts

 Run: devenv up bitcoind30 (or equivalent single-process start command)
 Check logs/bitcoind30-regtest.log for:
 - Successful chain initialization
 - IPC socket creation at .devenv/state/bitcoind30/regtest/node.sock
 - No errors about unknown options

 Fix and iterate until this works.

 Step 1.6 — Create tp/ mini-workspace

 New directory tp/ at the project root with its own Cargo.toml (NOT in roles/ workspace).
 This isolation prevents SRI 1.7.0 deps from conflicting with roles/ SRI 1.5.0 deps.

 tp/Cargo.toml deps:
 - bitcoin-core-sv2 = { git = "https://github.com/stratum-mining/sv2-apps", rev = "<pin>" }
 - template_distribution_sv2 = "4.0.0"  # SRI 1.7.0 TDP messages
 - codec_sv2 = { version = "...", features = ["with_buffer_pool"] }
 - network_helpers_sv2 = "..."           # TCP server utilities
 - tokio = { version = "1", features = ["full"] }
 - serde = { version = "1", features = ["derive"] }
 - toml = "0.8"
 - tracing = "0.1"
 - tracing-subscriber = "0.3"

 tp/src/main.rs (~150 LOC):
 - Reads config/tp.config.toml (IPC socket path, TDP listen addr)
 - Spawns dedicated thread with tokio LocalSet for BitcoinCoreSv2 (required by capnp-rpc)
 - Creates async channel between IPC thread and TDP TCP server
 - Accepts TDP TCP connections; on CoinbaseOutputConstraints from downstream, starts
   forwarding NewTemplate / SetNewPrevHash messages from the IPC channel

 Wire format risk: framing_sv2 bumped 5→6 between SRI 1.5.0 and 1.7.0. This is expected
 to be a Rust API change only (SV2 wire spec is stable), but verify during Step 1.9 smoke
 test that the pool (SRI 1.5.0) successfully parses templates from the thin TP (SRI 1.7.0).

 Step 1.7 — Create config/tp.config.toml

 [ipc]
 socket_path = ".devenv/state/bitcoind30/regtest/node.sock"
 fee_threshold = 1000   # satoshis; triggers new template on mempool fee change
 min_interval = 30      # seconds between consecutive NewTemplate messages

 [listen]
 address = "127.0.0.1:8443"

 Step 1.8 — Add tp process to devenv.nix

 tp = {
   exec = withLogging ''
     # Wait for IPC socket to appear
     while [ ! -S ${bitcoind30DataDir}/${config.env.BITCOIND_NETWORK}/node.sock ]; do sleep 1; done
     echo "Bitcoin Core IPC socket ready"
     cd ${config.devenv.root} && cargo -C tp -Z unstable-options run -- \
       --config ${config.devenv.root}/config/tp.config.toml
   '' "tp.log";
 };

 Step 1.9 — Validate tp generates templates

 Run: devenv up bitcoind30 tp
 Check logs/tp.log for:
 - Successful IPC connection to Bitcoin Core
 - TDP server listening on 127.0.0.1:8443
 - NewTemplate messages being generated

 Fix errors and iterate until this works.

 Step 1.10 — Validate end-to-end connectivity (smoke test)

 Temporarily update config/pool.config.toml to point tp_address at 127.0.0.1:8443.
 Start pool alongside bitcoind30 + tp. Verify pool logs show received templates.
 If the SRI 1.5.0 pool rejects framing from the 1.7.0 TP (wire format regression), diagnose
 and pin tp/ to an SRI version whose framing is compatible with 1.5.0.
 Revert the pool config change before proceeding to Phase 2.

 ---
 Phase 2: Crate Migration to crates.io Imports (Feature branch, 2–4 weeks)

 Goal: Replace all vendored SRI protocol crates with crates.io imports at 1.7.0 versions. Keep only hashpool-custom crates as local path deps.

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

 Step 2.2 — Switch leaf crates to crates.io (no app code impact)

 Update protocols/Cargo.toml to use crates.io versions instead of path deps:
 binary_sv2 = "5.0.0"         # was path = "v2/binary-sv2"
 const_sv2 = "latest"
 framing_sv2 = "6.0.0"        # was path = "v2/framing-sv2"
 noise_sv2 = "latest"
 buffer_sv2 = "3.0.0"         # was path = "v2/buffer-sv2"
 Validate: cargo build -p binary_sv2 -p framing_sv2 -p noise_sv2 (in protocols workspace)

 Step 2.3 — Switch subprotocol crates to crates.io

 common_messages_sv2 = "latest"
 template_distribution_sv2 = "latest"
 job_declaration_sv2 = "latest"       # includes wtxid_list rename
 mining_sv2 = "7.0.0"                 # now clean (Step 2.1 completed)
 channels_sv2 = "3.0.0"
 Validate: cargo build -p mining_sv2 -p template_distribution_sv2

 Step 2.4 — Fix job_declaration_sv2 rename

 Mechanical rename of tx_ids_list → wtxid_list in 3 files:
 - roles/jd-server/src/lib/job_declarator/message_handler.rs (lines 87, 237, 238)
 - roles/jd-client/src/lib/channel_manager/template_message_handler.rs (line 338)

 Step 2.5 — Update roles_logic_sv2 to crates.io

 roles_logic_sv2 = "latest"
 This is the facade crate — it re-exports from the component crates above.

 Step 2.6 — Update parsers_sv2 and handlers_sv2 to use crates.io deps

 These stay as LOCAL crates (in protocols/v2/) but their Cargo.toml internal deps switch from path deps to crates.io deps. They still live in the protocols
 workspace as hashpool-custom code.

 Step 2.7 — Update roles/ workspace Cargo.toml files

 For each role (pool, translator, jd-client, jd-server, mint):
 - Remove all path = "../../protocols/v2/..." dependencies for upstream SRI crates
 - Add crates.io version imports (same as Step 2.2–2.5)
 - Keep path deps only for: mint_quote_sv2, stats_sv2, ehash, parsers_sv2, handlers_sv2

 Step 2.8 — Fix channels_sv2 API changes (JobStore owned-return)

 channels_sv2 1.0.2 → 3.0.0 changes JobStore trait methods to return owned types instead of references. Primary impact on:
 - roles/pool/src/lib/mining_pool/mod.rs — main consumer of server-side channels_sv2 APIs
 - roles/translator/ — uses ExtendedChannel

 Fix pattern: Replace &T with T at callsites, clone where necessary.

 Step 2.9 — Fix compilation errors top-down

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

 Step 2.10 — Replace Sjors fork with Core 30 + sv2_bridge

 Update config/pool.config.toml and config/jds.config.toml to use tp_address = "127.0.0.1:8443" (thin TP). Update devenv.nix pool/jd-server startup to
 depend on tp instead of the old bitcoind. Remove (or disable) the Sjors bitcoind process.

 Step 2.11 — Full integration test

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
 │ bitcoind.nix                                                        │ 1        │ Keep intact; create parallel bitcoind30.nix │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ bitcoind30.nix                                                      │ 1.1      │ New file: official Core 30 derivation       │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ config/bitcoin30.conf                                               │ 1.2      │ New file: Core 30 IPC config, no sv2=1      │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ tp/Cargo.toml + tp/src/main.rs                                      │ 1.6      │ New: thin TP mini-workspace binary          │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ config/tp.config.toml                                               │ 1.7      │ New file: thin TP config (IPC sock, port)   │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ protocols/v2/subprotocols/mining/src/mint_quote_notification.rs     │ 2.1      │ Move to mint_quote_sv2                      │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ protocols/v2/subprotocols/mining/src/lib.rs                         │ 2.1      │ Remove custom exports                       │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ protocols/v2/parsers-sv2/                                           │ 2.1, 2.6 │ Update import source for custom messages    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ roles/*/Cargo.toml                                                  │ 2.7      │ Switch to crates.io deps                    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ roles/jd-server/src/lib/job_declarator/message_handler.rs           │ 2.4      │ tx_ids_list → wtxid_list                    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ roles/jd-client/src/lib/channel_manager/template_message_handler.rs │ 2.4      │ tx_ids_list → wtxid_list                    │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ roles/pool/src/lib/mining_pool/mod.rs                               │ 2.8      │ channels_sv2 owned-return fixes             │
 ├─────────────────────────────────────────────────────────────────────┼──────────┼─────────────────────────────────────────────┤
 │ devenv.nix                                                          │ 1.4, 1.8 │ Add bitcoind30 and tp processes             │
├─────────────────────────────────────────────────────────────────────┤──────────┤─────────────────────────────────────────────┘
│ config/pool.config.toml                                             │ 2.10     │ Point tp_address at 127.0.0.1:8443 (tp)    │
 └─────────────────────────────────────────────────────────────────────┴──────────┴─────────────────────────────────────────────┘

 ---
 Risk Register

 ┌─────────────────────────────────────────────────────────────────────────┬─────────────────────────────────────────────────────────────────────────────┐
 │                                  Risk                                   │                                 Mitigation                                  │
 ├─────────────────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────┤
 │ bitcoin-core-sv2 requires SRI 1.7.0 crates, creating crates.io version  │ Run as standalone binary process (no library dep); Cargo never resolves its │
 │ conflicts                                                               │  transitive deps                                                            │
 ├─────────────────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────┤
 │ TDP wire format changed between 1.5.0 and 1.7.0 (framing_sv2 5→6)       │ Phase 0.4 research; Sjors fork stays running until Phase 2.10 is validated  │
 ├─────────────────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────┤
 │ channels_sv2 3.0.0 owned-return changes cause lifetime cascades         │ Fix mechanically in pool/mod.rs; clone where necessary                      │
 ├─────────────────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────┤
 │ MintQuoteNotification message type 0xC0 conflicts with new upstream     │ Phase 0.2 confirms 0xC0+ is extension range; unlikely conflict              │
 │ mining messages                                                         │                                                                             │
 ├─────────────────────────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────┤
 │ bitcoin-core-sv2 binary build fails / configuration undocumented        │ Research in Phase 0.4; alternative is to compile from source with specific  │
 │                                                                         │ sv2-apps commit                                                             │
 └─────────────────────────────────────────────────────────────────────────┴─────────────────────────────────────────────────────────────────────────────┘

 ---
 Verification Checkpoints

 1. Phase 0 complete: Can build workspace cleanly; have confirmed which crates are unmodified; have bitcoin-core-sv2 config documented
 2. Phase 1.4: bitcoind30 starts in devenv, chain syncs in regtest, IPC socket created — visible in logs
 3. Phase 1.8: sv2_bridge connects to bitcoind30, logs show NewTemplate messages being generated on port 8443
 4. Phase 2.1: mining_sv2 compiles clean without custom message types; types importable from new location in mint_quote_sv2 or dedicated crate
 5. Phase 2.9: Full cargo build --workspace succeeds from roles/
 6. Phase 2.11: devenv up runs full stack with Core 30 + thin TP; miner submits shares; ehash minting produces ecash tokens
