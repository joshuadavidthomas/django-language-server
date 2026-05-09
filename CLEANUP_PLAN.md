# Python Crate Cleanup Plan

Scope: `crates/djls-python`

Goal: reduce over-organization and single-use abstractions, especially wrappers that only serve one caller.

## Main smell

The repeated pattern was:

> one caller → one wrapper → one visitor/type → one tiny operation

A helper or module should earn its keep by hiding real complexity, preserving an invariant, or reducing repeated logic. Many helpers only added a file/function jump.

## Original findings archive

The original audit found these over-abstraction candidates before cleanup. Items already completed are also summarized in the completed section below.

### `blocks` extraction

`blocks.rs::extract_block_spec` is the only caller of these detector entrypoints:

- `blocks/opaque.rs::detect`
- `blocks/parse_calls.rs::detect`
- `blocks/dynamic_end.rs::detect`
- `blocks/next_token.rs::detect`

The strategy split was not absurd because `parse_calls.rs` and `next_token.rs` are large, but the shape was over-organized: one strategy per file, each with a single `detect` facade, all owned by one parent function.

#### `blocks/opaque.rs`

- `collect_skip_past_tokens` was a one-call wrapper around `SkipPastVisitor`.
- Inline it into `detect`.

#### `blocks/dynamic_end.rs`

- `has_dynamic_end_in_body` was a one-call wrapper around `DynamicEndFinder`.
- `has_dynamic_end_tag_format` is only called from `next_token::detect`.
- `DynamicEndFinder` and `DynamicEndFormatFinder` have nearly identical visitor skeletons.
- `is_end_fstring` was `pub(super)` but only used inside `dynamic_end.rs`; make it private.

#### `blocks/next_token.rs`

- `has_next_token_loop` wrapped `NextTokenLoopFinder`, called once.
- `body_has_next_token_call` wrapped `NextTokenCallFinder`, called once.
- `collect_token_content_comparisons` wrapped `TokenComparisonVisitor`, called once.
- These were helper-hides-helper-hides-visitor layers.

#### `blocks/parse_calls.rs`

- `collect_parser_parse_calls` wrapped `ParseCallCollector`, called once.
- `ParseCallInfo` was a single-field struct around `Vec<String>`; use `Vec<Vec<String>>` or a local alias instead.
- `ParseCallCollector` and `ParseCallFinder` duplicate traversal shape.

#### `blocks.rs`

- `extract_string_sequence` repeated tuple/list/set arms; collapse to “get elements, then map once.”

### Registry, signature, filters

#### Collapse extraction dispatch

- `registry.rs::ExtractionOutput` and `RegistrationKind::extract` were a trampoline:
  - caller already has `reg.kind`
  - caller calls `reg.kind.extract(func)`
  - caller matches the returned enum
- Match `reg.kind` directly in `lib.rs::extract_rules_from_body` and delete the enum/method.

#### Merge single-caller modules

- `filters.rs::extract_filter_arity`
  - One production caller: `registry.rs`.
  - The whole module exists for one function plus tests.
  - Move into `registry.rs` unless test isolation is worth the file jump.

- `signature.rs::extract_parse_bits_rule`
  - One production caller: `registry.rs`.
  - Move into `registry.rs` if keeping all registration extraction together is preferable.

#### Smaller cleanup

- `registry.rs::FILTER_DECORATORS`
  - One-element array: `&["filter"]`.
  - Replace `.contains()` with `attr.as_str() == "filter"`.

- `registry.rs::kw_name_from`
  - Thin wrapper around `kw_constant_str(keywords, "name")`.
  - Collapse into one helper.

- `registry.rs::first_string_arg`
  - One-line helper: `args.first().and_then(ExprExt::string_literal)`.
  - Used a few times, but may cost more than it saves.

- `signature.rs::has_takes_context`
  - One caller.
  - Inline into `extract_parse_bits_rule` if `signature.rs` stays.

- `ext.rs::ExprExt::is_true_literal`
  - One call site.
  - Inline the `matches!` in `signature.rs`.

### Analysis module

