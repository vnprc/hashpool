# Prompt Loop

1. Read `AGENTS.md` for the repository overview.
2. Read `docs/SRI-1.5.0-migration/PHASE_1.md` to understand the current development plan and status.
3. Continue executing the plan until the current phase is complete.
4. After completing each phase:
   - Run `cargo build` for the impacted workspace to verify clean compilation.
   - Write succinct commit messages following best practices (subject line ≤50 chars, body ≤72 chars).
   - Do NOT attribute commits to Claude in the message.
   - Wait for the user to smoke test and approve before proceeding to the next phase.
5. Code formatting:
   - Run `rustfmt` only on files you modified and only after edits exist.
   - Avoid formatting untouched files or entire crates.
6. Avoid formatting the entire codebase; only touch files relevant to the current change.
