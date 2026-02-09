# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | **done** | Test baseline + corpus-grounded tests |
| M15 | **done** | Return values, not mutation (+ domain types T1-T4) |
| M16 | **in progress** | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | stub | Decompose blocks.rs into strategy modules |
| M18 | stub | Move environment scanning to djls-project |
| M19 | stub | HelperCache → Salsa tracked functions |
| M20 | stub | Rename djls-extraction → djls-python |

## M14 — Test baseline + corpus-grounded tests ✅

Replaced fabricated tests with corpus-sourced equivalents. Cleaned orphaned snapshots. See git history for details.

## M15 — Return values, not mutation (+ domain types T1-T4) ✅

All eval/collection functions return values instead of mutating `&mut` params. New domain types: `ConstraintSet`, `Classification`, `SplitPosition`, `TokenSplit`. See git history for details.

## M16 — Split god-context (+ CompileFunction, OptionLoop)

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 2)
**Plan file:** `.agents/plans/2026-02-09-m16-split-context.md`

### Phase 1: Introduce `AnalysisResult` and make `process_statements` return it ✅

- [x] **M16.1–M16.10** `AnalysisResult` introduced; `process_statements` returns it; `constraints` and `known_options` removed from context struct. All statement arms (If, While, Match, For, Try, With) return results instead of mutating ctx.

### Phase 2: Rename `AnalysisContext` → `CallContext` ✅

- [x] **M16.11–M16.13** Renamed and validated. 745 tests pass.

### Phase 3: Introduce `CompileFunction` validated input type

- [x] **M16.14** Define `CompileFunction<'a>` with `from_ast` constructor in `dataflow.rs`. Returns `None` if function has fewer than 2 positional params.
- [x] **M16.15** Update `analyze_compile_function_with_cache` to construct `CompileFunction`, eliminating `map_or("parser", ...)` fallbacks
- [x] **M16.16** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` — all green (745 passed, 0 failed, 7 ignored)

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

## Baseline (M14.1)

Starting point: 732 workspace tests (241 in djls-extraction). Every M14-M20 change must maintain all green.

## Current Test Counts (M15 complete)

| Suite | Passed |
|-------|--------|
| Unit tests (djls-extraction) | 252 |
| Corpus integration (djls-extraction) | 2 |
| **djls-extraction total** | **254** |
| **Full workspace** | **745 passed, 0 failed, 7 ignored** |

Snapshot files: 185

