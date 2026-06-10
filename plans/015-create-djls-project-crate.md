# Plan 015: Create `djls-project` — move the project model out of djls-semantic

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: This plan targets the POST-009 tree. Verify
> prerequisites in the README status table: 007/008/009 DONE (the project
> module is pull-shaped, the inspector is gone). Then inventory the current
> `crates/djls-semantic/src/project/` — it should contain roughly:
> `input.rs` (slim Project input), `resolve.rs` (search paths + module
> queries), `settings.rs`/`libraries.rs` (007/008's derivation queries),
> `templates.rs` (004's query), `names.rs`, `symbols.rs`, `db.rs`,
> remaining `sync.rs`. If `introspector.rs` or the cache code still exists,
> plan 009 has not landed: STOP.

## Status

- **Priority**: P2
- **Effort**: M/L (mechanical, but wide import churn)
- **Risk**: LOW-MED (structure-only; no behavior change permitted)
- **Depends on**: plans/007, plans/008, plans/009 (run before the startup track, or after it — see Maintenance notes)
- **Category**: tech-debt (the salvaged architecture of PR #626)
- **Planned at**: commit `922cc4d7`, 2026-06-10

## Why this matters

This is the one genuinely good *architectural* idea from PR #626, salvaged
without its 11k-line execution: a `djls-project` crate that owns the
mechanical "what is this project" work — the `Project` input, search paths,
module resolution, settings facts, template-file/dir/library derivation —
so that `djls-semantic` is finally what its name and AGENTS.md claim:
template *meaning* (validation, scoping, tag specs, structure). Today
two-thirds of djls-semantic is not template semantics. The reference
layering is ty's crate stack (each arrow a one-way dependency):
`ruff_db → resolver/project layers → semantic → ide → server`. Ours
becomes: `djls-source → djls-project → djls-semantic → djls-ide →
djls-server`, with `djls-python` (env discovery) and `djls-extraction`
(pure recognizers) feeding djls-project from the side.

The plans deliberately did NOT do this mid-track: moving the module before
006–009 would have relocated thousands of lines those plans delete
(inspector, cache, push-pipeline) and invalidated their file references.
Post-009 the module is slim and pull-shaped — the move is now mostly
`jj`-tracked file moves plus import updates.

## Current state

(Anchors verified at `922cc4d7`; re-verify shapes against the post-009 tree.)

- **The trait stack** (ARCHITECTURE.md "The Database Trait Stack"):
  `djls_source::Db` ← `ProjectDb` (`crates/djls-semantic/src/project/db.rs:18`)
  ← `SemanticDb` (`crates/djls-semantic/src/db.rs:12`). The split preserves
  this exactly, relocating the middle trait:
  `djls_source::Db` ← `djls_project::Db` ← `djls_semantic::Db`.
- **The only upward coupling from `project/` into `python/`** is the
  `ModulePath` newtype: `project/input.rs:16` and `project/resolve.rs:15`
  (`use crate::python::ModulePath;`). `ModulePath` is defined in
  `python/models/graph.rs:55-86` (unchecked string newtype, serde
  transparent).
- **The shared parse query**: `parse_python_module(db, File)`
  (`python.rs:87`, `pub(crate)`) is used by semantic's spec-extraction
  queries (`python.rs:120,193,225,260`, `python/models.rs:35`) AND — after
  plan 008 — by the library derivation that moves to djls-project.
- **Downstream consumers** reach project types only through
  `djls_semantic::` re-exports (verified): `djls-ide`
  (`hover.rs:5-7` — `TemplateLibraries`, `TemplateSymbolKind`,
  `TemplateSymbolName`; plus completions), `djls-db` (db.rs imports
  `Project`, `ProjectDb`, `TemplateLibraries`, …), `djls-bench`
  (`db.rs:114,186-210`, `specs.rs:8`), `djls-server` (via session/Project).
  All re-point mechanically.
- **registry analysis**: `python/registry.rs` is pure and db-free
  (`collect_registrations_from_body(&[Stmt])`, exposed crate-internally by
  plan 013). Plan 008's library derivation calls it; once that derivation
  lives in djls-project, calling *up* into djls-semantic would be a cycle.
- **What plan 014 built**: `ProjectFixture` in `testing.rs` — the single
  test-side `Project` constructor; it moves its construction internals to
  the new crate's types but can stay in djls-semantic's test harness.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0 (cargo via nox; does NOT run tests/e2e) |
| E2E suite    | `just e2e`                       | exit 0              |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope**:
- `crates/djls-project/` (create)
- `crates/djls-semantic/src/project/**` (moves out), `src/python.rs` +
  `src/python/registry.rs` (partial moves), `src/lib.rs`, `src/db.rs`,
  `src/testing.rs`, internal imports throughout djls-semantic
- `crates/djls-extraction/` (receives `registry.rs`)
- Import updates + manifests: `djls-db`, `djls-ide`, `djls-server`,
  `djls-bench`, root `Cargo.toml`
- `ARCHITECTURE.md` (code map + trait stack sections), `AGENTS.md` crate
  list, `CHANGELOG.md` (internal note per changelog conventions)

**Out of scope** (do NOT touch, even though they look related):
- Moving the spec-extraction analyses (`python/analysis/`, `python/blocks/`,
  `python/models/`) into djls-extraction — a real future consolidation,
  but a separate decision with its own plan; this plan moves ONLY
  `registry.rs` (forced by the cycle) and notes the rest.
- `resolution.rs` (template-name resolution) — it consumes project queries
  but is template meaning; it STAYS in djls-semantic.
- Any signature, behavior, or query-shape change. This plan is moves and
  imports only.
- Growing `djls-db` into the project crate (the research doc floated it):
  rejected — djls-db stays the thin concrete database; the
  trait+input+queries crate mirrors how djls-source already works.

## Git workflow

jj repo — no mutating `git`. When relocating code, MOVE the files first,
then edit in place (repo rule — never retype from memory; reviewers will
diff with rename detection). Commit per step group:
`"refactor: move registry analysis into djls-extraction"`,
`"refactor: create djls-project and move the project model"`,
`"docs: update architecture for the djls-project split"`. Do NOT push.

## Steps

### Step 1: Move `registry.rs` into djls-extraction

Move `crates/djls-semantic/src/python/registry.rs` to
`crates/djls-extraction/src/registry.rs` (file move, then fix imports —
it depends only on ruff AST per the plan-013 verification). Export
`RegistrationInfo`, `RegistrationKind`, `collect_registrations_from_body`
from djls-extraction's lib.rs. Update djls-semantic's call sites
(plan 013's re-exports become `pub(crate) use djls_extraction::…` or
direct imports — prefer direct, no shims). Move registry's tests and the
extraction snapshot tests that key off it (`djls-semantic/src/snapshots/`
registry snapshots — check `rg -l registry crates/djls-semantic/src/snapshots/`)
alongside, or leave snapshot tests in semantic if they exercise the
*spec-extraction* pipeline rather than registry alone — decide by reading
what each test calls, and record the decision.

**Verify**: `cargo test -q` → all pass; `rg "mod registry" crates/djls-semantic/` → no matches.

### Step 2: Scaffold djls-project and move the module

Create `crates/djls-project` (manifest modeled on a sibling crate;
`version = "0.0.0"`; deps: `djls-source`, `djls-python`, `djls-extraction`,
`ruff_python_ast`, `ruff_python_parser`, `salsa`, `camino`, `tracing`,
`rustc-hash`, serde if symbols.rs needs it). Move, file by file
(`jj` moves, then edit):

| From (djls-semantic/src/) | To (djls-project/src/) |
|---|---|
| `project/db.rs` (the `ProjectDb` trait) | `db.rs` (`djls_project::Db`) |
| `project/input.rs` (slim `Project` input) | `project.rs` |
| `project/resolve.rs` (SearchPath(s), module queries, probe) | `resolve.rs` |
| `project/settings.rs` + libraries derivation (from 007/008) | `settings.rs`, `libraries.rs` |
| `project/templates.rs` (from 004) | `templates.rs` |
| `project/names.rs`, `project/symbols.rs` | `names.rs`, `symbols.rs` |
| remaining `project/sync.rs` (compute/apply refresh) | `sync.rs` |
| `ModulePath` definition (from `python/models/graph.rs`) | `names.rs` or `module_path.rs` |
| `parse_python_module` tracked query (from `python.rs:87`) | `parse.rs` |

`lib.rs` of the new crate re-exports the boundary API (the set today's
`project.rs` façade exports, minus anything with zero external consumers —
re-check with `rg`). djls-semantic: delete `mod project;`, add the
dependency, update every `crate::project::…` import to `djls_project::…`,
and make `djls_semantic::Db` a supertrait of `djls_project::Db`.

**Verify**: `cargo build -q -p djls-project` then `cargo build -q` → exit 0.

### Step 3: Re-point downstream crates

Mechanical: `djls-db` (trait impls + imports), `djls-ide`, `djls-server`,
`djls-bench` import project types from `djls_project::` instead of
`djls_semantic::`. Clean break — djls-semantic keeps NO re-export shims for
moved types (repo rule: no multi-layer re-exports). `ProjectFixture` in
`testing.rs` stays but constructs `djls_project::Project`.

**Verify**: `cargo test -q` → all pass;
`rg "djls_semantic::(Project|ProjectDb|TemplateLibraries|SearchPath|ModulePath|LibraryName|PyModuleName)" crates/` → no matches.

### Step 4: Docs

- `ARCHITECTURE.md`: add the `crates/djls-project` section to the code map
  (mechanical project model: discovery inputs, search paths, module
  resolution, derived Django facts); update the trait-stack diagram
  (`SourceDb ← djls_project::Db ← SemanticDb`) and the djls-semantic
  section (now genuinely "where Django knowledge meets the parsed
  template"). Update `AGENTS.md`'s crate-responsibility list (add
  djls-project, djls-python, djls-extraction if not already done by earlier
  plans).
- `CHANGELOG.md`: internal-change note.

**Verify**: `just lint` → exit 0; `cargo test -q`, `just test`, `just e2e`,
`just clippy`, `just fmt` → all exit 0. Zero snapshot changes (this plan is
structure-only; any snapshot diff is a defect).

## Test plan

No new tests — the moved tests travel with their files and the full suite
plus e2e passing unchanged is the contract. Spot-check that incrementality
tests in `djls-db` still observe the same query names (salsa ingredient
debug names include the function name, not the crate — if any
`was_executed(...)` assertion breaks on a name change, fix the assertion
string, not the code).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `crates/djls-project` exists; `crates/djls-semantic/src/project/` does not
- [ ] `rg -c "not template semantics" /dev/null; true` — (human check) djls-semantic's src/ contains only: validation, scoping, structure, tags, filters, errors, resolution, python spec-extraction, db trait, testing
- [ ] djls-semantic has no re-export shims for moved types (`rg "pub use djls_project" crates/djls-semantic/src/lib.rs` → no matches)
- [ ] Dependency direction holds: `rg "djls_semantic" crates/djls-project/` → no matches
- [ ] `cargo test -q`, `just test`, `just e2e`, `just clippy` all exit 0
- [ ] Zero insta snapshot changes
- [ ] `ARCHITECTURE.md` and `AGENTS.md` updated
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- A genuine cycle appears that Step 1 didn't break (something in
  djls-project needs a djls-semantic item beyond registry) — report the
  item; the resolution is moving IT down or reshaping the seam, which is a
  design decision.
- A moved tracked query changes behavior (snapshot or e2e diff) — moves
  must be inert; report the diff.
- `TemplateLibraries`/`symbols.rs` turns out to carry semantic behavior
  (validation logic, not just facts) that doesn't belong in the project
  crate — report rather than splitting the type ad hoc.
- The startup track (plans 010–012) landed first and relocated/reshaped
  `sync.rs` — reconcile the move table against reality and report what
  changed before proceeding.

## Maintenance notes

- **Ordering vs the startup track**: this plan and plans 010–012 touch
  different crates except for `sync.rs` (plan 011 splits it into
  compute/apply). Either order works; whichever goes second must re-anchor
  on the moved/reshaped file. The README's recommended order puts 015
  right after 009 — the "everything is slim" point.
- The future consolidation this sets up (explicitly NOT this plan): the
  spec-extraction analyses (`python/analysis/`, `blocks/`, `models/`) are
  candidates for djls-extraction, which would shrink djls-semantic to pure
  template meaning. Each is a separate, evidence-driven move.
- PR #626's `architecture-decision-project-root.md`
  (`jj file show -r startup-rethink docs/agents/startup-rethink/architecture-decision-project-root.md`)
  is worth a read before executing — it documents why the stable Project
  root lives where it does; this plan is the right-sized version of that
  document's crate.
- Reviewers: insist on rename-detection diffs; reject any move that
  arrived as delete+retype.
