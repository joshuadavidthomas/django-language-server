# Codebase Review

## djls-extraction (NEW crate)

### src/lib.rs
Main entry point. `extract_rules()` orchestrates parsing, registration discovery, and rule extraction. Clean structure with proper feature gating (`#[cfg(feature = "parser")]`). `collect_func_defs` recursion into `ClassDef` bodies is correct for Django's pattern of defining compile functions inside classes. Test coverage is thorough with golden snapshot tests.

### src/types.rs
Core types: `SymbolKey`, `ExtractionResult`, `TagRule`, `FilterArity`, `BlockTagSpec`, etc. Well-designed with `#[must_use]` on constructors. `rekey_module` uses `debug_assert_eq` to catch data loss — good defensive programming. `ExtractionResult::merge` has clear last-wins semantics.

### src/environment/types.rs (was `environment_types.rs`)
`EnvironmentInventory`, `EnvironmentLibrary`, `EnvironmentSymbol` — always-available types (no feature gate). Uses `BTreeMap` for deterministic ordering. `tags_by_name()` and `filters_by_name()` build reverse lookup maps. Clean API design.

### src/registry.rs
Registration discovery from Python AST. Handles decorator-style (`@register.tag`), call-style (`register.tag("name", func)`), and all Django registration patterns (`simple_tag`, `inclusion_tag`, `filter`, `simple_block_tag`). Comprehensive test coverage including edge cases like `name=` kwarg override priority.

### src/signature.rs
Extracts rules from `simple_tag`/`inclusion_tag` function signatures via `parse_bits` semantics. Correctly handles `takes_context=True` parameter skipping, `*args`, `**kwargs`, keyword-only params.

### src/filters.rs
Filter arity extraction. Handles `self` skipping for method-style filters, positional-only params, multiple extra args with mixed defaults.

### src/blocks.rs
Block spec extraction from `parser.parse((...))` patterns. Complex but well-structured control flow classification with clear fallback strategy.

### src/dataflow.rs
Orchestrator for dataflow analysis. `analyze_compile_function_with_cache` sets up the abstract environment and delegates to `eval::process_statements`. `extract_arg_names` reconstructs positional argument names from env bindings — deterministic output via sort+dedup.

### src/dataflow/domain.rs
Abstract domain: `AbstractValue` variants (`Token`, `Parser`, `SplitResult`, `SplitElement`, `SplitLength`, `Int`, `Str`, `Tuple`, `Unknown`). `Env` wraps `HashMap<String, AbstractValue>` with `get`/`set`/`mutate`/`iter`. `SplitResult` tracks `base_offset` and `pops_from_end` for mutation tracking.

### src/dataflow/constraints.rs
Constraint extraction from `if condition: raise TemplateSyntaxError(...)` patterns. Handles `or`, `and`, negation, chained comparisons, range constraints. `ChoiceAt` constraint for `not in` patterns. `body_raises_template_syntax_error` for guard detection.

### src/dataflow/eval.rs
Expression evaluation and statement processing. Tracks `split_contents()` through assignments, subscripts, slices, `pop()`, `list()`, `len()`. Option loop detection (`while remaining: option = remaining.pop(0)`). Match statement support for Django 6.0+ patterns. Well-tested with 40+ unit tests.

### src/dataflow/calls.rs
Bounded inlining for helper function calls. `HelperCache` avoids re-analyzing helpers. `MAX_CALL_DEPTH = 2` prevents infinite recursion. Self-recursion guard. Cache key uses `AbstractValueKey` (hashable projection of `AbstractValue`).

### src/environment.rs (parent module)
Re-exports from `environment/types.rs` (always available) and `environment/scan.rs` (feature-gated). Follows the `folder.rs` + `folder/*.rs` submodule convention.

### src/environment/scan.rs (was `environment.rs`)
Environment scanning: `scan_environment` and `scan_environment_with_symbols` walk `sys.path` for `*/templatetags/*.py`. Package tree recursion skips `.dist-info`, `.egg-info`, `__pycache__`. Symbol extraction via Ruff parser.

### tests/corpus.rs, tests/golden.rs
Integration tests against real Django source files and golden snapshot fixtures.

## djls-semantic

### src/errors.rs
`ValidationError` enum — exhaustive, all variants have `span` fields for LSP diagnostic positioning. 20 variants covering structural, scoping, expression, filter arity, extends, and extracted rule violations. New variants: `FilterMissingArgument`, `FilterUnexpectedArgument`, `ExtractedRuleViolation`, `TagNotInInstalledApps`, `FilterNotInInstalledApps`, `UnknownLibrary`, `LibraryNotInInstalledApps`, `ExtendsMustBeFirst`, `MultipleExtends`.

