# Plan 015: Move the project model into `djls-project`

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: This plan targets the POST-009 tree. Verify
> prerequisites in the README status table: 006 DONE (`crates/djls-project`
> exists and contains ONLY the `extraction` module — no salsa in its
> manifest yet) and 007/008/009 DONE (the project module is pull-shaped,
> the inspector is gone). Then inventory the current
> `crates/djls-semantic/src/project/` — it should contain roughly:
> `input.rs` (slim Project input), `python.rs` (env discovery),
> `system.rs` (env-var seam), `resolve.rs` (search paths + module
> queries), `settings.rs`/`libraries.rs` (007/008's derivation queries),
> `templates.rs` (004's query), `names.rs`, `symbols.rs`, `db.rs`,
> remaining `sync.rs`. If `introspector.rs` or the cache code still exists,
> plan 009 has not landed: STOP.

## Status

- **Priority**: P2
- **Effort**: M/L (mechanical, but wide import churn)
- **Risk**: LOW-MED (structure-only; no behavior change permitted)
- **Depends on**: plans/006 (the crate + extraction module exist),
  plans/007, plans/008, plans/009 (run before the startup track, or after
  it — see Maintenance notes)
- **Category**: tech-debt (the salvaged architecture of PR #626)
- **Planned at**: commit `922cc4d7`, 2026-06-10; revised at `95e30371`,
  2026-06-10 (crate-count review: djls-python and djls-extraction folded
  into djls-project; registry single-file-move claim corrected — see
  `plans/README.md` reconciliation log)

## Why this matters

This is the one genuinely good *architectural* idea from PR #626, salvaged
without its 11k-line execution: a `djls-project` crate that owns the
mechanical "what is this project" work — the `Project` input, Python
environment discovery, search paths, module resolution, settings facts,
template-file/dir/library derivation — so that `djls-semantic` is finally
what its name and AGENTS.md claim: template *meaning* (validation, scoping,
tag specs, structure). Today two-thirds of djls-semantic is not template
semantics. The reference layering is ty's crate stack (each arrow a one-way
dependency): `ruff_db → resolver/project layers → semantic → ide → server`.
Ours becomes: `djls-source → djls-project → djls-semantic → djls-ide →
djls-server`. Environment discovery (`python.rs`) and the pure recognizers
(the `extraction` module, created by plan 006) live *inside* djls-project as
the project model's input adapters — the 2026-06-10 crate-count review
folded the once-planned `djls-python` and `djls-extraction` crates into this
one (ty's own walkers live inside their consumer crate; a 650-line
env-discovery crate with one consumer is a module wearing a manifest).

The plans deliberately did NOT do this mid-track: moving the module before
006–009 would have relocated thousands of lines those plans delete
(inspector, cache, push-pipeline) and invalidated their file references.
Post-009 the module is slim and pull-shaped — the move is now mostly
`jj`-tracked file moves plus import updates.

## Current state

(Anchors verified at `922cc4d7`/`95e30371`; re-verify shapes against the
post-009 tree.)

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
- **Environment discovery** (folded in from the deleted plan 005):
  - `crates/djls-semantic/src/project/python.rs` (444 lines) — pure
    filesystem probing, no Salsa: `Interpreter { Auto, VenvPath,
    InterpreterPath }` (`:14-21`), `Interpreter::discover` (checks explicit
    setting, then `$VIRTUAL_ENV`, else Auto, `:26-40`),
    `site_packages_path`/`site_packages_path_in_venv` (venv layout probing,
    `:42-117`), `auto_venv_paths` (`[".venv", "venv", "env", ".env"]`,
    `:119+`). Depends only on `camino`,
    `djls_source::{FileSystem, WalkEntryKind, WalkOptions}`, and
    `crate::project::system`.
  - `crates/djls-semantic/src/project/system.rs` (205 lines) — env-var/
    `which` access with test mocking, used by `Interpreter::discover` in
    test builds (`python.rs:34-37`). Ownership check before moving: run
    `rg -n "system::" crates/djls-semantic/src --no-heading` — if
    `python.rs` is the only consumer, `system.rs` moves too; otherwise it
    stays and the moved code takes a minimal env-var seam with it (record
    which case you found).
  - Consumers of `Interpreter` outside `python.rs` (verify with
    `rg -n "Interpreter" crates/ --no-heading`): `project/input.rs:171`
    (input field) and `:217` (bootstrap), `project/resolve.rs:95`
    (`interpreter.site_packages_path(fs, root)` inside
    `SearchPaths::from_project_settings`), `project/sync.rs:59,161`,
    `crates/djls-db/src/settings.rs:2,70`
    (`djls_semantic::Interpreter`), re-exports at `project.rs:23` and
    `djls-semantic/src/lib.rs`. All but djls-db move with the model;
    djls-db re-points to `djls_project::Interpreter`.
- **registry analysis — two halves, only one moves**: `python/registry.rs`
  (736 lines) is db-free but NOT freestanding — it imports seven sibling
  modules (`registry.rs:12-21`: `SymbolKind`, `analysis`, `blocks`,
  `ext::ExprExt`, `filters`, `signature`, `types::{AsVar, BlockSpec,
  FilterArity, TagRule}`). The file splits cleanly:
  - **The registration scanner** (moves down):
    `RegistrationInfo`/`RegistrationKind` (`:28-43`),
    `collect_registrations_from_body` (`:131`) and its private helpers
    (`collect_from_decorated_function`, `tag_name_from_decorator`,
    `filter_name_from_decorator`, `collect_from_call_statement`,
    `tag_registration_from_call`, `filter_registration_from_call`,
    `tag_decorator_kind`, `kw_*`, `first_string_arg`, `callable_name`,
    `:168-445`) — these need only the ruff AST plus the 80-line
    `ext.rs` (`ExprExt`). Plan 008's library derivation calls exactly
    this seam; once that derivation lives in djls-project, calling *up*
    into djls-semantic would be a cycle, which is what forces the move.
  - **The spec-extraction bridge** (stays in semantic):
    `ExtractionOutput` (`:45-53`) and the `impl RegistrationKind` block
    (`symbol_kind`, `as_var`, `extract`, `extract_filter_arity`,
    `extract_tag_rule`, `extract_block_spec`, `:55-129`) — these consume
    semantic's spec stack (`analysis`, `blocks`, `signature`, `filters`,
    `types`) and produce semantic vocabulary.
  - `ext.rs` (`ExprExt`) has 13 other consumers across semantic's
    `python/` tree (`analysis.rs`, `signature.rs`, `blocks.rs` + three
    block submodules, `analysis/` submodules) — after the move they import
    it from `djls_project::extraction`.
- **Downstream consumers** reach project types only through
  `djls_semantic::` re-exports (verified): `djls-ide`
  (`hover.rs:5-7` — `TemplateLibraries`, `TemplateSymbolKind`,
  `TemplateSymbolName`; plus completions), `djls-db` (db.rs imports
  `Project`, `ProjectDb`, `TemplateLibraries`, …), `djls-bench`
  (`db.rs:114,186-210`, `specs.rs:8`), `djls-server` (via session/Project).
  All re-point mechanically.
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
- `crates/djls-project/` (extend: manifest, `src/lib.rs`, the moved
  modules, `src/extraction/registry.rs` + `src/extraction/ext.rs`)
- `crates/djls-semantic/src/project/**` (moves out), `src/python.rs` +
  `src/python/registry.rs` (partial moves), `src/python/ext.rs` (moves),
  `src/lib.rs`, `src/db.rs`, `src/testing.rs`, internal imports throughout
  djls-semantic
- Import updates + manifests: `djls-db` (including
  `crates/djls-db/src/settings.rs`), `djls-ide`, `djls-server`,
  `djls-bench`, root `Cargo.toml`
- `ARCHITECTURE.md` (code map + trait stack sections), `AGENTS.md` crate
  list, `CHANGELOG.md` (internal note per changelog conventions)

**Out of scope** (do NOT touch, even though they look related):
- The spec-extraction analyses (`python/analysis/`, `python/blocks/`,
  `python/models/`, `python/types.rs`, `python/signature.rs`,
  `python/filters.rs`) — they produce semantic vocabulary
  (`TagRule`/`BlockSpec`/`FilterArity`) and STAY in djls-semantic
  permanently (2026-06-10 crate-count review). This plan moves ONLY the
  registration scanner + `ext.rs` (forced by the cycle).
- `resolution.rs` (template-name resolution) — it consumes project queries
  but is template meaning; it STAYS in djls-semantic.
- The `extraction` module's walker/facts/paths code — already in place
  from plan 006; this plan adds `registry.rs`/`ext.rs` beside it and
  changes nothing else there.
- Any signature, behavior, or query-shape change. This plan is moves and
  imports only.
- Growing `djls-db` into the project crate (the research doc floated it):
  rejected — djls-db stays the thin concrete database; the
  trait+input+queries crate mirrors how djls-source already works.

## Git workflow

jj repo — no mutating `git`. When relocating code, MOVE the files first,
then edit in place (repo rule — never retype from memory; reviewers will
diff with rename detection). Commit per step group:
`"refactor: move the registration scanner into djls-project"`,
`"refactor: move the project model into djls-project"`,
`"docs: update architecture for the djls-project split"`. Do NOT push.

## Steps

### Step 1: Move the registration scanner and `ExprExt` into djls-project

- Move `crates/djls-semantic/src/python/ext.rs` to
  `crates/djls-project/src/extraction/ext.rs`.
- Split `crates/djls-semantic/src/python/registry.rs`: move the scanner
  half (see Current state — `RegistrationInfo`, `RegistrationKind`,
  `collect_registrations_from_body`, the private collection helpers, and
  the registration-collection tests) to
  `crates/djls-project/src/extraction/registry.rs`. Move the file with
  `jj`/filesystem move first, then carve the bridge half back out — or
  copy-split if the tooling makes that cleaner; either way reviewers must
  see the scanner arrive as moved code, not retyped.
- The bridge half stays in djls-semantic (same `python/registry.rs` path):
  `ExtractionOutput` plus the spec-extraction methods. **Orphan rule**:
  `RegistrationKind` is now a foreign type, so the inherent
  `impl RegistrationKind` block must become an extension trait (e.g.
  `RegistrationKindExt` with `symbol_kind`/`as_var`/`extract`/
  `extract_*`), implemented in the bridge file — follow the `ExprExt`
  extension-trait shape it already uses.
- Export `ExprExt`, `RegistrationInfo`, `RegistrationKind`,
  `collect_registrations_from_body` from `src/extraction.rs` (the module
  façade plan 006 created). djls-semantic's consumers import
  `djls_project::extraction::…` directly — plan 013's `pub(crate)` seam
  re-exports in `python.rs` are replaced, no shims.
- After the split, verify the moved file's imports are only
  `ruff_python_ast` + `crate::extraction::ext` — if a scanner helper turns
  out to consume a semantic-side type, STOP (see STOP conditions).
- Spec-extraction snapshot tests stay in semantic if they exercise the
  pipeline rather than registration collection alone — decide by reading
  what each test calls
  (`rg -l registry crates/djls-semantic/src/snapshots/`), and record the
  decision.

**Verify**: `cargo test -q` → all pass;
`rg "mod ext" crates/djls-semantic/src/python.rs` → no matches.

### Step 2: Move the project model into djls-project

Extend `crates/djls-project/Cargo.toml` (created by plan 006): add
`djls-source`, `salsa`, `tracing` (all `workspace = true`), and `serde`
only if `symbols.rs` needs it. `ruff_python_ast`/`ruff_python_parser`/
`camino`/`rustc-hash` are already present. Move, file by file (`jj` moves,
then edit):

| From (djls-semantic/src/) | To (djls-project/src/) |
|---|---|
| `project/db.rs` (the `ProjectDb` trait) | `db.rs` (`djls_project::Db`) |
| `project/input.rs` (slim `Project` input) | `project.rs` |
| `project/python.rs` (env discovery) | `python.rs` |
| `project/system.rs` (per the Current-state ownership check) | `system.rs` |
| `project/resolve.rs` (SearchPath(s), module queries, probe) | `resolve.rs` |
| `project/settings.rs` + libraries derivation (from 007/008) | `settings.rs`, `libraries.rs` |
| `project/templates.rs` (from 004) | `templates.rs` |
| `project/names.rs`, `project/symbols.rs` | `names.rs`, `symbols.rs` |
| remaining `project/sync.rs` (compute/apply refresh) | `sync.rs` |
| `ModulePath` definition (from `python/models/graph.rs`) | `names.rs` or `module_path.rs` |
| `parse_python_module` tracked query (from `python.rs:87`) | `parse.rs` |

Note: with env discovery and `SearchPaths` now in the same crate,
`site_packages_path` keeps its `pub(crate)` visibility — no cross-crate
widening needed.

`lib.rs` of the crate re-exports the boundary API (the set today's
`project.rs` façade exports, minus anything with zero external consumers —
re-check with `rg`) alongside the existing `pub mod extraction;`.
djls-semantic: delete `mod project;`, add the `djls-project` dependency
(it may already be present from plan 007 — verify), update every
`crate::project::…` import to `djls_project::…`, and make
`djls_semantic::Db` a supertrait of `djls_project::Db`.

**Verify**: `cargo build -q -p djls-project` then `cargo build -q` → exit 0.

### Step 3: Re-point downstream crates

Mechanical: `djls-db` (trait impls + imports, including
`settings.rs:2,70` `djls_semantic::Interpreter` →
`djls_project::Interpreter`), `djls-ide`, `djls-server`, `djls-bench`
import project types from `djls_project::` instead of `djls_semantic::`.
Clean break — djls-semantic keeps NO re-export shims for moved types (repo
rule: no multi-layer re-exports). `ProjectFixture` in `testing.rs` stays
but constructs `djls_project::Project`.

**Verify**: `cargo test -q` → all pass;
`rg "djls_semantic::(Project|ProjectDb|TemplateLibraries|SearchPath|ModulePath|LibraryName|PyModuleName|Interpreter)" crates/` → no matches.

### Step 4: Docs

- `ARCHITECTURE.md`: add the `crates/djls-project` section to the code map
  (mechanical project model: discovery inputs, env discovery, search
  paths, module resolution, derived Django facts, plus the pure
  `extraction` recognizers); update the trait-stack diagram
  (`SourceDb ← djls_project::Db ← SemanticDb`) and the djls-semantic
  section (now genuinely "where Django knowledge meets the parsed
  template"). Update `AGENTS.md`'s crate-responsibility list (add
  djls-project; its entry should cover both the model and the extraction
  module).
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

- [ ] `crates/djls-semantic/src/project/` does not exist; the project model lives in `crates/djls-project`
- [ ] `rg -c "not template semantics" /dev/null; true` — (human check) djls-semantic's src/ contains only: validation, scoping, structure, tags, filters, errors, resolution, python spec-extraction, db trait, testing
- [ ] djls-semantic has no re-export shims for moved types (`rg "pub use djls_project" crates/djls-semantic/src/lib.rs` → no matches)
- [ ] Dependency direction holds: `rg "djls_semantic" crates/djls-project/` → no matches
- [ ] Extraction-module purity holds (the plan-006 firewall, now module-scoped): `rg "salsa|djls_source" crates/djls-project/src/extraction/` → no matches
- [ ] `cargo test -q`, `just test`, `just e2e`, `just clippy` all exit 0
- [ ] Zero insta snapshot changes
- [ ] `ARCHITECTURE.md` and `AGENTS.md` updated
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- A scanner helper in Step 1 turns out to consume a semantic-side type
  (beyond `ExprExt`) — the scanner/bridge split line is then wrong; report
  the helper and the type rather than dragging spec-stack code down.
- A genuine cycle appears that Step 1 didn't break (something in
  djls-project needs a djls-semantic item beyond the scanner) — report the
  item; the resolution is moving IT down or reshaping the seam, which is a
  design decision.
