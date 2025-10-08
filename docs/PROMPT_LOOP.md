# Prompt Loop

1. Read `AGENTS.md` for the repository overview.
2. Read ``docs/pool-stats.md to understand the current development plan and status.
3. Continue executing the plan.
4. After each phase:
   - Run `cargo build` for the impacted workspace.
   - Pause so the user can run the devenv smoke test.
   - Update the task plan to reflect the completed chunk.
   - Draft a commit message using: one short summary line, a blank line, then bullet points of succinct changes (stay within typical line widths). Do not commit or push.
   - When formatting code, run `rustfmt` or similar only on files you modified and only after edits exist (avoid formatting untouched files or entire crates).
6. Avoid formatting the entire codebase; only touch files relevant to the current change.
