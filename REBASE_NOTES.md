# Rebase Notes

## SRI Extension Feasibility Findings

- The Sv2 spec advertises TLV-based extensions, but the reference implementation was built assuming a one-to-one mapping between a frame and its core message. Once SRI deserializes a frame, any TLV payload is discarded.
- There is no mechanism to surface extension data in the existing codec/handler pipeline; the only option today is to intercept raw frames and mutate them before SRI consumes them.
- That approach forces fragile global caches or patched interceptors inside the fork and reintroduces coupling/race conditions. It is not viable for a standalone extension crate.
- A sustainable extension model would first require reworking SRI so parsed messages can carry companion `ExtensionData` alongside the core struct. Only then could Cashu logic live entirely out of tree.
- Given these constraints, the pragmatic route is to keep our fork, isolate Cashu functionality into small adapters, and rebase only after upstream support exists (or we contribute it).

## Phase 2 – Hashpool Diff Map vs `e8d76d68642ea28aa48a2da7e41fb4470bbe2681`

### Summary
- Compared the current Hashpool tree against upstream SRI commit `e8d76d6` using `git diff --name-status e8d76d6` inside `~/work/hashpool`.
- All functional changes fall into Cashu integration, diagnostics, or developer tooling that underpins the ehash mint flow. No stray edits were observed outside the files listed below.
- Legend: `Upstream?` indicates whether the change looks generally useful for SRI (`Yes`), Cashu-specific (`No`), or needs discussion (`Maybe`).

### Protocol Layer (`protocols/`)
| File(s) | Purpose | Shim / Entry Point | Upstream? | Notes |
| --- | --- | --- | --- | --- |
| `protocols/Cargo.toml` | register `mint-quote-sv2` crate and expose Cashu types | `mint_quote_sv2` feature wiring | No | Only needed when pool ↔ mint protocol is enabled. |
| `protocols/v2/binary-sv2/no-serde-sv2/codec/src/{decodable.rs,encodable.rs,impls.rs,datatypes/mod.rs,datatypes/non_copy_data_types/mod.rs,lib.rs}` | add `CompressedPubKey` primitive support required by quote + share messages | `binary_sv2::CompressedPubKey` | Maybe | Generic serialization upgrade; could be proposed upstream. |
| `protocols/v2/const-sv2/src/lib.rs` | reserve mint-quote protocol discriminant + message IDs, set channel bits | `SV2_MINT_QUOTE_PROTOCOL_DISCRIMINANT` | Maybe | Depends on upstream appetite for mint extension namespace. |
| `protocols/v2/roles-logic-sv2/Cargo.toml` | pull in `mint-quote-sv2` | `mint_quote_sv2` | No | Only relevant for Cashu flow. |
| `protocols/v2/roles-logic-sv2/src/channel_logic/channel_factory.rs` | capture share header hash / locking key, fix endianness on target comparisons, bubble mint errors | `ChannelFactory::check_hash`, `OnNewShare::SendSubmitShareUpstream` | Maybe | Hash fix is a bug fix; hash + locking key wiring is Cashu specific. |
| `protocols/v2/roles-logic-sv2/src/errors.rs` | add `MissingPremintSecret` and `KeysetError` variants | `Error::MissingPremintSecret`, `Error::KeysetError` | Maybe | Variants are Cashu-driven but could ship upstream once gated. |
| `protocols/v2/roles-logic-sv2/src/handlers/{common.rs,mining.rs}` | relax logging, ignore benign difficulty errors | `ParseUpstreamMiningMessages::parse_message` | Yes | Minor UX tweaks, candidate for upstream. |
| `protocols/v2/roles-logic-sv2/src/parsers.rs` | introduce `Minting` parser enum, route mint quote + notification frames, extend mining `SubmitShares*` with hash/locking key | `PoolMessages::Minting`, `Mining::MintQuoteNotification` | No | Requires mint quote protocol definitions. |
| `protocols/v2/roles-logic-sv2/src/utils.rs` | adjust logging guard for new share hash usage | `log_message` | Maybe | Tiny helper tweak, neutral. |
| `protocols/v2/subprotocols/mining/Cargo.toml` | enable optional mint features & CDK deps | `mint-quote-sv2` dependency | No | Cashu only. |
| `protocols/v2/subprotocols/mining/src/{cashu.rs,lib.rs,mint_quote_notification.rs,open_channel.rs,submit_shares.rs}` | add Cashu keyset/amount helpers, extend submit messages with `hash` + `locking_pubkey`, emit mint quote notifications, surface `OpenMiningChannelError::no_mint_keyset` | `mining_sv2::cashu::*`, `SubmitSharesExtended::hash` | No | Defines the Cashu-facing message surface. |
| `protocols/v2/subprotocols/mint-quote/{Cargo.toml,README.md,src/*}` | new protocol crate describing MintQuoteRequest/Response/Error | `mint_quote_sv2::MintQuoteRequest` | No | Entirely Cashu specific. |

### Pool Role (`roles/pool/`)
| File(s) | Purpose | Shim / Entry Point | Upstream? | Notes |
| --- | --- | --- | --- | --- |
| `roles/pool/Cargo.toml` | add `roles-utils` crates, mint quote protocol, Cashu deps | `ehash` helper crate | No | Pool-only wiring. |
| `roles/pool/src/lib/mining_pool/{message_handler.rs,mod.rs,setup_connection.rs}` | hook submit-share path to mint quote pipeline, track mint connection, register SV2 mint frames | `Pool::send_extension_message_to_downstream`, `handle_mint_quote_response` | No | Core Cashu integration. |
| `roles/pool/src/lib/mining_pool/pending_shares.rs` | manage share ↔ quote state until mint responds | `PendingShareManager::add/remove` | No | Cashu shim. |
| `roles/pool/src/lib/{stats.rs,web.rs}` | track quote metrics, expose status UI | `StatsHandle::send_stats`, `web::spawn_server` | No | Hashpool diagnostics. |
| `roles/pool/src/main.rs` | CLI wiring for mint connection + web server | `PoolCliArgs` | No | Cashu runtime glue. |

