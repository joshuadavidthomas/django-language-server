# Plan 017: Tidy djls-semantic — what remains after the split

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: Plans 013, 015, and 016 must be DONE (README
> status table). This plan tidies what *remains* in djls-semantic after the
> project model moved to djls-project (015) and the test infrastructure
> moved to djls-testing (016) — if either still lives in djls-semantic,
> STOP. All planned-at line numbers below WILL have shifted; every step
> begins with its own discovery command — trust those, not the excerpts'
> line numbers.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW-MED (structure-only; behavior frozen by the full suite +
  mdtest + insta snapshots)
- **Depends on**: plans/013, plans/015, plans/016
- **Category**: tech-debt
- **Planned at**: commit `922cc4d7`, 2026-06-10

## Why this matters

djls-semantic grew through semi-automated implementation loops; it works,
but accumulated layers nobody chose: an 840-line `lib.rs` that is 85% inline
test module, a one-implementor trait with its own removal TODO, a
1,100-line module façade with real query logic buried between re-exports,
and a public API where several re-exports have zero external consumers.
After plans 001–016 strip the dead scaffolding and relocate the project
model and test infra, this plan is the final pass: make every remaining
file's name match its contents, every façade thin, and every public export
earned. **Zero new types, zero new traits, zero new helpers** — this plan
only deletes, inlines, and relocates.

## Current state

(Verified at `922cc4d7`; expect heavy drift — re-discover everything.)

- `crates/djls-semantic/src/lib.rs` — 840 lines: module decls (`:1-15`),
  62 `pub use` re-exports (`:17-78`), two tracked queries
  (`validate_template_file`, `validate_nodelist`, `:81-119`), then a
  **720-line `#[cfg(test)] mod tests`** (`:121-840`) of end-to-end
  validation tests (fixture inventories, `collect_errors` assertions, two
  `insta::assert_snapshot!` calls at `:462` and `:551`).
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
- `crates/djls-semantic/src/python.rs` — 1,122 lines at planned-at: module
  decls + 13 `pub use` re-exports **plus** the extraction-coordination
  logic (`HelperCall` interned struct, `analyze_helper`,
  `extract_tag_rules`, `extract_filter_arities`, `extract_block_specs`,
  `extract_rules`, private helpers). Post-015 it is smaller
  (`ParsedPythonModule`/`parse_python_module`/`ModulePath` moved down, the
  registration scanner + `ExprExt` moved into djls-project — semantic's
  `python/registry.rs` retains only the spec-extraction bridge) but still
  a façade file holding query logic.
- Public-API surplus: the externally-unconsumed re-exports found at
  planned-at were `ProjectTemplateFiles`, `TemplateDirs`, `BlockSpecs`,
  `FilterArityMap`, `ModelDef`, `TagRuleMap` (deleted by plan 013) and,
  still standing, candidates `SymbolKind`, `TagRule`,
  `extract_model_graph`, `compute_opaque_regions` — to be re-verified in
  Step 4, since plan 016 made djls-testing a new external consumer of
  several semantic types.
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
- `crates/djls-semantic/src/lib.rs`
- `crates/djls-semantic/src/traits.rs` (delete)
- `crates/djls-semantic/src/structure.rs`, `src/structure/builder.rs`
  (trait inlining only)
- `crates/djls-semantic/src/python.rs` + new `src/python/queries.rs`
- `crates/djls-semantic/tests/` (new `validation.rs` + relocated insta
  snapshots)
- Import-line-only updates in other djls-semantic files and in consumer
  crates where a deleted re-export forces the owning-module path

