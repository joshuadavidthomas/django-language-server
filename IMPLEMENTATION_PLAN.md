# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | **done** | Test baseline + corpus-grounded tests |
| M15 | **done** | Return values, not mutation (+ domain types T1-T4) |
| M16 | **done** | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | **done** | Decompose blocks.rs into strategy modules |
| M18 | **ready** | Move environment scanning to djls-project |
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

- [x] **M17.10** Create `blocks/parse_calls.rs` with `detect(body, parser_var) -> Option<BlockTagSpec>`. Move `ParseCallInfo`, `Classification`, `collect_parser_parse_calls()`, `extract_parse_call_info()`, `classify_stop_tokens()`, `classify_in_body()`, `classify_from_if_chain()`, `extract_token_check()`, `extract_startswith_check()`, `body_has_parse_call()`. Shared helpers `is_token_contents_expr` and `extract_string_sequence` remain in `blocks.rs`.
- [x] **M17.11** `extract_string_sequence()` kept in `blocks.rs` as `pub(super)` shared helper. `is_token_contents_expr` also stays (used by `next_token` and `parse_calls`).
- [x] **M17.12** Added `mod parse_calls;` to `blocks.rs`. Orchestrator now calls `parse_calls::detect()` — equivalent flow to before (returns None when no parse calls found, falling through to dynamic_end and next_token).
- [x] **M17.13** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` — all green (745 passed, 0 failed, 7 ignored).

### Phase 5: Clean up orchestrator and evaluate BlockEvidence

- [x] **M17.14** Verify `blocks.rs` is compact (~50-100 lines): orchestrator + shared helpers + mod declarations only.
- [x] **M17.15** Evaluate `BlockEvidence` enum: does the module structure already provide clean separation? Document decision.
- [x] **M17.16** Final validation: `cargo test -q` — all green (745+ passed, 0 failed). Verify public API unchanged.

**M17.14 Verification Notes:**

`blocks.rs` is 396 lines total: 122 lines production code + 274 lines tests. The production
portion breaks down as: ~20-line orchestrator (`extract_block_spec`), mod declarations/imports
(~35 lines), and 3 shared helper functions (~67 lines: `is_parser_receiver`,
`extract_string_sequence`, `is_token_contents_expr`). Slightly above the 50-100 target for
production code due to the shared helpers, but these must stay in the parent module since they're
used across multiple strategy submodules. The orchestrator itself is compact.

Strategy module sizes: `opaque.rs` (113), `dynamic_end.rs` (181), `next_token.rs` (261),
`parse_calls.rs` (432). Total decomposed: 987 lines across strategy modules. Original monolith
was 1382 lines; current total (blocks.rs + submodules) is 1383 lines — essentially identical,
confirming this was a pure structural refactor with no logic changes.

**M17.15 BlockEvidence Decision: Do not introduce.**

The module decomposition already provides the separation between observation and interpretation
that `BlockEvidence` was designed to achieve. Each strategy module encapsulates its own
detection-to-`BlockTagSpec` path in its `detect()` function. Introducing `BlockEvidence` would
move interpretation logic OUT of strategy modules into the orchestrator — the opposite direction
of the clean separation we just achieved. The modules ARE the evidence types: `opaque::detect()`,
`parse_calls::detect()`, `dynamic_end::detect()`, `next_token::detect()` each represent a
distinct evidence pattern. An enum wrapper would add indirection without testability or
maintainability benefit.

**M17.16 Final Validation:**

745 passed, 0 failed, 7 ignored. Public API unchanged: `extract_block_spec` exported from
`lib.rs`, called in `registry.rs`. Build, clippy, and tests all green.

## M18 — Move environment scanning to djls-project

**Plan file:** `.agents/plans/2026-02-09-m18-move-env-scanning.md`

Move `environment/scan.rs` from `djls-extraction` to `djls-project`. Types (`EnvironmentInventory`, `EnvironmentLibrary`, `EnvironmentSymbol`) stay in `djls-extraction`.

### Phase 1: Create scanning module in djls-project

- [x] **M18.1** Create `crates/djls-project/src/scanning.rs` with scan functions moved from `environment/scan.rs`. Update imports to use `djls_extraction::` for types and `registry::collect_registrations_from_body`.
- [x] **M18.2** Add `mod scanning;` and public re-exports in `crates/djls-project/src/lib.rs`. Update `djls-project/Cargo.toml`: ensure `djls-extraction` dep has `features = ["parser"]`, add `ruff_python_parser` workspace dep.
- [x] **M18.3** Validate: `cargo build -q`, `cargo test -q` — all green. 761 passed (745 + 16 scan tests now in djls-project), 0 failed, 7 ignored.

### Phase 2: Update consumers

- [ ] **M18.4** Update `djls-server/src/db.rs` to import `scan_environment_with_symbols` from `djls_project` instead of `djls_extraction`.
- [ ] **M18.5** Validate: `cargo build -q`, `cargo test -q` — all green.

### Phase 3: Remove from djls-extraction

- [ ] **M18.6** Delete `crates/djls-extraction/src/environment/scan.rs`. Update `environment.rs` to remove `mod scan` and scan re-exports. Update `lib.rs` to remove `pub use environment::scan_environment*`.
- [ ] **M18.7** Ensure `collect_registrations_from_body` and `SymbolKind` are public in `djls-extraction` (needed by `djls-project/scanning.rs`).
- [ ] **M18.8** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` — all green (745+ passed, 0 failed).

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