#### Strong candidates

- `analysis.rs::CompileFunction`
  - Small struct used only by `analyze_compile_function`.
  - It only stores `parser_param`, `token_param`, and `body`.
  - Inline the parameter extraction and delete the type.

- `analysis.rs::infer_max_position`
  - One caller.
  - Inline into `extract_arg_names`.

- `analysis/mutations.rs::OptionPopFinder`
  - Fake visitor.
  - It explicitly does not recurse.
  - Replace with `body.iter().find_map(...)`.

- `analysis/mutations.rs::OptionCheckVisitor`
  - Fake visitor.
  - Only handles one `Stmt::If`.
  - `extract_option_checks` clones the whole `if_stmt` just to call the visitor.
  - Delete the visitor and inspect the `if` / `elif_else_clauses` directly.

- `analysis/match_arms.rs::PatternShape` and `analyze_case_pattern`
  - Enum is only consumed immediately by one match in `extract_match_constraints`.
  - Inline the pattern matching.

- `analysis/match_arms.rs::pattern_literal`
  - One call site.
  - Inline as a closure or local match.

- Duplicate `RaiseFinder`
  - One in `analysis/match_arms.rs`.
  - One in `analysis/rules.rs`.
  - Same idea, slightly different recursion behavior.
  - Share a tiny helper or use direct iteration where non-recursive.

- `analysis/rules.rs::eval_range_constraint`
  - One caller: `eval_negated_compare`.
  - Conceptually one branch of negated-compare handling.
  - Inline.

- `analysis/statements.rs::try_extract_token_kwargs_call`
  - Duplicates logic already in `analysis/expressions.rs::eval_call_with_ctx`.
  - Since assignment processing already evaluates the RHS, the side effect can probably live in one place.

#### Borderline / probably keep

- `analysis/calls.rs::extract_return_value`
  - One production caller, but it is a real semantic operation with recursive return walking.
  - Do not inline first.

- `analysis/expressions.rs::eval_expr`
  - Thin wrapper around `eval_expr_with_ctx(..., None)`.
  - Used enough as a no-context entrypoint that it is okay.

### Models / graph extraction

#### Strong candidates

- `models/extract.rs::is_django_model_parent`
  - One-line helper.
  - Inline.

- `models/extract.rs::is_django_models_module`
  - One-line helper.
  - Inline unless reuse grows.

- `models/extract.rs::ImportAliases`
  - Small struct with two sets, used only by `ModelCollector`.
  - Make the sets fields on `ModelCollector`.

- `models/graph.rs::RelationType::from_field_class`
  - One production caller.
  - The match would be clearer inside `extract_relation`, where all relation data is already present.

- `models/graph.rs::FieldName`
  - Newtype with no validation and little use outside relation construction.
  - `ModelName` and `ModulePath` earn their distinction more; `FieldName` is questionable.

- `models/graph.rs::ModelKind`
  - Currently `Concrete | Abstract`.
  - Comment says enum is for future proxy support.
  - This is future-proofing. Could be `is_abstract: bool` until proxy exists.
  - This touches public surface, so do later and deliberately.

- `models.rs`
  - Pure re-export shim around `models/extract.rs` and `models/graph.rs`.
  - Not harmful, but organization-only.

#### Small / unused

- `types.rs::SplitPosition::is_tag_name`
  - No call sites. Delete.

- `types.rs::SplitPosition::raw`
  - No call sites. Delete.

## Completed cleanup

### Extraction dispatch and registry helpers

- Removed `registry.rs::ExtractionOutput` and `RegistrationKind::extract`.
- Matched `RegistrationKind` directly in `lib.rs::extract_rules_from_body`.
- Removed small single-use helpers:
  - `FILTER_DECORATORS`
  - `kw_constant_str`
  - `first_string_arg`
- Inlined `ExprExt::is_true_literal` and removed that trait method.
- Inlined `signature.rs::has_takes_context`.
- Merged `filters.rs::extract_filter_arity` into `registry.rs` and deleted `filters.rs`.
- Merged `signature.rs::extract_parse_bits_rule` into `registry.rs` and deleted `signature.rs`.

