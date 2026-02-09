# Implementation Plan — Extraction Crate Refactor (M14-M20)

**Source of truth:** `.agents/ROADMAP.md` (milestones M14-M20), `.agents/rfcs/2026-02-09-rfc-extraction-salsa-integration.md`

**Branch:** `eval-intent-opus-4.6`

## Progress

| Milestone | Status | Description |
|-----------|--------|-------------|
| M14 | planned | Test baseline + corpus-grounded tests |
| M15 | stub | Return values, not mutation (+ domain types T1-T4) |
| M16 | stub | Split god-context (+ CompileFunction, OptionLoop) |
| M17 | stub | Decompose blocks.rs into strategy modules |
| M18 | stub | Move environment scanning to djls-project |
| M19 | stub | HelperCache → Salsa tracked functions |
| M20 | stub | Rename djls-extraction → djls-python |

## M14 — Test baseline + corpus-grounded tests

**Design docs:** `docs/dev/extraction-test-strategy.md`, `docs/dev/corpus-refactor.md`
**Plan file:** `.agents/plans/2026-02-09-m14-test-baseline.md`

### Phase 1: Record Baseline + Audit Fabricated Tests

- [x] **M14.1** Record baseline test counts: run `cargo test -q -p djls-extraction` and `cargo test -q -p djls-extraction --test corpus`, record total test count (pass/fail/ignored), total snapshot count, and corpus test count in this file
- [x] **M14.2** Audit fabricated Python tests across all extraction source files: categorize each test as (a) has corpus equivalent → replace, (b) pattern is real but no clean isolatable corpus example → keep with comment, or (c) pattern doesn't exist in real code → remove. Record audit results as a section below

### Phase 2: Create Corpus Test Helpers