**Out of scope** (do NOT touch, even though they look related):
- `python/analysis/` internals — `statements.rs` (1,249 lines) and
  `guards.rs` (1,238 lines) are big, but splitting them is a redesign of
  the extraction walker, not a tidy; it stays deferred until evidence
  demands it (same judgment as plan 015's deferral note).
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

### Step 2: Move the lib.rs test module out

Discovery: `rg -n "#\[cfg\(test\)\]" crates/djls-semantic/src/lib.rs` and
read the module — post-016 its scaffolding imports already come from
`djls_testing::`.

Move the entire `mod tests` to a new integration test
`crates/djls-semantic/tests/validation.rs` (these are end-to-end
validate-a-template tests — integration is their honest shape). Rules:

- Imports flip from `crate::X` to `djls_semantic::X`. If any needed item
  is not public, do NOT widen visibility for it — fall back to moving the
  affected tests into `src/validation.rs`'s own `#[cfg(test)]` module
  instead, and say which in your report.
- The two insta snapshots will be re-homed (snapshot files are keyed by
  module path; integration-test snapshots live under `tests/snapshots/`).
  Run the suite, accept the *renamed* files, then **diff old vs new
  snapshot content — it must be byte-identical**. Delete the orphaned old
  `.snap` files.

**Verify**: `cargo test -q -p djls-semantic` → all pass, same test count
as before the move; `rg -c "#\[cfg\(test\)\]" crates/djls-semantic/src/lib.rs`
→ 0; `wc -l crates/djls-semantic/src/lib.rs` → ≤ 150.

### Step 3: Split query logic out of the python.rs façade

Discovery: `rg -n "salsa::tracked|salsa::interned|fn |struct " crates/djls-semantic/src/python.rs`
to see what logic remains post-015.

Move everything that is not a `mod` decl or `pub use` — the extraction
queries (`extract_tag_rules`, `extract_filter_arities`,
`extract_block_specs`, `extract_rules`), `analyze_helper`, `HelperCall`,
and their private helpers — into a new `src/python/queries.rs`
(copy the file, trim each side; do not retype). `python.rs` becomes a thin
façade (mod decls + `pub use`, including `pub use queries::...` so
existing `crate::python::extract_*` callers in `tags.rs` and `filters.rs`
keep compiling unchanged). Salsa is indifferent to the module move — no
query identity changes.

Note: `python/models/extract.rs` already exists — that is why the new file
is `queries.rs`, not `extract.rs`. Don't confuse them.

**Verify**: `cargo test -q -p djls-semantic` → all pass;
`rg -c "fn |struct |enum |impl " crates/djls-semantic/src/python.rs` → 0
(façade only).

### Step 4: Audit the public re-exports

For each remaining `pub use` in `lib.rs` (list them:
`rg -n "^pub use" crates/djls-semantic/src/lib.rs`), check for an external
consumer:

```
rg -n "djls_semantic::.*\b<Name>\b" crates/ --no-heading -g '!djls-semantic/**'
```

(djls-testing and djls-bench COUNT as consumers.) Delete every re-export
with zero hits; any in-crate user of a deleted re-export imports from the
owning module path instead (repo convention: internal code does not import
through crate-root re-exports). Planned-at candidates to start from:
`SymbolKind`, `TagRule`, `extract_model_graph`, `compute_opaque_regions` —
but the sweep is authoritative, in both directions (some candidates may
have gained consumers via plan 016; others may have newly lost theirs).
Record the kept/deleted table in your report.

**Verify**: `cargo build -q` → exit 0 (workspace-wide, proving no consumer
broke); `cargo test -q` → all pass.

### Step 5: Full validation

**Verify**: `cargo test -q`, `just test`, `just clippy`, `just fmt`,
`just lint` → all exit 0. Then `jj diff --stat`: changes confined to
in-scope files; snapshot files renamed but not content-changed (Step 2's
byte-diff is the evidence).

## Test plan

No new tests — the deliverable is structure. The regression net is the
full suite plus two invariants this plan must demonstrate:

1. Test count per crate unchanged (record before/after).
2. Insta snapshot and mdtest snapshot *content* unchanged — renames from
   the test relocation are expected; content drift means behavior changed
   and is a STOP.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `crates/djls-semantic/src/traits.rs` does not exist; `rg "SemanticModel" crates/` → no matches
- [ ] `rg -c "#\[cfg\(test\)\]" crates/djls-semantic/src/lib.rs` → 0 and `wc -l` ≤ 150
- [ ] `rg -c "fn |struct |enum |impl " crates/djls-semantic/src/python.rs` → 0
- [ ] Every `pub use` left in lib.rs has ≥ 1 consumer outside djls-semantic (sweep table in report)
- [ ] Snapshot content byte-identical (renames allowed); test counts unchanged
- [ ] New types/traits/helpers introduced: 0 (`jj diff` review)
- [ ] `cargo test -q` exits 0; `just test` exits 0
- [ ] `just clippy` exits 0
- [ ] Only in-scope files modified (`jj diff --stat`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Inlining `SemanticModel` requires changing visitor traversal order or
  any `TemplateTree` output — that means the trait was load-bearing;
  report instead of adapting.
- Step 2's fallback also fails (a test needs an item that is neither
  public nor reachable from `validation.rs`) — report the item.
- Any snapshot's *content* changes at any step.
- A re-export deletion breaks djls-testing or djls-ide in a way an import
  edit doesn't fix — re-add it, record it as a kept consumer, continue;
  stop only if that happens for more than three items (the audit premise
  would be wrong).
- You feel the urge to split `statements.rs`/`guards.rs`, restructure
  `python/analysis/`, or introduce a helper module — out of scope; note
  it and move on.

## Maintenance notes

- lib.rs is now the honest external contract: thin re-exports + two entry
  queries. Reviewers should treat new `pub use` lines without an external
  consumer as regressions.
- The deferred big item remains `python/analysis/` (the extraction
  walker): it stays in djls-semantic permanently — it produces semantic
  vocabulary (see plan 015's maintenance notes, 2026-06-10 crate-count
  review) — so any future file-splitting decision is local to this crate.
- The maintainer's wish to move template-tree building into
  djls-templates is recorded as deferred in plans/README.md — Step 1's
  trait removal slightly eases it (one less crate-local abstraction tying
  the builder to djls-semantic), but the crate-dependency knot is the real
  blocker, unchanged by this plan.
