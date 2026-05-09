# Python Crate Cleanup Plan

Scope: `crates/djls-python`

Goal: reduce over-organization and single-use abstractions, especially wrappers that only serve one caller.

## Main smell

The repeated pattern is:

> one caller → one wrapper → one visitor/type → one tiny operation

A helper or module should earn its keep by hiding real complexity, preserving an invariant, or reducing repeated logic. Many current helpers only add a file/function jump.

## Progress

Done:

- Deleted unused `SplitPosition` API.
- Removed fake visitors in `analysis/mutations.rs`.
- Collapsed `ExtractionOutput` and `RegistrationKind::extract`.
- Inlined the smallest single-use wrappers in `blocks/*`.
- Removed small single-use helpers: `CompileFunction`, `infer_max_position`, `has_takes_context`, `ExprExt::is_true_literal`, `FILTER_DECORATORS`, and `kw_constant_str`.
- Removed `PatternShape`, `pattern_literal`, duplicate direct-raise visitor, and `eval_range_constraint`.
- Removed `ParseCallInfo` single-field wrapper.
- Inlined model import alias bookkeeping and `RelationType::from_field_class`.

Still open:

- Merge `filters.rs`, and maybe `signature.rs`, into `registry.rs`.
- Re-evaluate whether `blocks/{opaque,parse_calls,dynamic_end,next_token}.rs` should stay split.
- Continue the remaining model cleanup candidates.

## Recommended order

1. Merge `filters.rs`, and maybe `signature.rs`, into `registry.rs`.
2. Re-evaluate whether `blocks/{opaque,parse_calls,dynamic_end,next_token}.rs` should stay split.
3. Continue analysis cleanup: `token_kwargs` duplication.
4. Continue model cleanup: questionable tiny types and public convenience methods.

## `blocks` extraction

`blocks.rs::extract_block_spec` is the only caller of these detector entrypoints:

- `blocks/opaque.rs::detect`
- `blocks/parse_calls.rs::detect`
- `blocks/dynamic_end.rs::detect`
- `blocks/next_token.rs::detect`

The strategy split is not absurd because `parse_calls.rs` and `next_token.rs` are large, but the current shape is over-organized: one strategy per file, each with a single `detect` facade, all owned by one parent function.

### `blocks/opaque.rs`

- `collect_skip_past_tokens` is a one-call wrapper around `SkipPastVisitor`.
- Inline it into `detect`.

### `blocks/dynamic_end.rs`

- `has_dynamic_end_in_body` is a one-call wrapper around `DynamicEndFinder`.
- `has_dynamic_end_tag_format` is only called from `next_token::detect`.
- `DynamicEndFinder` and `DynamicEndFormatFinder` have nearly identical visitor skeletons.
- `is_end_fstring` is `pub(super)` but only used inside `dynamic_end.rs`; make it private.

### `blocks/next_token.rs`

- `has_next_token_loop` wraps `NextTokenLoopFinder`, called once.
- `body_has_next_token_call` wraps `NextTokenCallFinder`, called once.
- `collect_token_content_comparisons` wraps `TokenComparisonVisitor`, called once.
- These are helper-hides-helper-hides-visitor layers.

### `blocks/parse_calls.rs`

- `collect_parser_parse_calls` wraps `ParseCallCollector`, called once.
- `ParseCallInfo` is a single-field struct around `Vec<String>`; use `Vec<Vec<String>>` or a local alias instead.
- `ParseCallCollector` and `ParseCallFinder` duplicate traversal shape.

### `blocks.rs`

- `extract_string_sequence` repeats tuple/list/set arms; collapse to “get elements, then map once.”

## Registry, signature, filters

### Collapse extraction dispatch

- `registry.rs::ExtractionOutput` and `RegistrationKind::extract` are a trampoline:
  - caller already has `reg.kind`
  - caller calls `reg.kind.extract(func)`
  - caller matches the returned enum
- Match `reg.kind` directly in `lib.rs::extract_rules_from_body` and delete the enum/method.

### Merge single-caller modules

- `filters.rs::extract_filter_arity`
  - One production caller: `registry.rs`.
  - The whole module exists for one function plus tests.
  - Move into `registry.rs` unless test isolation is worth the file jump.

- `signature.rs::extract_parse_bits_rule`
  - One production caller: `registry.rs`.
  - Move into `registry.rs` if keeping all registration extraction together is preferable.

### Smaller cleanup

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

## Analysis module

### Strong candidates

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

### Borderline / probably keep

- `analysis/calls.rs::extract_return_value`
  - One production caller, but it is a real semantic operation with recursive return walking.
  - Do not inline first.

- `analysis/expressions.rs::eval_expr`
  - Thin wrapper around `eval_expr_with_ctx(..., None)`.
  - Used enough as a no-context entrypoint that it is okay.

## Models / graph extraction

### Strong candidates

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

### Small / unused

- `types.rs::SplitPosition::is_tag_name`
  - No call sites. Delete.

- `types.rs::SplitPosition::raw`
  - No call sites. Delete.

- `impl Display for SplitPosition`
  - No call sites. Delete unless intended public API.

- `models/graph.rs::ModelGraph::new`
  - Just `Self::default()`.
  - Some callers already use `ModelGraph::default()`.
  - Pick one.

- `models/extract.rs::ModelCollector::finish`
  - One caller.
  - Slightly defensible because it consumes the collector, but can inline.

## Suggested first cleanup branch

Keep the first diff boring:

1. Delete unused `SplitPosition` methods/display.
2. Remove `OptionPopFinder` and `OptionCheckVisitor`.
3. Run `cargo test -p djls-python`.

Then continue with dispatch and block cleanup in separate commits.
