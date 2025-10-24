# Prompt Loop

1. Read `AGENTS.md` for the repository overview.
2. Read ``docs/SRI-1.5.0-migration/PHASE_1.md to understand the current development plan and status.
3. Continue executing the plan.
4. After each phase:
   - Run `cargo build` for the impacted workspace.
   - Update the task plan to reflect the completed chunk.
   - When formatting code, run `rustfmt` or similar only on files you modified and only after edits exist (avoid formatting untouched files or entire crates).
6. Avoid formatting the entire codebase; only touch files relevant to the current change.