- [x] **M14.3** Add corpus test helpers to extraction crate test utilities: `find_function_in_source()`, `corpus_function()`, `corpus_source()` that work with `Corpus::discover()` and skip gracefully when corpus is not synced
- [x] **M14.4** Validate: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings` pass, no test behavior changes

### Phase 3: Replace Fabricated Tests — Registration & Blocks & Filters

- [x] **M14.5** Replace fabricated Python in `src/registry.rs` with corpus-sourced equivalents (map each registration pattern to a real Django function). 12 tests now use corpus source (defaulttags.py, defaultfilters.py, testtags.py, inclusion.py, custom.py, wagtailadmin_tags.py). 7 tests kept as fabricated with justification comments (edge cases, rare API patterns). Net +1 test (240 total). Removed `decorator_filter_with_name_kwarg` — the audit incorrectly claimed `defaultfilters.py` has `name=` kwarg; replaced with `decorator_filter_with_positional_string_name` (corpus: `@register.filter("escapejs")`) and added `tag_with_name_kwarg` (corpus: `@register.tag(name="partialdef")`) and `mixed_decorator_and_call_style` (corpus: testtags.py)
- [x] **M14.6** Replace fabricated Python in `src/blocks.rs` with corpus-sourced equivalents. 8 tests now use corpus source: `verbatim` (simple end-tag), `do_if` (intermediates), `comment` (opaque/skip_past), `do_for` (multiple parse calls), `now` (no block structure), `do_block` from loader_tags.py (endblock validation), `spaceless` (simple parse+delete), `do_block_translate` from i18n.py (next_token loop). Removed 2 duplicate tests (`django_if_tag_style` duplicated `if_else_intermediates`; `skip_past_string_constant` duplicated `opaque_block_skip_past`). Added 1 new corpus test (`simple_block_with_endblock_validation`). Net -1 test (239 total). Reclassified `dynamic_fstring_end_tag` and `convention_tiebreaker_single_call_multi_token` from (a) to (b) — no corpus function has f-string directly in parser.parse(), and no corpus function has only a single parse call with mixed tokens.
- [ ] **M14.7** Replace fabricated Python in `src/filters.rs` with corpus-sourced equivalents (filter arity from defaultfilters.py)
- [ ] **M14.8** Replace fabricated Python in `src/signature.rs` with corpus-sourced equivalents (simple_tag/inclusion_tag parameter patterns)
- [ ] **M14.9** Update and review snapshots — extraction results must be equivalent. Run `cargo insta test --accept -p djls-extraction` and review diffs
- [ ] **M14.10** Validate: `cargo test -q -p djls-extraction`, `cargo clippy -q --all-targets --all-features -- -D warnings` clean

### Phase 4: Replace Fabricated Tests — Dataflow

- [ ] **M14.11** Replace fabricated Python in `src/dataflow/constraints.rs` that models Django guard patterns with corpus-sourced equivalents. Keep inherently unit-level constraint logic tests as fabricated with justification comments
- [ ] **M14.12** Replace fabricated Python in `src/dataflow/eval.rs` that models Django compile function patterns with corpus-sourced equivalents. Keep pure unit tests (abstract value arithmetic, env operations) as fabricated with justification
- [ ] **M14.13** Replace fabricated Python in `src/dataflow/calls.rs` with corpus-sourced equivalents (helper function inlining patterns, e.g. allauth parse_tag)
- [ ] **M14.14** Replace fabricated Python in `src/environment/scan.rs` with corpus-sourced equivalents (AST scanning for registration patterns)
- [ ] **M14.15** Validate: `cargo test -q -p djls-extraction`, snapshots reviewed, `cargo clippy -q --all-targets --all-features -- -D warnings` clean

### Phase 5: Replace Fabricated Tests — Golden/End-to-End

- [ ] **M14.16** Audit and replace fabricated Python in `src/lib.rs` golden tests with corpus-sourced equivalents. Keep edge case tests (malformed registrations, error handling) as fabricated with documented justification
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

## M14.2 Audit — Fabricated Test Categorization

Categories:
- **(a) Has corpus equivalent → replace** — pattern exists in corpus, use real source
- **(b) Real pattern, no clean isolatable example → keep** — pattern exists in real code but corpus doesn't have a clean isolatable example, or the test is testing a specific edge case of real behavior
- **(c) Pattern doesn't exist in real code → remove** — fabricated pattern, not found in real Django/third-party code
- **(d) Pure Rust logic → keep as-is** — no Python involved, tests Rust types/logic

### `src/types.rs` (13 tests)

All **(d) Pure Rust logic**. Tests `SymbolKey` construction, `ExtractionResult` merge, `BlockTagSpec` fields, `FilterArity` fields, `rekey_module`. No Python source involved.

### `src/dataflow/domain.rs` (4 tests)

All **(d) Pure Rust logic**. Tests `Env` creation, set/get, mutation. No Python source.

### `src/registry.rs` (20 tests)

All test registration discovery from Python source using fabricated snippets.

| Test | Category | Corpus equivalent |
|------|----------|-------------------|
| `decorator_bare_tag` | (a) | `do_for` in `defaulttags.py` uses `@register.tag` (via call-style, but bare decorator exists on other functions) |
| `decorator_simple_tag_with_name_kwarg` | (a) | Third-party packages use `@register.simple_tag(name=...)` |
| `decorator_inclusion_tag` | (a) | Wagtail, crispy-forms use `@register.inclusion_tag(...)` |
| `decorator_filter_bare` | (a) | `defaultfilters.py` has `@register.filter` on many functions |
| `decorator_filter_with_name_kwarg` | (a) | `defaultfilters.py` has `@register.filter(name="floatformat")` etc. |
| `call_style_tag_registration` | (a) | `defaulttags.py` uses `register.tag("for", do_for)` |
| `call_style_filter_registration` | (a) | `defaultfilters.py` uses call-style filter registration |
| `function_name_fallback` | (a) | `@register.tag()` with empty parens — exists in corpus |
| `multiple_registrations` | (a) | Any templatetags module has multiple registrations |
| `tag_with_positional_string_name` | (a) | `@register.tag("name")` pattern in `defaulttags.py` |
| `call_style_tag_with_method_callable` | (b) | `register.tag("name", SomeClass.method)` — not in standard Django but plausible in third-party. Keep with comment |
| `simple_tag_func_positional` | (b) | `register.simple_tag(func, name=...)` call-style — rare but valid API. Keep with comment |
| `simple_block_tag_decorator` | (b) | `@register.simple_block_tag` — Django 5.2+ feature, may not be in corpus yet. Keep with comment |
| `empty_source` | (b) | Edge case — keep as-is |
| `no_registrations` | (b) | Edge case — keep as-is |
| `filter_with_positional_string_name` | (a) | `@register.filter("name")` exists in `defaultfilters.py` |
| `filter_with_is_safe_kwarg` | (a) | `@register.filter(is_safe=True)` in `defaultfilters.py` |
| `call_style_single_func_no_name` | (b) | `register.tag(func)` with no name — valid API but rare. Keep with comment |
| `call_style_filter_single_func_no_name` | (b) | `register.filter(func)` — valid API but rare. Keep with comment |
| `name_kwarg_overrides_positional_for_tag` | (b) | Edge case of name resolution priority. Keep as unit test |

### `src/blocks.rs` (18 tests)

All test block spec extraction from Python source.

| Test | Category | Corpus equivalent |
|------|----------|-------------------|
| `simple_end_tag_single_parse` | (a) | `do_for` in `defaulttags.py` |
| `if_else_intermediates` | (a) | `do_if` in `defaulttags.py` |
| `opaque_block_skip_past` | (a) | `do_comment` / `verbatim` in `defaulttags.py` |
| `non_conventional_closer_found_via_control_flow` | (b) | "done" as end-tag — not in Django core but tests the classification algorithm. Keep |
| `ambiguous_returns_none_for_end_tag` | (b) | Edge case — no corpus equivalent but tests classification logic. Keep |
| `dynamic_fstring_end_tag` | (a) | `do_block` in `defaulttags.py` uses `f"end{tag_name}"` |
| `multiple_parse_calls_classify_correctly` | (a) | `do_for` with `empty` intermediate in `defaulttags.py` |
| `no_parse_calls_returns_none` | (a) | `do_now` in `defaulttags.py` (non-block tag) |
| `self_parser_pattern` | (b) | classytags-style `self.parser` — not in standard Django corpus. Keep with comment |
| `convention_tiebreaker_single_call_multi_token` | (a) | Pattern of `parser.parse(("else", "endif"))` from `defaulttags.py` |
| `django_if_tag_style` | (a) | Directly models Django's `do_if` |
| `skip_past_string_constant` | (a) | `do_comment` in `defaulttags.py` |
| `no_parameters_returns_none` | (b) | Edge case — keep |
| `sequential_parse_then_check` | (a) | `do_spaceless` in `defaulttags.py` |
| `next_token_loop_blocktrans_pattern` | (a) | `do_block_translate` in `i18n.py` |
| `next_token_loop_static_end_tag` | (b) | Variation with static end-tag — real pattern but the specific combination is fabricated. Keep |
| `next_token_loop_with_intermediate_and_static_end` | (b) | Variation combining intermediate + static end — fabricated combination. Keep |
| `no_next_token_loop_no_parse_returns_none` | (b) | Duplicate of `no_parse_calls_returns_none` — keep as edge case |

### `src/filters.rs` (17 tests)

All test filter arity extraction from function signatures.

| Test | Category | Corpus equivalent |
|------|----------|-------------------|
| `no_arg_filter` | (a) | `title` in `defaultfilters.py` |
| `no_arg_filter_upper` | (a) | `upper` in `defaultfilters.py` |
| `required_arg_filter` | (a) | `cut` in `defaultfilters.py` |
| `required_arg_filter_add` | (a) | `add` in `defaultfilters.py` |
| `optional_arg_filter` | (a) | `default` in `defaultfilters.py` |
| `optional_arg_filter_none_default` | (a) | `truncatewords` in `defaultfilters.py` |
| `method_style_no_arg` | (b) | `self` parameter — not standard Django but valid Python. Keep |
| `method_style_with_arg` | (b) | Same — `self` variation. Keep |
| `method_style_with_optional_arg` | (b) | Same — `self` variation. Keep |
| `no_params_at_all` | (b) | Edge case — no real filter has zero params. Keep as robustness test |
| `self_only` | (b) | Edge case — keep |
| `posonly_params` | (b) | Python 3.8+ positional-only — no Django filter uses this currently. Keep with comment |
| `posonly_with_default` | (b) | Same — keep |
| `multiple_extra_args_all_with_defaults` | (b) | Unusual — no real filter has 3+ params. Keep as edge case |
| `multiple_extra_args_mixed_defaults` | (b) | Same — keep |
| `is_safe_does_not_affect_arity` | (a) | `defaultfilters.py` uses `is_safe=True` on many filters |
| `stringfilter_does_not_affect_arity` | (a) | `@stringfilter` decorator in `defaultfilters.py` |

### `src/signature.rs` (5 tests)

Test `simple_tag`/`inclusion_tag` parameter extraction.

| Test | Category | Corpus equivalent |
|------|----------|-------------------|
| `simple_tag_no_params` | (a) | Find no-param `simple_tag` in corpus (e.g., `now` equivalent) |
| `simple_tag_required_params` | (a) | Find multi-param `simple_tag` in corpus |
| `simple_tag_with_defaults` | (a) | Find `simple_tag` with defaults in corpus |
| `simple_tag_with_varargs` | (b) | `*args` on simple_tag — uncommon. Keep with comment |
| `simple_tag_takes_context` | (a) | `takes_context=True` pattern exists in corpus |

### `src/dataflow/constraints.rs` (31 tests)

Test constraint extraction from guard conditions.

| Test | Category | Corpus equivalent |
|------|----------|-------------------|
| `len_lt` | (a) | `len(bits) < N` guards in `defaulttags.py` |
| `len_ne` | (a) | `len(bits) != N` in `defaulttags.py` |
| `len_gt` | (a) | `len(bits) > N` in `defaulttags.py` |
| `len_le` | (a) | Exists in Django tag guards |
| `len_ge` | (a) | Exists in Django tag guards |
| `reversed_lt` | (b) | `N > len(bits)` — reversed comparison. Tests comparator normalization. Keep |
| `reversed_gt` | (b) | Same — reversed comparison. Keep |
| `required_keyword_ne` | (a) | `bits[1] != "as"` pattern in `regroup`, `cycle` etc. |
| `required_keyword_backward` | (b) | `"as" != bits[1]` — reversed string comparison. Keep |
| `compound_or` | (a) | Compound guard conditions in Django tags |
| `compound_and_discards_length` | (b) | Tests `and` semantics — unit logic. Keep |
| `negated_range` | (a) | `not (2 <= len(bits) <= 4)` in Django tags |
| `len_not_in` | (a) | `len(bits) not in (2, 3)` in Django tags |
| `offset_adjustment_after_slice` | (b) | Tests internal offset logic. Keep as unit test |
| `multiple_raises` | (a) | Multiple `raise TemplateSyntaxError` in one function — common in Django |
| `nested_if_raise` | (a) | Nested `if` with raise — common pattern |
| `elif_raise` | (a) | `elif` with raise — common pattern |
| `non_template_syntax_error_ignored` | (b) | Tests that non-TSE raises are ignored. Keep as unit test |
| `regroup_pattern_end_to_end` | (a) | Directly models Django's `regroup` tag |
| `unknown_variable_produces_no_constraint` | (b) | Tests robustness for unknown variables. Keep |
| `keyword_from_reversed_comparison` | (b) | `"keyword" != bits[N]` reversed. Keep |
| `star_unpack_then_constraint` | (a) | `tag_name, *rest = bits` then guard — exists in Django |
| `pop_0_offset_adjusted_constraint` | (a) | `bits.pop(0)` pattern in Django tags |
| `end_pop_adjusted_constraint` | (a) | `bits.pop()` (from end) pattern |
| `combined_pop_front_and_end` | (b) | Both pop(0) and pop() — tests offset arithmetic. Keep |
| `pop_0_with_assignment_then_constraint` | (a) | `x = bits.pop(0)` pattern in Django |
| `choice_at_not_in_tuple` | (a) | `bits[N] not in ("on", "off")` — `autoescape` in `defaulttags.py` |
| `choice_at_autoescape_pattern` | (a) | Directly models `autoescape` |
| `choice_at_with_list` | (b) | List instead of tuple — defensive test. Keep |
| `choice_at_negative_index` | (b) | `bits[-1] not in (...)` — tests negative index handling. Keep |
| `no_choice_at_for_single_string` | (b) | Single string → RequiredKeyword, not ChoiceAt. Keep |

### `src/dataflow/eval.rs` (48 tests)

Test abstract interpreter statement/expression evaluation. Many are inherently unit-level.

| Test | Category | Notes |
|------|----------|-------|
| `env_initialization` | (d) | Pure Rust — Env setup |
| `split_contents_binding` | (d) | Tests abstract value for `token.split_contents()` |
| `contents_split_binding` | (d) | Tests abstract value for `token.contents.split()` |
| `parser_token_split_contents` | (b) | Tests `parser.token.split_contents()` — fabricated but real pattern |
| `subscript_forward` | (d) | Tests `bits[0]` → abstract value |
| `subscript_negative` | (d) | Tests `bits[-1]` → abstract value |
| `slice_from_start` | (d) | Tests `bits[1:]` → abstract value |
| `slice_with_existing_offset` | (d) | Tests nested slice |
| `len_of_split_result` | (d) | Tests `len(bits)` → abstract value |
| `list_wrapping` | (d) | Tests `list(bits)` — pure eval |
| `star_unpack` | (d) | Tests `name, *rest = bits` |
| `tuple_unpack` | (d) | Tests `a, b, c = bits` |
| `contents_split_none_1` | (d) | Tests `contents.split(None, 1)` |
| `unknown_variable` | (d) | Tests unknown var → Unknown |
| `split_result_tuple_unpack_no_star` | (d) | Tests fixed-length tuple unpack |
| `subscript_with_offset` | (d) | Tests subscript on sliced result |
| `if_branch_updates_env` | (d) | Tests if-branch env propagation |
| `integer_literal` | (d) | Tests `x = 42` |
| `string_literal` | (d) | Tests `x = "hello"` |
| `slice_truncation_preserves_offset` | (d) | Tests `bits[:3]` on already-offset value |
| `star_unpack_with_trailing` | (d) | Tests `*rest, last = bits` |
| `pop_0_offset` | (d) | Tests `bits.pop(0)` side effect |
| `pop_0_with_assignment` | (d) | Tests `x = bits.pop(0)` |
| `pop_from_end` | (d) | Tests `bits.pop()` |
| `pop_from_end_with_assignment` | (d) | Tests `x = bits.pop()` |
| `multiple_pops` | (d) | Tests sequence of pops |
| `len_after_pop` | (d) | Tests `len(bits)` after pop |
| `len_after_end_pop` | (d) | Tests `len(bits)` after `pop()` |
| `option_loop_basic` | (a) | While-loop option pattern — models `include` tag in `defaulttags.py` |
| `option_loop_with_duplicate_check` | (a) | Duplicate option detection — models real Django option parsing |
| `option_loop_allows_unknown` | (b) | Unknown option tolerance — fabricated but tests real behavior |
| `option_loop_include_pattern` | (a) | Directly models Django's `include` tag option loop |
| `no_option_loop_returns_none` | (d) | Edge case — no option loop present |
| `match_partialdef_pattern` | (a) | Match statement pattern — models Django 5.2+ `match` usage if present |
| `match_partial_exact` | (b) | Match with exact pattern — keep for completeness |
| `match_non_split_result_no_constraints` | (d) | Tests that non-split match produces no constraints |
| `match_star_pattern_variable_length` | (b) | Star pattern in match — keep |
| `match_multiple_valid_lengths` | (b) | Multiple case patterns — keep |
| `match_all_error_cases_no_constraints` | (d) | Edge case — all cases raise |
| `match_wildcard_overrides_variable_min_to_zero` | (b) | Wildcard `_` case — keep |
| `match_wildcard_after_fixed_produces_no_min` | (b) | Wildcard after fixed — keep |
| `match_env_updates_propagate` | (d) | Tests env propagation through match |
| `while_body_assignments_propagate` | (d) | Tests while-body env propagation |
| `while_body_pop_side_effects` | (d) | Tests pop side effects in while |
| `contents_split_none_2_is_not_tuple` | (d) | Tests `split(None, 2)` is not tuple-unpackable |
| `contents_split_none_0_is_not_tuple` | (d) | Tests `split(None, 0)` edge case |
| `contents_split_none_variable_is_not_tuple` | (d) | Tests `split(None, var)` |
| `while_option_loop_skips_body_processing` | (d) | Tests that recognized option loops skip body eval |

### `src/dataflow/calls.rs` (14 tests)

Test helper function call inlining.

| Test | Category | Corpus equivalent |
|------|----------|-------------------|
| `simple_helper_returns_split_contents` | (b) | Fabricated but tests basic helper return. Keep |
| `tuple_return_destructuring` | (b) | Tests tuple return from helper. Keep |
| `allauth_parse_tag_pattern` | (a) | Directly models allauth's `parse_tag` helper |
| `depth_limit` | (d) | Tests recursion depth limiting — pure logic |
| `self_recursion` | (d) | Tests self-recursive helper detection |
| `helper_not_found` | (d) | Tests missing helper graceful handling |
| `token_kwargs_marks_unknown` | (d) | Tests `token_kwargs()` → Unknown |
| `parser_compile_filter` | (d) | Tests `parser.compile_filter()` → Unknown |
| `cache_hit_same_args` | (d) | Tests HelperCache hit behavior |
| `cache_miss_different_args` | (d) | Tests HelperCache miss behavior |
| `helper_with_pop_and_return` | (b) | Tests helper that pops and returns. Keep |
| `helper_call_in_tuple_element` | (b) | Tests helper call in tuple context. Keep |
| `helper_call_in_subscript_base` | (b) | Tests helper call in subscript context. Keep |
| `multiple_helper_calls_in_tuple` | (b) | Tests multiple helpers in tuple. Keep |

### `src/dataflow.rs` (5 tests)

Test `extract_arg_names` (derives argument names from constraints).

| Test | Category | Notes |
|------|----------|-------|
| `arg_names_from_tuple_unpack` | (d) | Pure Rust logic on constraint data |
| `arg_names_from_indexed_access` | (d) | Pure Rust logic |
| `arg_names_with_required_keyword` | (d) | Pure Rust logic |
| `arg_names_fallback_generic` | (d) | Pure Rust logic |
| `arg_names_empty_when_no_constraints` | (d) | Pure Rust logic |

### `src/environment/scan.rs` (16 tests)

Test environment scanning (filesystem + AST).

| Test | Category | Notes |
|------|----------|-------|
| `scan_discovers_libraries` | (b) | Uses `tempdir` with fabricated filesystem. Pattern is real but tests filesystem logic. Keep |
| `scan_derives_correct_app_module` | (b) | Tests module derivation logic. Keep |
| `scan_name_collision_detection` | (b) | Tests collision detection. Keep |
| `scan_skips_init_files` | (b) | Tests `__init__.py` skipping. Keep |
| `scan_requires_templatetags_init` | (b) | Tests directory validity check. Keep |
| `scan_empty_directory` | (b) | Edge case. Keep |
| `scan_nonexistent_path` | (b) | Edge case. Keep |
| `scan_multiple_sys_paths` | (b) | Tests multi-path scanning. Keep |
| `libraries_for_unknown_name_returns_empty` | (d) | Pure logic on scan results |
| `scan_skips_non_py_files` | (b) | Tests file filtering. Keep |
| `scan_with_symbols_extracts_registrations` | (b) | Uses fabricated Python source in temp files. Pattern is real but hard to isolate from corpus |
| `scan_with_symbols_parse_failure_still_discovers_library` | (b) | Tests graceful parse failure. Keep |
| `scan_with_symbols_reverse_lookup_tags` | (b) | Tests reverse lookup. Keep |
| `scan_with_symbols_reverse_lookup_collision` | (b) | Tests collision in reverse lookup. Keep |
| `scan_without_symbols_has_empty_tags_filters` | (b) | Tests no-symbol mode. Keep |
| `scan_with_symbols_no_registrations` | (b) | Tests empty registration file. Keep |

Note: `scan.rs` tests are inherently filesystem-oriented. They create temp directories with fabricated file structures. Replacing with corpus would mean pointing at real corpus paths — but the tests need controlled directory structures (collisions, missing `__init__.py`, etc.) that corpus can't provide. Category (b) is appropriate for most.

### `src/lib.rs` (48 tests)

Golden end-to-end tests. These are the highest-value candidates for corpus replacement.

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

### Summary

| File | Total | (a) Replace | (b) Keep+comment | (c) Remove | (d) Pure Rust |
|------|-------|-------------|-------------------|------------|---------------|
| `types.rs` | 13 | 0 | 0 | 0 | 13 |
| `dataflow/domain.rs` | 4 | 0 | 0 | 0 | 4 |
| `registry.rs` | 20 | 12 | 8 | 0 | 0 |
| `blocks.rs` | 18 | 10 | 8 | 0 | 0 |
| `filters.rs` | 17 | 8 | 9 | 0 | 0 |
| `signature.rs` | 5 | 4 | 1 | 0 | 0 |
| `dataflow/constraints.rs` | 31 | 14 | 10 | 0 | 7 |
| `dataflow/eval.rs` | 48 | 4 | 10 | 0 | 34 |
| `dataflow/calls.rs` | 14 | 1 | 5 | 0 | 8 |
| `dataflow.rs` | 5 | 0 | 0 | 0 | 5 |
| `environment/scan.rs` | 16 | 0 | 15 | 0 | 1 |
| `lib.rs` | 48 | 31 | 17 | 0 | 0 |
| **Total** | **239** | **84** | **83** | **0** | **72** |

**Key findings:**
- **84 tests** (35%) should be replaced with corpus-sourced equivalents
- **83 tests** (35%) should be kept with justification comments (real pattern but no clean isolatable example, edge cases, or filesystem-oriented)
- **0 tests** to remove — no purely fictional patterns found. The fabricated snippets model real Django patterns, just not using actual corpus source.
- **72 tests** (30%) are pure Rust logic, no Python involved
- `lib.rs` golden tests are highest priority — 31 of 48 should use corpus source
- `environment/scan.rs` tests are filesystem-oriented and should stay fabricated (they need controlled directory structures)
- `dataflow/eval.rs` is mostly pure Rust unit tests (34 of 48) — the abstract interpreter tests don't need corpus source

## Discoveries

_(Record anything learned during implementation that affects future milestones)_
