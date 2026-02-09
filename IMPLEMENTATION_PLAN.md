# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | **done** | Test baseline + corpus-grounded tests |
| M15 | **in progress** | Return values, not mutation (+ domain types T1-T4) |
| M16 | stub | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | stub | Decompose blocks.rs into strategy modules |
| M18 | stub | Move environment scanning to djls-project |
| M19 | stub | HelperCache → Salsa tracked functions |
| M20 | stub | Rename djls-extraction → djls-python |

## M14 — Test baseline + corpus-grounded tests ✅

**Design docs:** `docs/dev/extraction-test-strategy.md`, `docs/dev/corpus-refactor.md`
**Plan file:** `.agents/plans/2026-02-09-m14-test-baseline.md`

Replaced 55 fabricated tests with corpus-sourced equivalents across all modules. Cleaned 25 orphaned snapshot files (210→185). Final: 247 unit + 2 corpus tests. See git history and plan file for per-phase details.

## M15 — Return values, not mutation (+ domain types T1-T4)

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 1), `docs/dev/extraction-type-driven-vision.md`
**Plan file:** `.agents/plans/2026-02-09-m15-return-values.md`

### Phase 1: `ConstraintSet` type (T4) + constraint functions return values ✅

Renamed `Constraints` → `ConstraintSet` with algebraic `or()`/`and()`/`extend()` methods. All constraint eval functions now return `ConstraintSet` instead of mutating `&mut`. See git history for per-task details (M15.1-M15.6).

### Phase 2: `blocks.rs` collection functions return values ✅

All collection functions (`collect_parser_parse_calls`, `collect_skip_past_tokens`, `classify_in_body`, `collect_token_content_comparisons`) now return values instead of taking `&mut` params. Added `Classification` struct. See git history for per-task details (M15.7-M15.12).

### Phase 3: `SplitPosition` newtype (T1) — cross-crate

- [x] **M15.13** Define `SplitPosition` enum (`Forward(usize)`, `Backward(usize)`) in `types.rs` with `arg_index()`, `raw()`, `is_tag_name()`, `to_bits_index()` methods
- [x] **M15.14** Update `RequiredKeyword.position` and `ChoiceAt.position` from `i64` to `SplitPosition`
- [x] **M15.15** Update `dataflow/constraints.rs` to emit `SplitPosition` values — already done: M15.13-14 changed field types to `SplitPosition` and added `index_to_split_position` helper; constraints.rs already emits `SplitPosition` for all `RequiredKeyword` and `ChoiceAt` outputs
- [x] **M15.16** Evaluate `Index` enum in `domain.rs` — consolidated: `Index` removed, `SplitElement` now uses `SplitPosition` directly. `index_to_split_position` helper deleted (was trivial 1:1 mapping). `SplitPosition` is `Copy` so no `.clone()` needed.
- [ ] **M15.17** Update `djls-semantic/src/rule_evaluation.rs` to use `SplitPosition` methods
- [ ] **M15.18** Update `dataflow.rs` `extract_arg_names` and any other consumers
- [ ] **M15.19** Update snapshots: `cargo insta test --accept -p djls-extraction`
- [ ] **M15.20** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` all green

### Phase 4: `TokenSplit` type (T2)

- [ ] **M15.21** Define `TokenSplit` struct in `dataflow/domain.rs` with `fresh()`, `after_slice_from()`, `after_pop_front()`, `after_pop_back()`, `resolve_index()`, `resolve_length()` methods
- [ ] **M15.22** Replace `SplitResult { base_offset, pops_from_end }` and `SplitLength { base_offset, pops_from_end }` with `SplitResult(TokenSplit)` and `SplitLength(TokenSplit)`
- [ ] **M15.23** Replace all scattered `+ base_offset + pops_from_end` calculations in `constraints.rs` with `TokenSplit` method calls
- [ ] **M15.24** Update `eval/effects.rs` pop mutations to use `TokenSplit` methods
- [ ] **M15.25** Update snapshots: `cargo insta test --accept -p djls-extraction`
- [ ] **M15.26** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` all green

### Phase 5: Evaluate `Guard` type (T3)

- [ ] **M15.27** Evaluate whether `Guard` type is worth introducing (single call site). Document decision in this plan.
- [ ] **M15.28** If introduced: define `Guard` type, refactor `extract_from_if_inline` to use it. If skipped: document rationale.
- [ ] **M15.29** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` all green

### Phase 6: Final validation

- [ ] **M15.30** Full suite: `cargo test -q` — all green (740+ tests)
- [ ] **M15.31** Verify: no `&mut Vec<T>` params in `blocks.rs`, no `&mut Constraints` in `constraints.rs`
- [ ] **M15.32** Verify: public API unchanged (`extract_rules()` → `ExtractionResult`)
- [ ] **M15.33** Run `cargo insta test --accept --unreferenced delete -p djls-extraction` to clean orphaned snapshots

## M16 — Split god-context (+ CompileFunction, OptionLoop)

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 2)

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m16-split-context.md`_

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

## Current Test Counts (M14 complete)

| Suite | Passed |
|-------|--------|
| Unit tests (djls-extraction) | 247 |
| Corpus integration (djls-extraction) | 2 |
| **djls-extraction total** | **249** |
| **Full workspace** | **740 passed, 0 failed, 7 ignored** |

Snapshot files: 185 (down from 210 — orphaned snapshots cleaned in M14.16)

## Discoveries

- **Corpus vs fabricated**: Real Django functions don't always match assumed patterns. Always verify corpus function signatures before replacing tests.