- A moved tracked query changes behavior (snapshot or e2e diff) — moves
  must be inert; report the diff.
- `TemplateLibraries`/`symbols.rs` turns out to carry semantic behavior
  (validation logic, not just facts) that doesn't belong in the project
  crate — report rather than splitting the type ad hoc.
- `system.rs` turns out to be consumed by modules with semantics beyond
  env-var/`which` lookup and splitting it isn't mechanical — report the
  consumer list.
- The startup track (plans 010–012) landed first and relocated/reshaped
  `sync.rs` — reconcile the move table against reality and report what
  changed before proceeding.

## Maintenance notes

- **Ordering vs the startup track**: this plan and plans 010–012 touch
  different crates except for `sync.rs` (plan 011 splits it into
  compute/apply). Either order works; whichever goes second must re-anchor
  on the moved/reshaped file. The README's recommended order puts 015
  right after 009 — the "everything is slim" point.
- **Env discovery growth path**: `python.rs`/`system.rs` are the future
  home of richer static environment discovery — pyvenv.cfg parsing
  (model: `PyvenvCfgParser` in
  `reference/ruff/crates/ty_site_packages/src/lib.rs` — pyvenv.cfg "looks
  like INI but isn't valid INI", hence ty's hand-rolled cursor parser),
  `home`-key resolution, uv's `extends-environment`, Debian
  `dist-packages` layouts. Each lands as pure functions + fixtures, never
  as db queries. If this grows to ty_site_packages scale (3,646 lines,
  three consumer crates), extracting it into its own crate then is a
  mechanical follow-up — that threshold, not before, is when the crate
  pays for itself (2026-06-10 crate-count review).
- The spec-extraction analyses (`python/analysis/`, `blocks/`, `models/`)
  stay in djls-semantic permanently — they produce semantic vocabulary.
  The seam to watch in review is the bridge trait from Step 1: it should
  stay thin (map `RegistrationKind` → spec extraction), and any growth
  there is a sign the scanner/bridge line was drawn wrong.
- PR #626's `architecture-decision-project-root.md`
  (`jj file show -r startup-rethink docs/agents/startup-rethink/architecture-decision-project-root.md`)
  is worth a read before executing — it documents why the stable Project
  root lives where it does; this plan is the right-sized version of that
  document's crate.
- Reviewers: insist on rename-detection diffs; reject any move that
  arrived as delete+retype.
