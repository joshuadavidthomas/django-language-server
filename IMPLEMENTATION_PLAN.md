# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | **done** | Test baseline + corpus-grounded tests |
| M15 | **done** | Return values, not mutation (+ domain types T1-T4) |
| M16 | **done** | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | **ready** | Decompose blocks.rs into strategy modules |
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

- [x] **M16.17** Evaluate whether `KnownOptions` should be renamed to `OptionLoop`. Check type shape, usage sites, and whether rename adds clarity. Document decision.
- [x] **M16.18** Decision: **keep `KnownOptions`**, do not rename.
- [x] **M16.19** Validate: no code changes needed — baseline already green.

**M16.17 Decision Notes:**

The ROADMAP proposed `OptionLoop` as a "first-class type for while-loop option parsing." However,
`KnownOptions` already *is* a first-class type — it was extracted from the `ctx.known_options`
side-channel in M15-M16 Phase 1. The question is purely whether the name should change.

**Keep `KnownOptions` because:**
1. The struct fields (`values`, `allow_duplicates`, `rejects_unknown`) describe option *semantics*,
   not loop structure. `KnownOptions` matches this domain.
2. At the consumption site (`djls-semantic/rule_evaluation.rs`), the consumer cares about "what
   options does this tag accept?" — `KnownOptions` communicates that directly.
3. `OptionLoop` describes *extraction origin* (a while-loop pattern), which is an implementation
   detail irrelevant to consumers.
4. The extraction function `try_extract_option_loop()` already captures the loop-detection concept;
   the returned type should describe the *result*, not the *source*.
5. Renaming crosses crate boundaries (`djls-semantic`) for no semantic gain.

### Phase 5: Final validation

- [x] **M16.20** Full suite: `cargo test -q` — all green (745 passed, 0 failed, 7 ignored)
- [x] **M16.21** Verify: no `ctx.constraints` mutation, no `ctx.known_options` mutation (one comment reference only)
- [x] **M16.22** Verify: `AnalysisContext` type no longer exists (renamed to `CallContext`)
- [x] **M16.23** Verify: public API unchanged (`analyze_compile_function()` → `TagRule`)

## M17 — Decompose blocks.rs into strategy modules

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 3), `docs/dev/extraction-type-driven-vision.md` (`BlockEvidence`)

**Plan file:** `.agents/plans/2026-02-09-m17-decompose-blocks.md`

Split `blocks.rs` (1382 lines) into strategy modules under `blocks/`. Each strategy gets its own module with a `detect()` entry point. Public API unchanged: `blocks::extract_block_spec()`.

Module convention: `blocks.rs` (orchestrator) + `blocks/` directory (strategy submodules). NOT `blocks/mod.rs`.

### Phase 1: Create blocks/ directory and move opaque strategy

- [x] **M17.1** Create `blocks/opaque.rs` with `detect(body, parser_var) -> Option<BlockTagSpec>`. Move `collect_skip_past_tokens()` and `extract_skip_past_token()`.
- [x] **M17.2** Update `blocks.rs`: add `mod opaque;`, call `opaque::detect()` in orchestrator. Keep `is_parser_receiver()` in `blocks.rs` as shared helper. (Already done in M17.1 — mod declaration and orchestrator call were part of the same commit.)
- [x] **M17.3** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` — all green (745 passed, 0 failed, 7 ignored).

### Phase 2: Move dynamic_end strategy

- [x] **M17.4** Create `blocks/dynamic_end.rs` with `detect(body, parser_var) -> Option<BlockTagSpec>`. Move `has_dynamic_end_in_body()`, `is_dynamic_end_parse_call()`, `is_end_fstring()`, `has_dynamic_end_tag_format()`, `is_end_format_expr()`.
- [x] **M17.5** Update `blocks.rs`: add `mod dynamic_end;`, call `dynamic_end::detect()` in orchestrator. `has_dynamic_end_tag_format` and `is_end_fstring` made `pub(super)` for reuse by `next_token` strategy.
- [x] **M17.6** Validate: all green (745 passed, 0 failed, 7 ignored).

### Phase 3: Move next_token strategy

- [x] **M17.7** Create `blocks/next_token.rs` with `detect(body, parser_var) -> Option<BlockTagSpec>`. Move `extract_next_token_loop_spec()`, `has_next_token_loop()`, `is_parser_tokens_check()`, `body_has_next_token_call()`, `is_next_token_call()`, `collect_token_content_comparisons()`, `extract_comparisons_from_expr()`.
- [x] **M17.8** Update `blocks.rs`: add `mod next_token;`. Made `is_token_contents_expr` `pub(crate)` so `next_token` module can use it via `super::is_token_contents_expr`.
- [x] **M17.9** Validate: all green (745 passed, 0 failed, 7 ignored).

### Phase 4: Move parse_calls strategy

- [ ] **M17.10** Create `blocks/parse_calls.rs` with `detect(body, parser_var) -> Option<BlockTagSpec>`. Move `ParseCallInfo`, `Classification`, `collect_parser_parse_calls()`, `extract_parse_call_info()`, `classify_stop_tokens()`, `classify_in_body()`, `classify_from_if_chain()`, `extract_token_check()`, `extract_startswith_check()`, `is_token_contents_expr()`, `body_has_parse_call()`.
- [ ] **M17.11** Move `extract_string_sequence()` to `blocks.rs` shared helpers (used by multiple strategies).
- [ ] **M17.12** Update `blocks.rs`: add `mod parse_calls;`.
- [ ] **M17.13** Validate: all green.

### Phase 5: Clean up orchestrator and evaluate BlockEvidence

- [ ] **M17.14** Verify `blocks.rs` is compact (~50-100 lines): orchestrator + shared helpers + mod declarations only.
- [ ] **M17.15** Evaluate `BlockEvidence` enum: does the module structure already provide clean separation? Document decision.
- [ ] **M17.16** Final validation: `cargo test -q` — all green (745+ passed, 0 failed). Verify public API unchanged.

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

## Current Test Counts (M16 complete)

| Suite | Passed |
|-------|--------|
| Unit tests (djls-extraction) | 252 |
| Corpus integration (djls-extraction) | 2 |
| **djls-extraction total** | **254** |
| **Full workspace** | **745 passed, 0 failed, 7 ignored** |

Snapshot files: 185

