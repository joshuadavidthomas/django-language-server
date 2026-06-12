# Plan 017: Tidy djls-semantic — what remains after the split

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: Plans 013, 015, 016, and 021 must be DONE
> (README status table). This plan tidies what *remains* in djls-semantic
> after the project model moved to djls-project (015), spec extraction
> followed it (021), and the test scaffolding — corpus harness, shared
> database, fixtures, mdtest runner — moved to djls-testing along with
> the tests that consume it (016, as revised 2026-06-11). Expect:
> `src/testing.rs` gone, no `#[cfg(test)]` in `lib.rs` (the inline test
> module now lives at `tests/validation.rs` — 016 absorbed this plan's
> original Step 2), and the database-consuming tests under `tests/`.
> One deliberate keeper is NOT a leftover: `resources/mdtest/` (the
> suites; only the runner moved). If lib.rs still carries the ~720-line
> inline `mod tests`, 016's revised form has not landed — STOP. In
> particular, `crates/djls-semantic/src/python/` must NOT exist (021
> moved it); if it does, 021 has not landed. All planned-at line numbers
> below WILL have shifted; every step begins with its own discovery
> command — trust those, not the excerpts' line numbers.

## Status

- **Priority**: P2
- **Effort**: S (was M; plan 016 absorbed the test-module move)
- **Risk**: LOW (structure-only; behavior frozen by the full suite +
  mdtest + insta snapshots)
- **Depends on**: plans/013, plans/015, plans/016, plans/021
- **Category**: tech-debt
- **Planned at**: commit `922cc4d7`, 2026-06-10; revised 2026-06-11
  (post-015 boundary memo,
  [memo-project-semantic-boundary.md](memo-project-semantic-boundary.md):
  plan 021 moves the `python/` subtree to djls-project — the original
  Step 3 (split query logic out of the python.rs façade) is removed and
  the export audit re-scoped); revised again 2026-06-11 (016
  mid-execution redesign: the lib.rs test-module move — this plan's
  original Step 2 — was absorbed by 016's test relocation, leaving the
  trait deletion and the export audit)
- **Execution status**: source-complete locally at `378a7179`
  ("refactor: tidy djls-semantic structure post-split"); not pushed per
  this plan's git workflow

## Execution record — local source commit `378a7179` (2026-06-11)

Drift check passed after PR #670: `src/testing.rs` absent, no
`#[cfg(test)]` in `src/lib.rs`, `src/python/` absent, and Plan 016 marked
DONE in the local plan index.

Step 1 deleted `crates/djls-semantic/src/traits.rs`, removed `mod
traits;`, and moved the `SemanticModel::model` traversal directly onto
`TemplateTreeBuilder::model`. `rg "SemanticModel" crates/` is empty.

Step 2 kept every existing `pub use` in `src/lib.rs`. A strict
other-crates-only sweep showed some structural exports are only named by
`djls-semantic` integration tests, but deleting them would make those
public-API tests impossible without changing module visibility, which is
outside this plan. Counting integration tests as public consumers, every
remaining re-export has at least one consumer. `compute_opaque_regions`,
the planned-at candidate, is also consumed by `djls-bench`.

Validation passed: `cargo test -p djls-semantic` before and after (same
semantic test counts: 90 lib tests plus integration targets
1/9/3/10/8/8/28), `cargo build -q`, `cargo test -q`, `just test`, clean
`just clippy`, `just fmt`, and `just lint`. Diff is confined to
`crates/djls-semantic/src/lib.rs`, `src/structure.rs`,
`src/structure/builder.rs`, and the deleted `src/traits.rs`; no snapshot
files changed.

**Review verdict (2026-06-11): approved.** Independently re-verified:
the inlined `model()` body is behavior-identical to the old trait path
(visit loop, then the former `construct()` = `finish()` +
`apply_operations()`); the builder retains its `Visitor` impl; the nine
exports kept for integration tests match exactly what
`crates/djls-semantic/tests/*.rs` imports; `compute_opaque_regions` is
consumed by `djls-bench`. The Step 2 divergence — counting the crate's
own `tests/` as public-API consumers — was reported rather than worked
around, and is ratified as the correct reading (see the amended done
criterion below). Remaining: push and PR when Josh says go.

## Why this matters

djls-semantic grew through semi-automated implementation loops; it works,
but accumulated layers nobody chose: a one-implementor trait with its own
removal TODO and a public API where several re-exports have zero external
consumers. (Two other original findings left with other plans: the
1,100-line python.rs façade went to djls-project with plan 021, and the
840-line lib.rs that was 85% inline test module was thinned by 016's
revised test relocation, which moved that module to
`tests/validation.rs`.) After plans 001–016 and 021 strip the dead
scaffolding and relocate the project model, spec extraction, and test
infra, this plan is the final pass: delete the trait nobody needed and
make every public export earned. **Zero new types, zero new traits, zero
new helpers** — this plan only deletes and inlines.