### src/db.rs
`Db` trait — adds `filter_arity_specs()` and `environment_inventory()` methods. `ValidationErrorAccumulator` Salsa accumulator.

### src/lib.rs
`validate_nodelist` — orchestrates all validation passes: block tree, semantic forest, opaque regions, tag arguments, tag scoping, filter scoping, load libraries, if expressions, filter arity, extends. Clean single-pass composition.

### src/extends.rs (NEW)
`validate_extends` — checks `{% extends %}` must be first non-text tag (S122) and cannot appear multiple times (S123). Simple linear scan.

### src/filters.rs (parent module, NEW)
Re-exports from `filters/arity.rs` and `filters/validation.rs`. Follows the `folder.rs` + `folder/*.rs` submodule convention.

### src/filters/arity.rs (was `filter_arity.rs`, NEW)
`FilterAritySpecs` — maps filter name → `(SymbolKey, FilterArity)`. Last-wins merge semantics matching Django's builtin ordering.

### src/filters/validation.rs (was `filter_validation.rs`, NEW)
`validate_filter_arity` — S115 (missing required arg) and S116 (unexpected arg) diagnostics. Guards: suppressed when no inspector inventory, skips opaque regions, skips unknown filters.

### src/if_expression.rs (NEW)
`validate_if_expressions` — Pratt parser port of Django's `smartif.py` for compile-time expression syntax validation. S114 diagnostics. Handles all Django operators including `in`, `not in`, `is`, `is not`.

### src/rule_evaluation.rs (NEW)
Evaluates `TagRule` constraints against template tag arguments with proper `split_contents` index adjustment.

### src/opaque.rs (NEW)
`OpaqueRegions` — sorted spans for opaque blocks (`{% verbatim %}`, `{% comment %}`). Binary search for O(log n) containment checks. `compute_opaque_regions` walks block tree for `opaque: true` tag specs.

### src/loads.rs (was `load_resolution.rs`, NEW)
`compute_loaded_libraries` — Salsa tracked function that parses `{% load %}` tags into `LoadedLibraries` for position-aware availability queries.

### src/loads/load.rs (was `load_resolution/load.rs`, NEW)
`LoadStatement`, `LoadKind`, `LoadedLibraries`, `AvailabilityState`. Parses `{% load lib %}` and `{% load sym from lib %}` patterns.

### src/loads/symbols.rs (was `load_resolution/symbols.rs`, NEW)
`AvailableSymbols` — three-layer resolution: builtins → loaded libraries → check tag/filter availability at a position.

### src/loads/validation.rs (was `load_resolution/validation.rs`, NEW)
Tag scoping (S108/S109/S110), filter scoping (S111/S112/S113), load library validation (S120/S121), and three-layer environment resolution (S118/S119).

### src/arguments.rs
`validate_all_tag_arguments` — validates each tag's arguments against extracted rules via `rule_evaluation::evaluate_tag_rules`. Skips opaque regions and tags without extracted rules.

### src/templatetags/specs.rs
`TagSpecs` and `TagSpec` — `merge_extraction_results` integrates extraction data (block specs, tag rules) into tag specifications. `test_tag_specs()` provides minimal Django tag structure for tests.

### src/templatetags/builtins.rs (DELETED)
Hardcoded builtins removed — all tag knowledge now comes from extraction.

### src/blocks/builder.rs, src/blocks/grammar.rs, src/blocks/tree.rs
Block tree construction updates to support extracted tag specs.

### src/semantic/forest.rs
Semantic forest minor updates.

## djls-ide

### src/completions.rs
**FIX APPLIED (pass 3)**: Fixed potential UTF-8 panic in `calculate_replacement_range`. The expression `&line_text[cursor_offset..=cursor_offset]` creates a 1-byte string slice that would panic if `cursor_offset` points to the start of a multi-byte UTF-8 character (because `..=` expands to `cursor_offset..cursor_offset+1`, which would split the character). Changed to byte-level comparison `line_text.as_bytes().get(cursor_offset) == Some(&b'}')` which is both safe and more idiomatic for single-byte ASCII checks.

**FIX APPLIED (pass 3)**: Removed unused parameters `_parsed_args: &[String]` and `_template_tags: Option<&TemplateTags>` from `generate_argument_completions`. These were dressed up with underscore prefixes to silence warnings but were genuinely unused. Updated the caller in `generate_template_completions` to use `..` pattern destructuring instead.

**FIX APPLIED (pass 2)**: Fixed char-vs-byte indexing bug in `get_line_info()`. The UTF-16 branch computed a **character count** but `cursor_offset` was used downstream for byte-based string slicing (`line[..cursor_offset]`). For multi-byte UTF-8 characters, this would index into the middle of a character and either produce wrong results or panic. Changed to compute **byte offset** directly.

