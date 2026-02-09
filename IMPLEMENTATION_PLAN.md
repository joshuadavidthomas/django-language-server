# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | **done** | Test baseline + corpus-grounded tests |
| M15 | **done** | Return values, not mutation (+ domain types T1-T4) |
| M16 | **planned** | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | stub | Decompose blocks.rs into strategy modules |
| M18 | stub | Move environment scanning to djls-project |
| M19 | stub | HelperCache → Salsa tracked functions |
| M20 | stub | Rename djls-extraction → djls-python |

## M14 — Test baseline + corpus-grounded tests ✅

Replaced 55 fabricated tests with corpus-sourced equivalents. Cleaned 25 orphaned snapshot files (210→185). Final: 247 unit + 2 corpus tests.

## M15 — Return values, not mutation (+ domain types T1-T4) ✅

Six phases completed. Key changes:
- **Phase 1**: `Constraints` → `ConstraintSet` with algebraic `or()`/`and()`/`extend()`. All constraint eval functions return values.
- **Phase 2**: All `blocks.rs` collection functions return values instead of `&mut` params. Added `Classification` struct.
- **Phase 3**: `SplitPosition` enum (`Forward(usize)`, `Backward(usize)`) replaces raw `i64` positions. `Index` enum removed.
- **Phase 4**: `TokenSplit` type encapsulates split offset arithmetic. Replaces manual `base_offset + pops_from_end` math.
- **Phase 5**: `Guard` type skipped (single call site, not worth the abstraction).
- **Phase 6**: Final validation — 745 tests pass, public API unchanged.

## M16 — Split god-context (+ CompileFunction, OptionLoop)

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 2)
**Plan file:** `.agents/plans/2026-02-09-m16-split-context.md`

### Phase 1: Introduce `AnalysisResult` and make `process_statements` return it

- [x] **M16.1** Define `AnalysisResult { constraints: ConstraintSet, known_options: Option<KnownOptions> }` in `eval.rs` with `extend()` method for merging — added `#[derive(Default)]`, `#[allow(dead_code)]` until M16.2 wires it up
- [x] **M16.2** Change `process_statement` to return `AnalysisResult` — each arm returns its accumulated constraints/options instead of mutating `ctx`. Arms that directly set `ctx.constraints` or `ctx.known_options` (If, While, Match) now populate a local `AnalysisResult` instead. `process_statements` merges each statement's result into `ctx`. Recursive `process_statements` calls within arms still accumulate into `ctx` directly (will be addressed in M16.3-M16.6).
- [x] **M16.3** Adapt `Stmt::If` arm: collect body/elif results as `AnalysisResult`, discard keywords via `clear()` on returned results instead of `truncate()` on ctx — added `collect_statements_result()` helper that swaps ctx accumulator fields to capture sub-statement results independently
- [x] **M16.4** Adapt `Stmt::While` arm: use `collect_statements_result` for else-branch body, merge results into returned `AnalysisResult` (option loop path already returned via `result.known_options`)
- [ ] **M16.5** Adapt `Stmt::Match` arm: merge `extract_match_constraints` result into returned `AnalysisResult`
- [ ] **M16.6** Change `process_statements` to return `AnalysisResult` by folding over `process_statement` results
- [ ] **M16.7** Remove `constraints` and `known_options` fields from `AnalysisContext`
- [ ] **M16.8** Update `analyze_compile_function_with_cache` to use returned `AnalysisResult`
- [ ] **M16.9** Update test helpers in `eval.rs` and `constraints.rs` that construct `AnalysisContext`
- [ ] **M16.10** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Rename `AnalysisContext` → `CallContext`

- [ ] **M16.11** Rename `AnalysisContext` to `CallContext` in `eval.rs`, update all imports and references across `statements.rs`, `expressions.rs`, `calls.rs`, `constraints.rs`, `dataflow.rs`
- [ ] **M16.12** Update doc comments to reflect narrower purpose (call resolution context, not analysis accumulator)
- [ ] **M16.13** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Introduce `CompileFunction` validated input type

- [ ] **M16.14** Define `CompileFunction<'a>` with `from_ast(func: &StmtFunctionDef) -> Option<Self>` constructor
- [ ] **M16.15** Update `analyze_compile_function_with_cache` to construct `CompileFunction`, eliminating `map_or("parser", ...)` fallbacks
- [ ] **M16.16** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Evaluate `OptionLoop` type rename

- [ ] **M16.17** Evaluate whether `KnownOptions` should be renamed to `OptionLoop`. Check type shape, usage sites, and whether rename adds clarity. Document decision.
- [ ] **M16.18** If renaming: update `KnownOptions` → `OptionLoop` in `types.rs` and all references. If not: document why.
- [ ] **M16.19** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 5: Final validation

- [ ] **M16.20** Full suite: `cargo test -q` — all green
- [ ] **M16.21** Verify: no `ctx.constraints` mutation in `statements.rs`, no `ctx.known_options` mutation
- [ ] **M16.22** Verify: `AnalysisContext` type no longer exists (renamed to `CallContext`)
- [ ] **M16.23** Verify: public API unchanged (`analyze_compile_function()` → `TagRule`)

## M17 — Decompose blocks.rs into strategy modules

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 3), `docs/dev/extraction-type-driven-vision.md` (`BlockEvidence`)

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m17-decompose-blocks.md`_

## M18 — Move environment scanning to djls-project

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m18-move-env-scanning.md`_

## M19 — HelperCache → Salsa tracked functions

**RFC:** `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m19-salsa-integration.md`_

## M20 — Rename djls-extraction → djls-python

**RFC:** `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m20-rename-crate.md`_

## Baseline (M14.1 — 2026-02-09)

### djls-extraction test counts

| Suite | Passed | Failed | Ignored | Total |
|-------|--------|--------|---------|-------|
| Unit tests (`cargo test -q -p djls-extraction --features parser`) | 239 | 0 | 0 | 239 |
| Corpus integration tests (`--test corpus`) | 2 | 0 | 0 | 2 |
| **Total** | **241** | **0** | **0** | **241** |

- **Snapshot files:** 210 (in `crates/djls-extraction/`)
- **Corpus tests:** 2 (integration tests under `tests/corpus/`)

### Full workspace test counts

| Metric | Count |
|--------|-------|
| Total passed | 732 |
| Total failed | 0 |
| Total ignored | 7 |

All tests green. This is the baseline that every M14-M20 change must maintain.

## Current Test Counts (M15 complete)

| Suite | Passed |
|-------|--------|
| Unit tests (djls-extraction) | 252 |
| Corpus integration (djls-extraction) | 2 |
| **djls-extraction total** | **254** |
| **Full workspace** | **745 passed, 0 failed, 7 ignored** |

Snapshot files: 185

## Discoveries

- **Corpus vs fabricated**: Real Django functions don't always match assumed patterns. Always verify corpus function signatures before replacing tests.
