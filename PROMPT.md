# Build Instructions — Extraction Crate Refactor (M14-M20)

## Context

This worktree refactors `djls-extraction` from agent-written procedural code into type-driven
Rust with Salsa integration, following Ruff/ty patterns. The algorithms are correct — the
implementation and representation change.

**This is a refactoring. All existing tests must stay green at every step.** If a test fails
after your change, fix the change, not the test (unless the test was wrong — document why).

Source of truth: `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md` and the dev
docs under `docs/dev/`. The ROADMAP and plan files link to the relevant ones per milestone.

## Your Task

1. Read `AGENTS.md` for build commands and code style rules
2. Read `IMPLEMENTATION_PLAN.md` to understand current progress
3. If `IMPLEMENTATION_PLAN.md` does not exist, create it (see Planning below) and stop
4. Check `.agents/ROADMAP.md` milestones M14-M20 for any not yet in `IMPLEMENTATION_PLAN.md` —
   if any are missing, add stub entries (this is a planning iteration — commit and stop)
5. Pick the next unchecked task from the plan
6. Read the plan for the current milestone only
7. Before making changes, search the codebase — don't assume something isn't implemented
8. **Run tests BEFORE making changes** to confirm the baseline is green
9. Implement that single task completely
10. Run quality checks: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`
11. If checks pass, mark the task complete in `IMPLEMENTATION_PLAN.md` and note any discoveries
12. `git add -A && git commit` with a descriptive message

## Planning

When there are no unchecked tasks available (either the plan doesn't exist or the current
milestone is complete), this is a planning iteration:

1. If `IMPLEMENTATION_PLAN.md` doesn't exist, read `.agents/ROADMAP.md` (milestones M14-M20)
   and the RFC, then create it with stub entries for M14-M20. Commit and stop.
2. If the next milestone has no plan file in `.agents/plans/`, read the milestone description
   from `.agents/ROADMAP.md` and the dev doc it links to, then generate the plan file following
   the format of `.agents/plans/2026-02-06-m10-dataflow-analyzer.md` (phases, changes required,
   success criteria with checkboxes). Commit and stop.
3. If the plan file exists but tasks aren't expanded in `IMPLEMENTATION_PLAN.md`, read the plan
   and expand that milestone's section with detailed tasks. Commit and stop.

Only read one milestone's docs at a time. Each phase should end with a validation task so every
phase is independently green. Do NOT implement anything during a planning iteration.

## Quality Requirements

- ALL commits must pass quality checks (use `-q` flags)
- Do NOT commit broken code
- Keep changes focused to the current task
- Follow existing code patterns in the codebase
- Module convention: `folder.rs` + `folder/` submodules, NOT `folder/mod.rs`
- Workspace deps: ALL dependency versions go in root `Cargo.toml` `[workspace.dependencies]`

## Update AGENTS.md

If you discover something operational that future iterations should know, add it to `AGENTS.md`.
Do NOT add progress notes — those belong in `IMPLEMENTATION_PLAN.md`.

## Stop Condition

After completing your work — creating the plan, expanding a milestone, or implementing a task —
commit and stop. Do not start the next piece of work.

Before declaring completion, check `.agents/ROADMAP.md` milestones M14-M20 against
`IMPLEMENTATION_PLAN.md`. Only when every task across M14-M20 is checked off, reply with
exactly `PLAN_COMPLETE` and stop.