**FIX APPLIED (pass 1)**: Removed dead `Variable` and `None` variants from `TemplateCompletionContext` enum and removed the `#[allow(dead_code)]` that was hiding them. `Filter` variant is actively used.

Major additions: `TagArgument` context, `LibraryName` context, argument-aware completions using extracted args, filter completions from available symbols.

### src/diagnostics.rs
S-code mapping for all new `ValidationError` variants (S115-S123). Clean exhaustive match.

### src/snippets.rs
`generate_partial_snippet` for argument-aware snippet generation from `ExtractedArg`.

### src/context.rs
Minor update.

## djls-project

### src/resolve.rs (NEW)
Module path → file path resolution. `resolve_module` searches `sys_path` entries, classifies as Workspace or External. `build_search_paths` constructs search path list from interpreter, root, pythonpath.

### src/django.rs
Major additions: `extracted_external_rules` tracked field on `Project`, `TemplateTags` API extensions.

### src/project.rs
Project bootstrap updates for new fields.

### src/lib.rs
Re-exports for `resolve_modules`, `build_search_paths`.

## djls-server

### src/db.rs
`DjangoDatabase` — `compute_tag_specs`, `compute_tag_index`, `compute_filter_arity_specs` tracked functions. `extract_module_rules` tracked per-file extraction. `collect_workspace_extraction_results` partitions workspace vs external modules. `refresh_inspector` updates inventory, extracts external rules, scans environment.

### src/server.rs
Server initialization updates.

### tests/corpus_templates.rs (NEW)
Corpus-based integration tests against real Django template files.

## djls-corpus (NEW crate)

### src/lib.rs, src/main.rs, src/manifest.rs, src/sync.rs, src/enumerate.rs
Corpus syncing infrastructure for downloading and caching Django source files for integration tests.

## djls-conf

### src/lib.rs
Removed `tagspecs` module and all `TagSpecDef`/`TagLibraryDef` types — legacy configuration format removed.

### src/diagnostics.rs
Minor update.

## djls-bench

### src/db.rs
Added `filter_arity_specs()` and `environment_inventory()` to `SemanticDb` impl.

## djls-templates

### src/parser.rs, src/nodelist.rs, src/lib.rs
Parser updates to support filter span tracking and variable node filter information.

## Review History

### Pass 5 (current)
- **Refactored**: `djls-extraction` — `environment.rs` + `environment_types.rs` (flat siblings) → `environment.rs` parent module + `environment/types.rs` + `environment/scan.rs` submodules. Follows the `folder.rs` + `folder/*.rs` convention used by `dataflow.rs` + `dataflow/*.rs`. Feature gating moved inside the parent module.
- **Refactored**: `djls-semantic` — `filter_arity.rs` + `filter_validation.rs` (flat siblings with shared `filter_` prefix) → `filters.rs` parent module + `filters/arity.rs` + `filters/validation.rs`. Follows the `blocks.rs` + `blocks/*.rs` convention. Shorter names.
- **Refactored**: `djls-semantic` — `load_resolution.rs` + `load_resolution/*.rs` → `loads.rs` + `loads/*.rs`. Renamed from verbose `load_resolution` to short `loads`, matching the crate's naming pattern (`blocks`, `extends`, `semantic`, `templatetags`, `loads`). Eliminated confusion with the existing `resolution.rs` (template file resolution).
- **Fixed**: Removed all decorative section dividers (`──`, `═══`, `====`) from 7 files in `djls-extraction` and `djls-semantic`, replacing with plain `// Section Name` comments per project convention.
- **Updated**: `AGENTS.md` references to renamed modules.
- All changes pass `cargo clippy -q --all-targets --all-features -- -D warnings` and `cargo test -q`.

### Pass 4
- **Fixed**: Pre-existing UTF-32 encoding panic in `get_line_info()` — the `_ =>` catch-all treated UTF-32 `position.character` (codepoint count) as a byte offset, which would panic on `&line[..offset]` for non-ASCII text. Added explicit `PositionEncoding::Utf32` branch that converts codepoint positions to byte offsets. Also changed the `_ =>` to exhaustive matching (`PositionEncoding::Utf8 =>`) to prevent future encoding variants from silently falling through.

### Pass 3
- **Fixed**: UTF-8 safety issue in `calculate_replacement_range` — `&line_text[cursor_offset..=cursor_offset]` could panic on multi-byte characters; switched to byte-level comparison
- **Fixed**: Removed unused parameters `_parsed_args` and `_template_tags` from `generate_argument_completions` per project convention against dressing up dead code with underscore prefixes
