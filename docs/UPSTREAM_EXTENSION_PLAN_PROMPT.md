# Prompt: Upstream Extension Points + Protocol Crate Migration Plan

You are an agent tasked with producing a concrete plan to upstream extension points into SRI/sv2-apps and to migrate hashpool’s custom protocol pieces into standalone crates. The goal is to move hashpool toward an extension model without requiring patches to upstream core crates.

## Context Summary (use as constraints)
- Hashpool is currently a fork of SRI with custom protocols and role logic for ehash minting.
- Custom protocol pieces include:
  - `protocols/ehash/`
  - `protocols/v2/subprotocols/mint-quote/` (MintQuoteRequest/Response/Error)
  - Mining protocol extensions `MintQuoteNotification` and `MintQuoteFailure` (extension msg types 0xC0/0xC1) currently injected into `mining_sv2` and `parsers_sv2`.
- Custom role logic exists in pool/translator for mint-quote dispatch and wallet handling.
- We want to reduce long-term fork maintenance by upstreaming **extension points** (not necessarily our business logic).
- We also want to move the custom protocol code into **standalone crates** that can be used without patching upstream core crates.
- Optional: make these crates compatible with **Hydrapool**; if a decision point appears with equivalent tradeoffs, bias toward the path that makes Hydrapool integration easiest.

## Deliverable
Produce a written plan (not code) that covers:
1. **Upstream extension points** required in SRI/sv2-apps:
   - Parser / message registry or plugin hooks for custom Mining messages
   - Extension hooks in Pool / Translator to allow custom share accounting and notification messages
   - Any minimal trait surfaces needed to load custom message codecs without patching core crates
2. **Crate migration plan**:
   - How to move `ehash` and `mint-quote` protocols into separate crates (likely a new repo or workspace)
   - How to expose MintQuoteNotification without forking `mining_sv2`
   - Compatibility targets: SRI and sv2-apps
3. **Roadmap** with phases:
   - Upstream extension points
   - Migrate crates
   - Adoption plan in hashpool
4. **Risk/compatibility matrix**:
   - Protocol versioning, message type collisions, compatibility with future SRI updates
   - Impact on existing deployments
5. **Upstreaming strategy**:
   - What to propose first (smallest change set)
   - How to keep proposals minimal and acceptable to upstream maintainers

## Constraints / Notes
- SV2 allows extension message types (0xC0+), so the protocol itself is not the blocker; implementation hooks are.
- Aim for minimal intrusive changes to upstream code (prefer traits, registries, or adapter layers).
- Keep the plan actionable: list concrete files/areas likely to change in upstream repositories.
- Include the Hydrapool compatibility note explicitly and explain any adjustments needed.

## Output Format
- Title
- Executive summary (5–8 bullets)
- Detailed plan (phases with steps)
- Risks + mitigations
- Open questions
