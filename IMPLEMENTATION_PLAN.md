# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | **in-progress** | Test baseline + corpus-grounded tests |
| M15 | stub | Return values, not mutation (+ domain types T1-T4) |
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
- [ ] **M14.17** Run `cargo insta test --accept --unreferenced delete -p djls-extraction` to clean up orphaned snapshots
- [ ] **M14.18** Validate: `cargo test -q -p djls-extraction`, no orphaned snapshot files, `cargo clippy -q --all-targets --all-features -- -D warnings` clean

### Phase 6: Validation — Full Suite Green

- [ ] **M14.19** Run full suite: `cargo build -q`, `cargo test -q`, `cargo clippy -q --all-targets --all-features -- -D warnings` — all green across all crates
- [ ] **M14.20** Update baseline counts in this file with final numbers, mark M14 as "done" in progress table

## M15 — Return values, not mutation (+ domain types T1-T4)

**Design docs:** `docs/dev/extraction-refactor-plan.md` (Phase 1), `docs/dev/extraction-type-driven-vision.md`

_Tasks not yet expanded. Needs plan file: `.agents/plans/2026-02-09-m15-return-values.md`_

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

## Current Test Counts (after Phase 4)

| Suite | Passed |
|-------|--------|
| Unit tests | 245 |
| Corpus integration | 2 |
| **Total** | **247** |

## M14.2 Audit — lib.rs Golden Tests (Phase 5 reference)

The full audit was completed in M14.2. Phases 3-4 consumed audit results for `registry.rs`, `blocks.rs`, `filters.rs`, `signature.rs`, `constraints.rs`, `eval.rs`, `calls.rs`, and `scan.rs`. The `lib.rs` section below is the remaining work for Phase 5.

**Audit summary for completed files:** 78 tests replaced with corpus source, 95 kept with justification, 0 removed, 72 pure Rust. See git history (commit `8cf9415d`) for full audit.

### `src/lib.rs` (48 tests)

Golden end-to-end tests — highest-value candidates for corpus replacement.

| Test | Category | Notes |
|------|----------|-------|
| `smoke_test_ruff_parser` | (d) | Tests parser works at all |
| `extract_rules_simple_tag` | (a) | Full pipeline — use real `simple_tag` from corpus |
| `extract_rules_filter` | (a) | Full pipeline — use real filter from corpus |
| `extract_rules_filter_with_arg` | (a) | Full pipeline — use real filter with arg |
| `extract_rules_block_tag` | (a) | Full pipeline — use real block tag |
| `extract_rules_empty_source` | (b) | Edge case — keep |
| `extract_rules_invalid_python` | (b) | Edge case — keep |
| `extract_rules_no_registrations` | (b) | Edge case — keep |
| `extract_rules_multiple_registrations` | (a) | Use real module with multiple registrations |
| `extract_rules_call_style_registration_no_func_def` | (b) | Edge case — call-style with missing function. Keep |
| `golden_decorator_bare_tag` | (a) | Use real `@register.tag` from corpus |
| `golden_decorator_tag_with_explicit_name` | (a) | Use real named tag from corpus |
| `golden_decorator_tag_with_name_kwarg` | (a) | Use real `name=` kwarg tag |
| `golden_simple_tag_no_args` | (a) | Use real no-arg simple_tag |
| `golden_simple_tag_with_args` | (a) | Use real simple_tag with args |
| `golden_simple_tag_takes_context` | (a) | Use real `takes_context=True` simple_tag |
| `golden_inclusion_tag` | (a) | Use real inclusion_tag |
| `golden_inclusion_tag_takes_context` | (a) | Use real inclusion_tag with takes_context |
| `golden_call_style_registration` | (a) | Use real call-style registration from `defaulttags.py` |
| `golden_filter_bare_decorator` | (a) | Use real `@register.filter` |
| `golden_filter_with_name_kwarg` | (a) | Use real filter with `name=` |
| `golden_filter_is_safe` | (a) | Use real filter with `is_safe=True` |
| `golden_multiple_registrations` | (a) | Use real multi-registration module |
| `golden_len_exact_check` | (a) | Use real tag with `len(bits) != N` |
| `golden_len_min_check` | (a) | Use real tag with `len(bits) < N` |
| `golden_len_max_check` | (a) | Use real tag with `len(bits) > N` |
| `golden_len_not_in_check` | (a) | Use real tag with `len(bits) not in (...)` |
| `golden_keyword_position_check` | (a) | Use real tag with `bits[N] != "keyword"` |
| `golden_option_loop` | (a) | Use real tag with while-loop option parsing (e.g., `include`) |
| `golden_non_bits_variable` | (b) | Tests tag that doesn't use split_contents — keep |
| `golden_multiple_raise_statements` | (a) | Use real tag with multiple raises |
| `golden_simple_block` | (a) | Use real block tag (e.g., `for`) |
| `golden_block_with_intermediates` | (a) | Use real tag with intermediates (e.g., `if`/`elif`/`else`) |
| `golden_opaque_block` | (a) | Use real opaque block (e.g., `verbatim`) |
| `golden_for_tag_with_empty` | (a) | Use real `for`+`empty` from `defaulttags.py` |
| `golden_filter_no_arg` | (a) | Use real no-arg filter |
| `golden_filter_required_arg` | (a) | Use real required-arg filter |
| `golden_filter_optional_arg` | (a) | Use real optional-arg filter |
| `golden_filter_method_style` | (b) | `self` parameter — not standard Django. Keep |
| `golden_no_split_contents` | (b) | Tag without split_contents — keep |
| `golden_dynamic_end_tag` | (a) | Use real tag with dynamic end-tag (e.g., `block`) |
| `golden_empty_source` | (b) | Edge case — keep |
| `golden_invalid_python` | (b) | Edge case — keep |
| `golden_no_registrations` | (b) | Edge case — keep |
| `golden_call_style_no_func_def` | (b) | Edge case — keep |
| `golden_mixed_library` | (a) | Use real module with tags + filters |
| `golden_simple_tag_with_name_kwarg` | (a) | Use real named simple_tag |
| `golden_inclusion_tag_with_args` | (a) | Use real inclusion_tag with args |

**Phase 5 scope:** 31 tests to replace with corpus source (category a), 17 to keep as-is (categories b/d).

## Discoveries

- **`-q` with `--features parser` quirk**: Shows 0 tests due to cargo output formatting. Use `--all-features` or drop `-q` to see actual counts.
- **Audit corrections**: Several audit (a) classifications were wrong — real Django functions don't always match assumed patterns. Always verify corpus function signatures before replacing tests. Examples: `default` filter has required `arg` (not optional), `truncatewords` has required `arg` (not optional), `defaultfilters.py` doesn't use `name=` kwarg on `@register.filter`.
