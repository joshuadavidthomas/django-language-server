# Plan 021: Move spec extraction into `djls-project`

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: Plan 015 must be DONE and merged (README
> status row; PR #668). Plans 016 and 017 should still be TODO — this plan
> is sequenced before both. If 016 already landed, the corpus helpers and
> shared test database live in `djls-testing`; re-anchor the testing-file
> rows of the move table and report the adjustment. If 017 already landed
> with its original Step 3, the extraction queries live in
> `crates/djls-semantic/src/python/queries.rs` instead of `python.rs`;
> the move table's first row changes accordingly. Then inventory
> `crates/djls-semantic/src/python/` — it should contain `analysis/` (9
> files + façade), `blocks/` (4 files + façade), `models/` (2 files +
> façade), `filters.rs`, `registry.rs`, `signature.rs`, `testing.rs`,
> `types.rs`. All line numbers below were verified at source `735cea66`
> (PR #668 head); content-match before relying on any of them.

## Status

- **Priority**: P2
- **Effort**: M/L (mechanical, but a ~10k-line subtree with wide import
  churn and snapshot relocation)
- **Risk**: LOW-MED (structure-only; no behavior change permitted)
- **Depends on**: plans/015 (merged). Sequenced BEFORE plans/016 and
  plans/017 (both re-anchored by this plan; see README dependency notes).
- **Category**: tech-debt / architecture (boundary correction)
- **Planned at**: 2026-06-11, anchored to source `735cea66`; design record:
  [memo-project-semantic-boundary.md](memo-project-semantic-boundary.md)

## Why this matters

The full argument is in the memo; the operative summary:

- The crate seam should classify by **activity**, not output vocabulary:
  djls-project answers *what did the source mechanically say* (observed
  facts); djls-semantic answers *what do those facts mean* — it is the
  **project-meaning** layer (maintainer framing, 2026-06-11), expressed
  through template analysis today and Python-file features later.
- Everything under `crates/djls-semantic/src/python/` is mechanical source
  observation: compile-function interpretation → `TagRule`, parser-call
  recognition → `BlockSpec`, signature inspection → `FilterArity`, model
  class recognition → `ModelGraph`. None of it reads a template; none of
  it decides a diagnostic. It already sits below the semantic trait
  (`analyze_helper` takes `&dyn djls_source::Db`, python.rs:85; the one
  semantic-`Db` import, `python/models.rs:10`, uses nothing semantic).
- After the move, the boundary is one sentence — *djls-project observes
  source; djls-semantic decides meaning* — and one manifest check: only
  djls-project (of the domain crates) depends on ruff. All ruff usage in
  djls-semantic is already confined to `src/python/` (verified).
- The orphan-rule workaround from plan 015 (`RegistrationKindExt` in
  `python/registry.rs`) dissolves back into an inherent
  `impl RegistrationKind` — net deletion.

This supersedes the 2026-06-10 crate-count review's "spec extraction stays
in djls-semantic permanently" note (recorded in plan 015's out-of-scope
section and README reconciliation log). That review decided crate *count*;
the cycle-forced scanner move then fixed where the line fell. The memo
re-derives the line from the desired architecture.

## Current state

(Verified at `735cea66`; re-verify shapes before moving.)

- **The subtree** (`crates/djls-semantic/src/python/` + `python.rs`,
  ~10,000 lines): `python.rs` (1,096 — salsa queries `extract_tag_rules`
  :154, `extract_filter_arities` :186, `extract_block_specs` :221,
  interned `HelperCall` + `analyze_helper` with cycle recovery :63-131,
  non-salsa `extract_rules(source, module_path)` :263-290, golden snapshot
  tests); `analysis/` (~5,250 — abstract interpretation; `CallContext`
  threads `Option<&dyn djls_source::Db>` so the same machinery runs pure
  in `extract_rules` and salsa-assisted via `analyze_helper`); `blocks/`
  (~1,512); `models/` (~1,705 — `compute_model_graph(db, project)` over
  `model_modules`); `signature.rs` (204); `filters.rs` (252);
  `registry.rs` (93 — the `RegistrationKindExt` bridge); `types.rs`
  (522 — `TagRule`, `BlockSpec`/`BlockSpecs`, `FilterArity`, `SymbolKey`,
  `SymbolKind`, `ExtractionResult`, and the `TagRule` component types:
  `ArgumentCountConstraint`, `RequiredKeyword`, `ChoiceAt`,
  `KnownOptions`, `SplitPosition`, `ExtractedArg`/`ExtractedArgKind`,
  `ExtractedDiagnostic*`); `testing.rs` (117 — corpus loaders,
  `find_function_in_source`).
- **Semantic-side consumers** (these stay; imports re-point):
  `tags.rs:17-18,49,54` (`extract_block_specs`, `extract_tag_rules`),
  `tags/specs.rs:14` (`ExtractionResult` in `merge_extraction_results`),
  `filters.rs:9,87` (`extract_filter_arities`), `db.rs:8,38`
  (`ModelGraph` in the `model_graph()` trait accessor), `testing.rs:46-56`
  (the `TagRule` component cluster + `FilterArity`/`ModelGraph`/
  `SymbolKey`/`TagRule`), `lib.rs` tests (corpus validation suite uses
  `crate::FilterArity`/`SymbolKey` and `testing::build_entry_specs`).
- **External consumers**: `djls-db/src/db.rs:22,176-179`
  (`compute_model_graph`, `ModelGraph`) and tests :675-786
  (`extract_filter_arities`); `djls-bench` (`src/specs.rs:123,211-216`
  `ExtractionResult`/`extract_rules`; `src/db.rs:217` `ModelGraph`;
  `benches/models.rs`, `benches/extraction.rs`);
  `crates/djls-semantic/tests/corpus.rs` + `tests/corpus_models.rs`
  (move with the subtree).
- **Golden snapshots**: 13 files
  `crates/djls-semantic/src/snapshots/djls_semantic__python__tests__golden_*.snap`,
  produced by python.rs's in-file test module.
- **lib.rs re-exports that leave** (`crates/djls-semantic/src/lib.rs:22-31`):
  `ExtractionResult`, `FilterArity`, `ModelGraph`, `SymbolKey`,
  `SymbolKind`, `TagRule`, `compute_model_graph`, `extract_filter_arities`,
  `extract_model_graph`, `extract_rules`.
- **Manifests**: djls-project already depends on `ruff_python_ast`,
  `ruff_python_parser`, `rustc-hash`, `serde`, `salsa` and dev-depends on
  `djls-corpus` + `tempfile`; it needs `insta` added to
  `[dev-dependencies]`. djls-semantic drops `ruff_python_ast` +
  `ruff_python_parser` from `[dependencies]`; it KEEPS `djls-corpus` and
  `insta` dev-deps (the corpus *validation* suite in lib.rs and the
  validation snapshots stay). djls-bench already depends on djls-project —
  import edits only.
- **Module naming**: the new home is `crates/djls-project/src/specs.rs` +
  `src/specs/`. `python` is taken (`src/python.rs` = interpreter
  discovery); "spec extraction" is the established vocabulary
  (AGENTS.md). The subtree does NOT go under `extraction/` — `analysis/`
  threads `djls_source` types and calls the salsa `analyze_helper` query,
  which would break the plan-006 purity firewall
  (`rg "salsa|djls_source" crates/djls-project/src/extraction/` → must
  stay empty). djls-project thereby has two recognizer tiers, mirroring
  what already exists: pure recognizers in `extraction/`, salsa-assisted
  recognition beside `settings.rs` at crate root.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build (crate)| `cargo build -q -p djls-project` | exit 0              |
| Build        | `cargo build -q`                 | exit 0              |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0              |
| E2E suite    | `just e2e`                       | exit 0              |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope**:
- `crates/djls-project/`: manifest (`insta` dev-dep), `src/lib.rs`, new
  `src/specs.rs` + `src/specs/**`, `src/extraction/registry.rs` (gains
  nothing — see Step 2 note), `tests/` (arriving corpus tests),
  `src/snapshots/` (arriving golden snapshots)
- `crates/djls-semantic/`: `Cargo.toml`, `src/lib.rs`, `src/python.rs` +
  `src/python/**` (move out), `src/db.rs`, `src/tags.rs`,
  `src/tags/specs.rs`, `src/filters.rs`, `src/testing.rs`,
  `tests/corpus.rs` + `tests/corpus_models.rs` (move out),
  `src/snapshots/djls_semantic__python__tests__golden_*.snap` (move out)
- Import-line updates: `crates/djls-db/src/db.rs`, `crates/djls-bench/`
  (`src/specs.rs`, `src/db.rs`, `benches/models.rs`,
  `benches/extraction.rs`)
- Docs: `AGENTS.md`, `ARCHITECTURE.md`, `CONTEXT.md`, `CHANGELOG.md`

**Out of scope** (do NOT touch, even though they look related):
- The fusion layer: `compute_tag_specs`, `builtin_tag_specs`,
  `compute_filter_arity_specs`, `TagSpecs`/`TagSpec`/`TagRole`, and all of
  `structure/`, `scoping/`, `validation/`, `resolution.rs`, `offset.rs` —
  meaning stays in djls-semantic.
- `Db::model_graph()` stays on the semantic trait, implemented in djls-db
  via `djls_project::compute_model_graph` — same shape as
  `template_libraries()` today. Only the type import changes.
- Any behavior, signature, or query-shape change. Salsa query identities
  are function names; none change.
- Splitting `statements.rs`/`guards.rs` or restructuring `analysis/` —
  the deferred-redesign note moves homes but stays deferred.
- The extraction purity firewall's definition. If a file you placed under
  `extraction/` trips it, the placement is wrong — fix the placement.

## Git workflow

jj repo — no mutating `git`. MOVE files first, then edit in place (repo
rule — never retype from memory; reviewers diff with rename detection).
Suggested commits: `"refactor: move spec extraction into djls-project"`,
`"docs: rebill djls-semantic as the project-meaning layer"`. Do NOT push.

## Steps

### Step 1: Move the subtree

File moves (jj/filesystem move, then edit):

| From (`crates/djls-semantic/src/`) | To (`crates/djls-project/src/`) |
|---|---|
| `python.rs` | `specs.rs` |
| `python/types.rs` | `specs/types.rs` |
| `python/analysis.rs` + `python/analysis/` (9 files) | `specs/analysis.rs` + `specs/analysis/` |
| `python/blocks.rs` + `python/blocks/` (4 files) | `specs/blocks.rs` + `specs/blocks/` |
| `python/models.rs` + `python/models/{extract,graph}.rs` | `specs/models.rs` + `specs/models/{extract,graph}.rs` |
| `python/signature.rs` | `specs/signature.rs` |
| `python/filters.rs` | `specs/filters.rs` |
| `python/registry.rs` | `specs/registry.rs` |
| `python/testing.rs` | `specs/testing.rs` (`#[cfg(test)]`) |
| `tests/corpus.rs`, `tests/corpus_models.rs` | `crates/djls-project/tests/` (same names) |

Then edit the moved files in place:

- Internal paths: `crate::python::X` → `crate::specs::X`.
- Cross-crate imports become owning-module paths (repo rule — internal
  code does not import through crate-root re-exports):
  `djls_project::ExprExt` → `crate::extraction::ext::ExprExt`;
  `djls_project::{RegistrationInfo, RegistrationKind,
  collect_registrations_from_body}` → `crate::extraction::registry::…`;
  `djls_project::parse_python_module` → `crate::parse::parse_python_module`;
  `djls_project::{ModulePath, Project, model_modules}` →
  `crate::names::ModulePath` / `crate::project::Project` /
  `crate::resolve::model_modules`.
- `specs/models.rs`: `use crate::db::Db` (the old semantic trait) becomes
  `use crate::db::Db` (djls-project's trait) — verify `compute_model_graph`
  needs nothing beyond `djls_project::Db` (the memo verified it: only
  `model_modules` + `parse_python_module`).
- `specs/registry.rs`: delete the `RegistrationKindExt` trait; restore an
  inherent `impl RegistrationKind { … }` block (legal here — same crate as
  the type's definition in `extraction/registry.rs`; the impl lives in
  `specs/registry.rs` next to the machinery it dispatches to, NOT in
  `extraction/` — it consumes specs types and would trip the firewall).
  Update the call sites in `specs.rs` from trait-method to inherent calls
  and drop the trait import.
- Declare `mod specs;` in djls-project, add `insta` to
  `[dev-dependencies]`.
- `specs/testing.rs` boundary check: run
  `rg -n "python::testing|crate::python" crates/djls-semantic/src/testing.rs`
  first. Helpers consumed only by the moved extraction tests move;
  helpers consumed by semantic's staying corpus-validation suite
  (`build_entry_specs`, `build_specs_from_extraction`) stay in
  djls-semantic's `testing.rs` and re-point to `djls_project::` items in
  Step 2. Record the split in your report.

**Verify**: `cargo build -q -p djls-project` → exit 0 (djls-semantic will
not build yet — that is Step 2).

### Step 2: Re-point djls-semantic and downstream crates

- djls-semantic `lib.rs`: delete `mod python;` and the ten re-exports
  listed in Current state. Clean break — no shims.
- Re-point the semantic-side consumers (Current state inventory):
  `tags.rs`, `tags/specs.rs`, `filters.rs`, `db.rs`, `testing.rs`, and the
  lib.rs test module import `djls_project::{extract_block_specs,
  extract_tag_rules, extract_filter_arities, ExtractionResult, TagRule,
  FilterArity, ModelGraph, SymbolKey, …}`.
- djls-semantic `Cargo.toml`: remove `ruff_python_ast` and
  `ruff_python_parser`. Keep `djls-corpus` + `insta` dev-deps.
- djls-project `lib.rs`: export what external consumers actually import —
  derive the set from the compile errors plus a sweep
  (`rg -n "djls_project::<Name>" crates/ -g '!djls-project/**'`). Expected
  set: the three tracked `extract_*` queries, `extract_rules`,
  `compute_model_graph`, `extract_model_graph`, `ExtractionResult`,
  `TagRule` + its component types used by semantic's rule evaluator and
  test fixtures (`ArgumentCountConstraint`, `RequiredKeyword`, `ChoiceAt`,
  `SplitPosition`, `ExtractedDiagnostic*`, …), `BlockSpec`/`BlockSpecs`,
  `FilterArity`, `SymbolKey`, `SymbolKind`, `ModelGraph`, and the map
  types returned by the queries that semantic's merge functions consume.
  No `pub use` without a consumer outside djls-project (plan 017's audit
  rule, applied on arrival).
- djls-db: `src/db.rs:22` and `:176-179` re-point `compute_model_graph` /
  `ModelGraph`; test imports `:675-786` re-point `extract_filter_arities`.
  `was_executed("extract_filter_arities")`-style assertions are unchanged
  (salsa ingredient names are function names).
- djls-bench: re-point the imports inventoried in Current state.
- Snapshots: run the moved tests, let insta write the renamed files under
  djls-project, then **diff old vs new snapshot content below the
  metadata header (`---`) — it must be byte-identical**; the header's
  `source:`/module lines legitimately change. Delete the 13 orphaned
  `.snap` files in djls-semantic. Same for the two corpus integration
  tests' snapshots if any.

**Verify**: `cargo build -q` → exit 0; `cargo test -q` → all pass, and
per-crate test counts match the pre-move totals (djls-semantic's count
drops by exactly the number that arrived in djls-project; record both).

### Step 3: Docs — the boundary becomes the documented rule

- `AGENTS.md` crate list: djls-semantic becomes
  "Django **project meaning**: template validation, scoping, structure,
  tag specs, template resolution" (drop "Python spec extraction");
  djls-project gains "Python spec extraction (tag rules, block specs,
  filter arities, model graph)" alongside its existing entries.
- `ARCHITECTURE.md`: the "Python Static Analysis" section (`:157-171` at
  planned-at) moves under the djls-project description; the djls-semantic
  section is rebilled as the project-meaning layer (fusion + template
  analysis) and loses the sentence about consuming Ruff ASTs from
  djls-project (`:98`); the `:30` deep-dive pointer re-points to
  `crates/djls-project/src/specs/analysis/`.
- `CONTEXT.md`: record the boundary rule in the glossary: *observed source
  facts → djls-project; project meaning (fusion, validity, availability,
  diagnostics) → djls-semantic; only djls-project parses Python.*
- `CHANGELOG.md`: internal-change note per changelog conventions.

**Verify**: `just lint` → exit 0;
`rg -n "spec extraction" AGENTS.md` → only the djls-project line.

### Step 4: Full validation and guards

Run: `cargo test -q`, `just test`, `just e2e`, clean-tree `just clippy`,
`just fmt`, `just fmt --check`, `just lint` → all exit 0.

Guards (all must hold):

```
rg -l "mod python" crates/djls-semantic/src/lib.rs        # no matches
rg "ruff_python" crates/djls-semantic/                    # no matches
rg "djls_semantic" crates/djls-project/                   # no matches
rg "salsa|djls_source" crates/djls-project/src/extraction/  # no matches
rg "pub use djls_project" crates/djls-semantic/src/lib.rs # no matches
rg "RegistrationKindExt" crates/                          # no matches
```

Zero mdtest/e2e/golden-fixture diffs; zero `.snap.new` files; snapshot
relocations content-identical below the header.

## Test plan

No new tests — moved tests travel with their files; the contract is the
full suite + e2e passing unchanged, per-crate test counts reconciling
exactly, and the snapshot content-identity check. The corpus extraction
tests keep their skip-gracefully-when-unsynced behavior (djls-project
already dev-depends on djls-corpus).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `crates/djls-semantic/src/python/` and `src/python.rs` do not exist
- [ ] All six guard `rg` commands above return no matches
- [ ] djls-semantic's `Cargo.toml` has no ruff dependencies
- [ ] djls-project exports the spec vocabulary; every new `pub use` has a
      consumer outside djls-project (sweep table in report)
- [ ] Snapshot content below headers byte-identical; test counts reconcile
- [ ] `cargo test -q`, `just test`, `just e2e`, `just clippy` all exit 0
- [ ] `AGENTS.md`, `ARCHITECTURE.md`, `CONTEXT.md`, `CHANGELOG.md` updated
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- A moved item turns out to import a semantic-side type (anything from
  `tags/`, `validation/`, `scoping/`, `structure/`, `errors.rs`) — the
  memo's import inventory found none; if one exists the analysis is wrong.
  Report the item.
- Any snapshot's content (below the header), any golden fixture, any
  mdtest, or any e2e output changes — moves must be inert.
- `compute_model_graph` or `analyze_helper` turns out to need a trait
  method that `djls_project::Db` does not provide — report; do not widen
  the project trait ad hoc.
- The extraction-purity guard fails — a file was placed under
  `extraction/` that belongs under `specs/`; fix placement, and if the
  failure is anything other than placement, report.
- Plan 016 or 017 landed first (drift check) and the re-anchoring is more
  than path substitution — report what moved.
- A test count fails to reconcile exactly.

## Maintenance notes

- **The two-tier recognizer layout is deliberate**: `extraction/` stays
  pure (string/AST in, facts out — the plan-006 firewall); `specs/` is
  salsa-assisted recognition (cached parses, interned helper analysis).
  If the firewall is ever re-scoped, folding the pure specs files
  (`blocks/`, `signature.rs`, `filters.rs`, `types.rs`,
  `models/extract.rs`) into `extraction/` is a mechanical follow-up —
  do not do it as part of this plan.
- **Deferred items move homes, not status**: restructuring
  `statements.rs` (1,249 lines) / `guards.rs` (1,238) remains deferred;
  it is now local to djls-project (README "Deferred" list updated by this
  plan's README edit).
- **Future Python-file features**: `ModelGraph` is deliberate groundwork
  for features operating on models.py/settings.py directly. Their
  *meaning* layer belongs in djls-semantic (the project-meaning layer);
  whether they need an offset→context map over Python ASTs — which would
  soften the "only djls-project parses Python" check — is memo §6 Q4,
  decided when the first such feature is designed.
- **Watch the façade**: djls-project's export list grew by the spec
  vocabulary. Reviewers should hold the plan-017 line — new `pub use`
  without an external consumer is a regression.
