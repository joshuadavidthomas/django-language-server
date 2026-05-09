# Python Crate Cleanup Plan

Scope: `crates/djls-python`

Goal: reduce over-organization and single-use abstractions, especially wrappers that only serve one caller.

## Main smell

The repeated pattern was:

> one caller → one wrapper → one visitor/type → one tiny operation

A helper or module should earn its keep by hiding real complexity, preserving an invariant, or reducing repeated logic. Many helpers only added a file/function jump.

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

### Models / graph extraction

- Inlined model import alias bookkeeping into `ModelCollector`.
- Removed `models/extract.rs::is_django_model_parent`.
- Removed `models/extract.rs::is_django_models_module`.
- Inlined `ModelCollector::finish`.
- Inlined `RelationType::from_field_class` into `extract_relation` and removed the method.
- Removed `ModelGraph::new`; callers now use `ModelGraph::default()`.

### Types

- Removed unused `SplitPosition::is_tag_name`.
- Removed unused `SplitPosition::raw`.
- Kept `Display for SplitPosition`; broader workspace tests showed `djls-semantic` uses it.

## Remaining candidates

### Merge single-caller modules

- `filters.rs::extract_filter_arity`
  - One production caller: `registry.rs`.
  - The module mostly exists for one function plus tests.
  - Candidate: move the function and tests into `registry.rs`.

- `signature.rs::extract_parse_bits_rule`
  - One production caller: `registry.rs`.
  - Candidate: move into `registry.rs` if keeping all registration extraction together is preferable.

### Re-evaluate block module split

`blocks.rs::extract_block_spec` is still the only caller of these detector entrypoints:

- `blocks/opaque.rs::detect`
- `blocks/parse_calls.rs::detect`
- `blocks/dynamic_end.rs::detect`
- `blocks/next_token.rs::detect`

The split is now less noisy after wrapper removal. `parse_calls.rs` and `next_token.rs` still have enough meat to justify separate files. `opaque.rs` and `dynamic_end.rs` are more borderline.

### Model API shape

- `models/graph.rs::FieldName`
  - Newtype with no validation and little use outside relation construction.
  - `ModelName` and `ModulePath` earn their distinction more; `FieldName` is questionable.

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
