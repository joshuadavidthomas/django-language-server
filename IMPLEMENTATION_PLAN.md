# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | **done** | Test baseline + corpus-grounded tests |
| M15 | **planning** | Return values, not mutation (+ domain types T1-T4) |
| M16 | stub | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | stub | Decompose blocks.rs into strategy modules |
| M18 | stub | Move environment scanning to djls-project |
| M19 | stub | HelperCache → Salsa tracked functions |
| M20 | stub | Rename djls-extraction → djls-python |

## M14 — Test baseline + corpus-grounded tests

**Design docs:** `docs/dev/extraction-test-strategy.md`, `docs/dev/corpus-refactor.md`
**Plan file:** `.agents/plans/2026-02-09-m14-test-baseline.md`

### Phase 1: Record Baseline + Audit Fabricated Tests ✅

- [x] **M14.1** Record baseline test counts (241 total: 239 unit + 2 corpus, 210 snapshots)
- [x] **M14.2** Audit all 239 fabricated tests — categorized as (a) replace / (b) keep / (c) remove / (d) pure Rust

### Phase 2: Create Corpus Test Helpers ✅

- [x] **M14.3** Add `find_function_in_source()`, `corpus_function()`, `corpus_source()` helpers
- [x] **M14.4** Validate build and clippy clean

### Phase 3: Replace Fabricated Tests — Registration & Blocks & Filters ✅

- [x] **M14.5** `registry.rs` — 12 tests → corpus, 7 kept fabricated, net +1 (240 total)
- [x] **M14.6** `blocks.rs` — 8 tests → corpus, removed 2 duplicates, added 1 new, net -1 (239 total)
- [x] **M14.7** `filters.rs` — 8 tests → corpus, 9 kept fabricated
- [x] **M14.8** `signature.rs` — 4 tests → corpus, 1 kept fabricated
- [x] **M14.9** Snapshots reviewed — all 210 current, no diffs
- [x] **M14.10** Validate: 241 tests pass, clippy clean

### Phase 4: Replace Fabricated Tests — Dataflow ✅

- [x] **M14.11** `constraints.rs` — 8 tests → corpus (2 replaced, 6 new end-to-end), 23 kept fabricated, net +6 (245 total)
- [x] **M14.12** `eval.rs` — 4 tests → corpus, 34 pure Rust, 10 kept fabricated
- [x] **M14.13** `calls.rs` — 1 test → corpus (allauth), added `analyze_function_with_helpers()` utility, 13 kept
- [x] **M14.14** `scan.rs` — all 16 kept as fabricated (filesystem-oriented, corpus can't provide controlled layouts)
- [x] **M14.15** Validate: 245 unit + 2 corpus tests pass, clippy clean

### Phase 5: Replace Fabricated Tests — Golden/End-to-End

- [x] **M14.16** Audit and replace fabricated Python in `src/lib.rs` golden tests with corpus-sourced equivalents. Keep edge case tests (malformed registrations, error handling) as fabricated with documented justification. Replaced 31 fabricated tests: 7 per-module snapshot tests (defaulttags, loader_tags, defaultfilters, i18n, inclusion, custom, testtags) + 24 corpus assertion tests. Kept 7 edge case tests (b/d). Discovered real Django diverges from fabricated assumptions (verbatim uses parser.parse not skip_past; widthratio uses if/elif/else not !=; debug has no split_contents). Deleted 25 orphaned snapshot files, added 7 new ones (net: 38→13 golden snapshots). Test count: 50 lib.rs tests (was 48).
- [x] **M14.17** Run `cargo insta test --accept --unreferenced delete -p djls-extraction` to clean up orphaned snapshots — no unreferenced snapshots found (M14.16 already cleaned up). 185 snapshot files remain.
- [x] **M14.18** Validate: 247 unit tests pass, no orphaned snapshots, clippy clean
- [x] **M14.19** Full suite: `cargo build -q`, `cargo test`, `cargo clippy -q --all-targets --all-features -- -D warnings` — all green (740 passed, 0 failed, 7 ignored)
- [x] **M14.20** Baseline counts updated below, M14 marked done

## M15 — Return values, not mutation (+ domain types T1-T4)

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 1), `docs/dev/extraction-type-driven-vision.md`
**Plan file:** `.agents/plans/2026-02-09-m15-return-values.md`

### Phase 1: `ConstraintSet` type (T4) + constraint functions return values

- [x] **M15.1** Define `ConstraintSet` in `dataflow/constraints.rs` with `and()`/`or()`/`extend()` methods (replaces `Constraints`). Renamed `Constraints` → `ConstraintSet` in-place, added `single_length`/`single_keyword`/`single_choice` constructors, algebraic `or()`/`and()`, `is_empty()`, and `extend()`. Added `Clone` derive. Methods `#[allow(dead_code)]` until M15.2 wires them.
- [x] **M15.2** Make `eval_condition`, `eval_compare`, `eval_negated_compare`, and all internal constraint helpers return `ConstraintSet` instead of mutating `&mut Constraints`. Used `ConstraintSet` constructors (`single_length`, `single_keyword`, `single_choice`) and algebraic `or()`/`and()` methods. `extract_from_if_inline` still takes `&mut ConstraintSet` (M15.3) but now extends with returned values. Removed top-level `#[allow(dead_code)]` on impl block since constructors are now used.
- [ ] **M15.3** Make `extract_from_if_inline` return `ConstraintSet`
- [ ] **M15.4** Make `extract_match_constraints` in `eval/match_arms.rs` return `ConstraintSet`
- [ ] **M15.5** Update `AnalysisContext.constraints` field type to `ConstraintSet`, update `process_statement` if-arm to collect returned constraints
- [ ] **M15.6** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` all green

### Phase 2: `blocks.rs` collection functions return values

- [ ] **M15.7** Make `collect_parser_parse_calls` return `Vec<ParseCallInfo>` (no `&mut` param)
- [ ] **M15.8** Make `collect_skip_past_tokens` return `Vec<String>`
- [ ] **M15.9** Make `classify_in_body` and `classify_from_if_chain` return a `Classification` struct (intermediates + end_tags)
- [ ] **M15.10** Make `collect_token_content_comparisons` and `extract_comparisons_from_expr` return `Vec<String>`
- [ ] **M15.11** Update all callers in `blocks.rs` to use return values
- [ ] **M15.12** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` all green

### Phase 3: `SplitPosition` newtype (T1) — cross-crate

- [ ] **M15.13** Define `SplitPosition` enum (`Forward(usize)`, `Backward(usize)`) in `types.rs` with `arg_index()`, `raw()`, `is_tag_name()` methods
- [ ] **M15.14** Update `RequiredKeyword.position` and `ChoiceAt.position` from `i64` to `SplitPosition`
- [ ] **M15.15** Update `dataflow/constraints.rs` to emit `SplitPosition` values
- [ ] **M15.16** Evaluate `Index` enum in `domain.rs` — consolidate with or map to `SplitPosition`
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

- **`-q` with `--features parser` quirk**: Shows 0 tests due to cargo output formatting. Use `--all-features` or drop `-q` to see actual counts.
- **Corpus vs fabricated**: Real Django functions don't always match assumed patterns. Always verify corpus function signatures before replacing tests.