## Current state

(Verified at `922cc4d7`; expect heavy drift — re-discover everything.)

- `crates/djls-semantic/src/lib.rs` — at planned-at, 840 lines: module
  decls, 62 `pub use` re-exports, two tracked queries
  (`validate_template_file`, `validate_nodelist`), then a 720-line
  `#[cfg(test)] mod tests`. Post-016 (revised) the test module lives at
  `tests/validation.rs` and lib.rs is down to decls, re-exports (~10
  fewer after 021), and the two queries.
- `crates/djls-semantic/src/traits.rs` — 30 lines, one `pub(crate) trait
  SemanticModel` whose default `model()` is a visit-loop + `construct()`.
  Its own header comment says:

  ```rust
  // traits.rs:6-8
  // TODO: Consider removing this Visitor-based abstraction once TemplateTree
  // needs source-node links. Directly iterating `NodeList` with indices would let
  // structural nodes reference parser nodes without making TemplateTree lossless.
  ```

  Exactly one implementor: `TemplateTreeBuilder`
  (`src/structure/builder.rs:421`). Two import sites (`structure.rs:34`,
  `structure/builder.rs:20`).
- `crates/djls-semantic/src/python.rs` — GONE post-021 (the whole
  `python/` subtree, its queries, and its lib.rs re-exports moved to
  djls-project as `specs/`). The original Step 3 of this plan (split the
  query logic into `python/queries.rs`) is removed; nothing remains to
  split. lib.rs is correspondingly ~10 re-exports shorter than the
  planned-at inventory.
- Public-API surplus: the externally-unconsumed re-exports found at
  planned-at were `ProjectTemplateFiles`, `TemplateDirs`, `BlockSpecs`,
  `FilterArityMap`, `ModelDef`, `TagRuleMap` (deleted by plan 013). Of
  the remaining planned-at candidates, `SymbolKind`, `TagRule`, and
  `extract_model_graph` left the crate with plan 021; the standing
  candidate is `compute_opaque_regions` — to be re-verified in the audit
  step, since plan 016 made djls-testing a new external consumer of
  several semantic types and plan 021 re-pointed others to djls-project.
- What is fine and stays as-is: the `folder.rs` façades (`structure.rs`,
  `scoping.rs`, `tags.rs`, `validation.rs`) follow the repo's module
  convention; the `validation/` (4 files, ~707 lines), `scoping/`
  (loads 516 + symbols 991), `structure/` (5 files, ~1,765), `tags/`
  (rules 986 + specs 1,171) splits are principled; `db.rs`, `errors.rs`,
  `filters.rs`, `offset.rs`, `resolution.rs` are right-sized.

## Commands you will need

| Purpose      | Command                          | Expected on success |
|--------------|----------------------------------|---------------------|
| Build        | `cargo build -q`                 | exit 0              |
| Test (crate) | `cargo test -q -p djls-semantic` | exit 0, all pass    |
| Test (all)   | `cargo test -q`                  | exit 0, all pass    |
| Rust matrix  | `just test`                      | exit 0              |
| Lint         | `just clippy`                    | exit 0, no warnings |
| Format       | `just fmt`                       | exit 0              |
| Hooks        | `just lint`                      | exit 0              |

## Scope

**In scope** (the only files you should modify/create/delete):
- `crates/djls-semantic/src/lib.rs` (re-export deletions only)
- `crates/djls-semantic/src/traits.rs` (delete)
- `crates/djls-semantic/src/structure.rs`, `src/structure/builder.rs`
  (trait inlining only)
- Import-line-only updates in other djls-semantic files and in consumer
  crates where a deleted re-export forces the owning-module path

**Out of scope** (do NOT touch, even though they look related):
- Anything in `crates/djls-project/` — including the relocated spec
  extraction (`specs/`); plan 021 moved it and any tidy there is a
  separate decision.
- The `validation/`, `scoping/`, `structure/`, `tags/` file layouts.
- `offset.rs` — the name is adequate; renaming is churn without payoff
  (recorded as rejected in plans/README.md).
- Any behavior: no validator, scoping, or extraction logic change; no
  test deleted or weakened; no snapshot content change.
- `resources/mdtest/` and anything in djls-testing.

## Git workflow

jj repo — no mutating `git`. Commit per step is fine; suggested final:
`jj commit -m "refactor: tidy djls-semantic structure post-split"`.
Do NOT push.

## Steps

### Step 1: Delete the one-implementor trait

Discovery: `rg -n "SemanticModel" crates/djls-semantic/src/` — expect the
trait (`traits.rs`), one impl (`structure/builder.rs`), two imports.