### Mint Role (`roles/mint/`)
| File(s) | Purpose | Shim / Entry Point | Upstream? | Notes |
| --- | --- | --- | --- | --- |
| `roles/mint/{Cargo.toml,src/**/*}` | brand-new Cashu mint role embedding CDK, handling SV2 mint quote frames, and forwarding quotes back to pool | `mint::run`, `sv2_connection::message_handler::handle_pool_frame` | No | Pure Hashpool functionality. |

### Shared Utilities (`roles/roles-utils/` + scripts)
| File(s) | Purpose | Shim / Entry Point | Upstream? | Notes |
| --- | --- | --- | --- | --- |
| `roles/roles-utils/config/{Cargo.toml,src/lib.rs}` | shared configuration loader for mint/pool/translator | `shared_config::load_config` | No | Cashu-specific config abstraction. |
| `roles/roles-utils/mint-pool-messaging/{Cargo.toml,src/*}` | async channel hub for pool ⇄ mint coordination + stats | `mint_pool_messaging::MessageHub` | No | Supports mint bridge. |
| `roles/roles-utils/web-assets/{Cargo.toml,src/*}` | helper crate for pool/translator web dashboards | `web_assets::render_dashboard` | No | UI only. |
| `scripts/{build-cdk-cli.sh,patch-cdk-path.sh,regtest-setup.sh}` | developer tooling for CDK binaries and regtest bring-up | Shell entry points | No | Local DX. |

### Translator Role (`roles/translator/`)
| File(s) | Purpose | Shim / Entry Point | Upstream? | Notes |
| --- | --- | --- | --- | --- |
| `roles/translator/Cargo.toml` | add CDK wallet + web deps | Translator feature wiring | No | Cashu specific. |
| `roles/translator/src/{args.rs,main.rs}` | CLI args for wallet DB, mint endpoint, web UI | `TranslatorCliArgs` | No | Runtime glue. |
| `roles/translator/src/lib/{mod.rs,error.rs}` | thread Cashu services through translator lifecycle | `Translator::new` | No | Cashu integration. |
| `roles/translator/src/lib/proxy/bridge.rs` | capture share hash + locking key, attach wallet, manage keyset broadcasts, translate SubmitShares with Cashu metadata | `Bridge::translate_submit`, `Bridge::new` | No | Cashu-specific share handling. |
| `roles/translator/src/lib/upstream_sv2/{mod.rs,upstream.rs,extension_handler.rs}` | parse mint quote notifications, maintain mint connection state | `extension_handler::handle_mint_notification` | No | Cashu extension surface. |
| `roles/translator/src/lib/downstream_sv1/{diff_management.rs,downstream.rs,mod.rs}` | align diff logic with share hash tracking & wallet metrics | `Downstream::on_submit_share` | No | Hash tracking to support quotes. |
| `roles/translator/src/lib/{proxy_config.rs,status.rs,miner_stats.rs,web.rs}` | expose wallet state via web, share metrics, config plumbing | `web::spawn_server`, `miner_stats::record_quote` | No | Hashpool instrumentation. |

### Tests & Tooling
| File(s) | Purpose | Shim / Entry Point | Upstream? | Notes |
| --- | --- | --- | --- | --- |
| `roles/tests-integration/{Cargo.toml,tests/common/{mod.rs,sniffer.rs}}` | add mint mock support & SV2 sniffing for Cashu flows | `common::mint::MockMint` (in-tree) | No | Enables ehash E2E testing. |
| `roles/test-utils/mining-device-sv1/src/client.rs` | propagate locking pubkey/hash in fake miner submissions | `TestClient::submit_share` | No | Supports Cashu share data. |
| `utils/message-generator/src/into_static.rs` | extend helper to cover new mint message types | `IntoStatic` impls | Maybe | Generic improvement, could upstream. |

### Documentation & Environment
- Added runbooks (`EHASH_PROTOCOL.md`, `PAYMENT_*`, `QUICK_DEPLOY_PLAN.md`, `TLV_INFRASTRUCTURE_PLAN.md`, `TUI_UNFUCK_GUIDE.md`) that describe the Cashu economics and deployment steps.
- Introduced Nix/devenv scaffolding (`flake.nix`, `devenv.nix`, `bitcoind.nix`, etc.) plus `justfile` helpers.
- Bundled CDK binaries (`bin/cdk-cli`, `bin/cdk-mint-cli`) for local testing.

### Candidate Upstream Contributions
- `protocols/v2/binary-sv2` `CompressedPubKey` primitive support.
- Target endianness cleanup in `channel_logic/channel_factory.rs`.
- Less noisy logging around `difficulty-too-low` share rejects.
- `utils/message-generator` static conversion helpers for new datatypes.

Everything else is intentionally Hashpool-specific and should stay local unless SRI adopts a comparable minting workflow.

### Scope Check
Command used: `git diff --name-status e8d76d68642ea28aa48a2da7e41fb4470bbe2681`. No additional tracked changes exist outside the paths enumerated above.