### Analysis helpers

- Removed `analysis.rs::CompileFunction`.
- Inlined `analysis.rs::infer_max_position`.
- Removed fake visitors in `analysis/mutations.rs`:
  - `OptionPopFinder`
  - `OptionCheckVisitor`
- Removed `analysis/match_arms.rs::PatternShape` and `analyze_case_pattern`.
- Removed `analysis/match_arms.rs::pattern_literal`.
- Removed the duplicate direct-raise visitor in `analysis/rules.rs`.
- Inlined `analysis/rules.rs::eval_range_constraint` into `eval_negated_compare`.
- Centralized `token_kwargs` side-effect handling in `analysis/expressions.rs` and reused it from `analysis/statements.rs`.

### Block extraction helpers

- Inlined the smallest single-use wrappers in `blocks/*`:
  - `opaque::collect_skip_past_tokens`
  - `dynamic_end::has_dynamic_end_in_body`
  - `next_token::has_next_token_loop`
  - `next_token::body_has_next_token_call`
  - `next_token::collect_token_content_comparisons`
  - `parse_calls::collect_parser_parse_calls`
- Made `dynamic_end::is_end_fstring` private.
- Collapsed `blocks.rs::extract_string_sequence` to one element collection path.
- Removed `parse_calls::ParseCallInfo`, a single-field wrapper around `Vec<String>`.
- Merged `blocks/opaque.rs` into `blocks.rs` and deleted the one-strategy module.
- Merged `blocks/dynamic_end.rs` into `blocks.rs` and deleted the one-strategy module.
- Renamed the remaining generic detector entrypoints to strategy-specific extraction helpers:
  - `blocks/parse_calls.rs::extract_parse_call_block_spec`
  - `blocks/next_token.rs::extract_next_token_block_spec`

### Models / graph extraction

- Inlined model import alias bookkeeping into `ModelCollector`.
- Removed `models/extract.rs::is_django_model_parent`.
- Removed `models/extract.rs::is_django_models_module`.
- Inlined `ModelCollector::finish`.
- Inlined `RelationType::from_field_class` into `extract_relation` and removed the method.
- Removed `ModelGraph::new`; callers now use `ModelGraph::default()`.
- Removed the `FieldName` newtype; relation field names now use plain `String`.

### Types

- Removed unused `SplitPosition::is_tag_name`.
- Removed unused `SplitPosition::raw`.
- Kept `Display for SplitPosition`; broader workspace tests showed `djls-semantic` uses it.

## Remaining candidates

### Re-evaluate block module split

`blocks.rs::extract_block_spec` is still the only caller of the parser-parse and parser-next-token block extraction helpers. The generic `detect` facades were renamed to strategy-specific entrypoints so callers show which extraction path is being tried:

- `blocks/parse_calls.rs::extract_parse_call_block_spec`
- `blocks/next_token.rs::extract_next_token_block_spec`

The split is now less noisy after wrapper removal. `parse_calls.rs` and `next_token.rs` still have enough meat to justify separate files.

### Model API shape

- `models/graph.rs::ModelKind`
  - Currently `Concrete | Abstract`.
  - Comment says enum is for future proxy support.
  - Could become `is_abstract: bool`, but this touches public serialized/API shape and should be deliberate.

- `models.rs`
  - Pure re-export shim around `models/extract.rs` and `models/graph.rs`.
  - Not harmful, but organization-only.

### Probably keep

- `analysis/calls.rs::extract_return_value`
  - One production caller, but it is a real semantic operation with recursive return walking.

- `analysis/expressions.rs::eval_expr`
  - Thin wrapper around `eval_expr_with_ctx(..., None)`, but useful as a no-context entrypoint.

- `blocks/parse_calls.rs::ParseCallFinder`
  - Still a visitor type, but it has multiple call sites through `body_has_parse_call` and avoids collecting when only existence is needed.

## Current validation used

- `just fmt`
- `cargo test -p djls-python`
- `cargo test -p djls-db`
- `cargo test -p djls-bench`