Inline the trait's `model()` walk into `TemplateTreeBuilder` as an inherent
method (same body: iterate `nodelist.nodelist(db)`, `visit_node` each,
then the code currently in `construct()`); update the call site in
`structure.rs`; delete `traits.rs` and its `mod traits;` decl. The builder
keeps implementing `djls_templates::Visitor` — only the
crate-local trait wrapper dies.

**Verify**: `rg -n "SemanticModel" crates/` → no matches;
`cargo test -q -p djls-semantic` → all pass.

### Step 2: Audit the public re-exports

(The original Step 2 — moving the lib.rs inline test module out — was
absorbed by plan 016's revised test relocation on 2026-06-11; the
module now lives at `tests/validation.rs`. The original Step 3 —
splitting query logic out of the python.rs façade — was removed earlier
the same day: plan 021 moved those queries to djls-project, so there is
nothing left to split.)

For each remaining `pub use` in `lib.rs` (list them:
`rg -n "^pub use" crates/djls-semantic/src/lib.rs`), check for an external
consumer:

```
rg -n "djls_semantic::.*\b<Name>\b" crates/ --no-heading -g '!djls-semantic/**'
```

(djls-testing and djls-bench COUNT as consumers.) Delete every re-export
with zero hits; any in-crate user of a deleted re-export imports from the
owning module path instead (repo convention: internal code does not import
through crate-root re-exports). Planned-at candidate to start from:
`compute_opaque_regions` (the other planned-at candidates — `SymbolKind`,
`TagRule`, `extract_model_graph` — left the crate with plan 021) — but
the sweep is authoritative, in both directions (some candidates may have
gained consumers via plan 016; others may have newly lost theirs, e.g.
re-exports whose only consumer was the moved spec-extraction code).
Record the kept/deleted table in your report.

**Verify**: `cargo build -q` → exit 0 (workspace-wide, proving no consumer
broke); `cargo test -q` → all pass.

### Step 3: Full validation

**Verify**: `cargo test -q`, `just test`, `just clippy`, `just fmt`,
`just lint` → all exit 0. Then `jj diff --stat`: changes confined to
in-scope files.

## Test plan

No new tests — the deliverable is structure. The regression net is the
full suite plus two invariants this plan must demonstrate:

1. Test count per crate unchanged (record before/after).
2. Insta snapshot and mdtest snapshot *content* unchanged — this plan
   relocates no tests, so even renames are unexpected; any snapshot
   churn means behavior changed and is a STOP.

## Done criteria

Machine-checkable. ALL must hold:

- [x] `crates/djls-semantic/src/traits.rs` does not exist; `rg "SemanticModel" crates/` → no matches
- [x] `wc -l crates/djls-semantic/src/lib.rs` ≤ 150 (inherited from 016; this plan only shrinks it further) — 82 at `378a7179`
- [x] Every `pub use` left in lib.rs has ≥ 1 consumer outside `djls-semantic/src/` (sweep table in report). *Amended at review (2026-06-11): the crate's own `tests/` directory counts as a consumer — plan 016 deliberately made integration tests public-API consumers, so exports they name are earned. The original "outside djls-semantic" wording predates 016's revised test relocation.*
- [x] Snapshot files untouched (no renames, no content changes); test counts unchanged
- [x] New types/traits/helpers introduced: 0 (`jj diff` review — one inherent method, body moved verbatim)
- [x] `cargo test -q` exits 0; `just test` exits 0
- [x] `just clippy` exits 0
- [x] Only in-scope files modified (`jj diff --stat`: 4 files, all in scope)
- [x] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Inlining `SemanticModel` requires changing visitor traversal order or
  any `TemplateTree` output — that means the trait was load-bearing;
  report instead of adapting.
- Any snapshot changes at any step (this plan relocates no tests).
- A re-export deletion breaks djls-testing or djls-ide in a way an import
  edit doesn't fix — re-add it, record it as a kept consumer, continue;
  stop only if that happens for more than three items (the audit premise
  would be wrong).
- You feel the urge to reach into `crates/djls-project/` (e.g. to tidy
  the relocated `specs/` modules) or introduce a helper module — out of
  scope; note it and move on.

## Maintenance notes

- lib.rs is now the honest external contract: thin re-exports + two entry
  queries. Reviewers should treat new `pub use` lines without an external
  consumer as regressions.
- The extraction walker (`statements.rs`/`guards.rs` and the rest of the
  old `python/analysis/`) now lives in djls-project as `specs/` (plan
  021, superseding the 2026-06-10 "stays permanently" note — see
  [memo-project-semantic-boundary.md](memo-project-semantic-boundary.md)).
  Its deferred restructuring decision is local to djls-project.
- The maintainer's wish to move template-tree building into
  djls-templates is recorded as deferred in plans/README.md — Step 1's
  trait removal slightly eases it (one less crate-local abstraction tying
  the builder to djls-semantic), but the crate-dependency knot is the real
  blocker, unchanged by this plan.
