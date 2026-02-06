# Agent Guidelines

## Build/Test Commands
```bash
cargo build -q                      # Build all crates
cargo clippy -q --all-targets --all-features --fix -- -D warnings  # Lint with fixes
cargo +nightly fmt              # Format code (requires nightly)
cargo test -q                      # Run all tests  
cargo test test_name            # Run single test by name
cargo test -p crate_name        # Test specific crate
just test                       # Run tests via nox (with Django matrix)
just lint                       # Run pre-commit hooks
# NEVER use `cargo doc --open` - it requires browser interaction
```

## Code Style
- **IMPORTANT LSP**: Use `tower-lsp-server` NOT `tower-lsp`. Imports are `tower_lsp_server::*` NOT `tower_lsp::*`
- **LSP Types**: Use `tower_lsp_server::lsp_types` - we don't add `lsp-types` directly, it comes transitively from tower-lsp-server
- **Imports**: One per line, grouped (std/external/crate), vertical layout per `.rustfmt.toml`
- **Errors**: Use `anyhow::Result` for binaries, `thiserror` for libraries
- **Naming**: snake_case functions/variables, CamelCase types, SCREAMING_SNAKE constants
- **Comments**: Avoid unless essential; use doc comments `///` for public APIs only
- **Testing**: Use `insta` for snapshot tests in template parser. NEVER write standalone test files - always add test cases to the existing test modules in the codebase
- **Python**: Inspector runs via zipapp, test against Django 4.2/5.1/5.2/main

## Project Structure
- `crates/djls/` - Main CLI binary and PyO3 interface
- `crates/djls-server/` - LSP server implementation  
- `crates/djls-templates/` - Django template parser
- `crates/djls-workspace/` - Workspace/document management
- `crates/djls-extraction/` - Python AST analysis via Ruff parser (feature-gated)
- **Module convention**: Uses `folder.rs` NOT `folder/mod.rs`. E.g. `templatetags.rs` + `templatetags/specs.rs`, NOT `templatetags/mod.rs`

## Key File Paths — Extraction Crate (`djls-extraction`)
- **Crate root**: `crates/djls-extraction/src/lib.rs` — public API, `extract_rules()` pipeline, feature-gated re-exports
- **Types**: `crates/djls-extraction/src/types.rs` — `SymbolKey`, `ExtractionResult`, `TagRule`, `FilterArity`, `BlockTagSpec`, `ArgumentCountConstraint`, `KnownOptions`, `RequiredKeyword`
- **Registration discovery**: `crates/djls-extraction/src/registry.rs` — `collect_registrations()`, `RegistrationInfo`, `RegistrationKind` (behind `parser` feature)
- **Rule extraction**: `crates/djls-extraction/src/rules.rs` — `extract_tag_rule()`, compile function analysis, `extract_parse_bits_rule()` for simple/inclusion tags (behind `parser` feature)
- **Block spec extraction**: `crates/djls-extraction/src/blocks.rs` — `extract_block_spec()`, control-flow based intermediate/end-tag classification (behind `parser` feature)
- **Filter arity extraction**: `crates/djls-extraction/src/filters.rs` — `extract_filter_arity()`, function signature analysis (behind `parser` feature)
- **Context detection**: `crates/djls-extraction/src/context.rs` — `detect_split_var()` for finding `token.split_contents()` bindings (behind `parser` feature)
- **Ruff AST node reference**: `target/cargo_home/git/checkouts/ruff-*/*/crates/ruff_python_ast/src/nodes.rs` — struct definitions for `StmtFunctionDef`, `Parameters`, `ParameterWithDefault`, etc. Use `find` to locate since checkout hash varies.

## Key File Paths
- **Inspector Python**: `crates/djls-project/inspector/queries.py` — tag/filter collection, `build.rs` rebuilds pyz on change
- **Rust Django types**: `crates/djls-project/src/django.rs` — `TemplateTag`, `TemplateFilter`, `TemplateTags`, `TagProvenance` types and accessors
- **Project Salsa input**: `crates/djls-project/src/project.rs` — `Project` struct with all Salsa input fields
- **Database + queries**: `crates/djls-server/src/db.rs` — `DjangoDatabase`, update/refresh methods, tracked queries go here
- **Semantic Db trait**: `crates/djls-semantic/src/db.rs` — `Db` (Salsa jar trait) and `SemanticDb` (runtime accessor trait for tag_specs, tag_index, diagnostics_config, inspector_inventory)
- **Project lib.rs exports**: `crates/djls-project/src/lib.rs` — re-exports for `TagProvenance`, `TemplateFilter`, `TemplateTags`, inspector request/response types
- **Completions**: `crates/djls-ide/src/completions.rs` — `generate_library_completions()` at ~line 526, `TemplateCompletionContext` enum, `analyze_template_context()`
- **Completion context detection**: `crates/djls-ide/src/context.rs` — `OffsetContext` enum with `Variable { filters: Vec<String> }` variant
- **Node enum**: `crates/djls-templates/src/nodelist.rs` — `Node::Variable { var, filters: Vec<String>, span }`, `Node::Tag`, etc.
- **Parser**: `crates/djls-templates/src/parser.rs` — `parse_variable()` at ~line 182, `TestNode` helper in test module
- **NodeView (semantic)**: `crates/djls-semantic/src/blocks/tree.rs` — `NodeView::Variable` at ~line 332, mirrors `Node::Variable`
- **Semantic templatetags module**: `crates/djls-semantic/src/templatetags.rs` (NOT `templatetags/mod.rs`)
- **Semantic specs**: `crates/djls-semantic/src/templatetags/specs.rs` — `TagSpecs`, `TagIndex`, `django_builtin_specs()`
- **Semantic builtins**: `crates/djls-semantic/src/templatetags/builtins.rs` — builtin tag spec definitions
- **Block grammar**: `crates/djls-semantic/src/blocks/grammar.rs` — block structure parsing
- **Config types**: `crates/djls-conf/` — `TagSpecDef`, `DiagnosticsConfig`, `Settings`; `tagspecs.rs` for `TagSpecDef`
- **Load resolution root**: `crates/djls-semantic/src/load_resolution.rs` — re-exports `LoadedLibraries`, `AvailableSymbols`, `validate_tag_scoping`, `compute_loaded_libraries`
- **Load resolution submodules**: `crates/djls-semantic/src/load_resolution/load.rs` (parsing), `symbols.rs` (AvailableSymbols + TagAvailability), `validation.rs` (S108/S109/S110 diagnostics)
- **Opaque regions**: `crates/djls-semantic/src/opaque.rs` — `OpaqueRegions` type, `compute_opaque_regions()` — byte-span based check for `verbatim`/`comment` block interiors
- **If-expression parser**: `crates/djls-semantic/src/if_expression.rs` — Pratt parser for `{% if %}`/`{% elif %}` expression validation (S114)
- **Validation errors**: `crates/djls-semantic/src/errors.rs` — `ValidationError` enum with all diagnostic variants (S101–S114+)
- **Filter arity specs**: `crates/djls-semantic/src/filter_arity.rs` — `FilterAritySpecs` type, `compute_filter_arity_specs` tracked query, `validate_filter_arity` (S115/S116)
- **Filter snapshots**: `crates/djls-templates/src/snapshots/` — `parse_django_variable_with_filter.snap`, `parse_filter_chains.snap` — structured `Filter` objects with name/arg/span
- **Rule evaluator**: `crates/djls-semantic/src/rule_evaluation.rs` — `evaluate_tag_rules()`, S117 diagnostic, index offset logic between extraction and parser
- **Extraction types**: `crates/djls-extraction/src/types.rs` — `SymbolKey`, `ExtractionResult`, `TagRule`, `ExtractedArg`, `FilterArity`, `BlockTagSpec`, `ArgumentCountConstraint`
- **Snippets**: `crates/djls-ide/src/snippets.rs` — `generate_snippet_from_args()`, reads `TagArg` to produce LSP snippet strings
- **Diagnostics mapping**: `crates/djls-ide/src/diagnostics.rs` — `ValidationError` → S-code mapping, LSP `Diagnostic` construction, span → range conversion

## Documentation File Paths
- **MkDocs config**: `.mkdocs.yml` — nav structure, theme config, markdown extensions
- **Diagnostic codes docs**: `docs/configuration/index.md` — S101–S116 descriptions, severity config
- **TagSpecs docs**: `docs/configuration/tagspecs.md` — custom tag spec configuration
- **Template validation overview**: `docs/template-validation.md` — how validation works, inspector + extraction architecture
- **GitHub issue templates**: `.github/ISSUE_TEMPLATE/` — not yet created, needed for M7 Phase 3

## Django Engine Internals (for inspector work)
- `engine.builtins` — `list[str]` of module paths (e.g., `"django.template.defaulttags"`)
- `engine.template_builtins` — `list[Library]` of loaded Library objects, **parallel to `engine.builtins`**
- `engine.libraries` — `dict[str, str]` mapping load-name → module path (e.g., `{"static": "django.templatetags.static"}`)
- Use `zip(engine.builtins, engine.template_builtins)` to pair module paths with Library objects
- Use `engine.libraries.items()` (not `.values()`) to preserve load-name keys

## Worktree Gotchas
- **`target/` in `.gitignore`** — already added, but verify before `git add -A` that it's still excluded. Worktree `.gitignore` is separate from the main repo's.
- **Shared `target/` causes phantom compile errors**: `.cargo/config.toml` sets `target-dir = "target_local"` but `cargo metadata` may still report the shared `target/`. If you see mysterious E0599 ("no function/associated item named X") that works with `-p crate_name` but fails with `--workspace`, stale artifacts from the main repo's `target/` are the cause. Fix: `rm -rf ../../target/` or use `CARGO_TARGET_DIR=target_local cargo test`.

## Salsa Patterns
- **Setter API**: Salsa input setters use `.set_field(db).to(value)` — NOT `.set_field(db, value)`. The `.to()` call is required.
- **Manual comparison before setting**: Always compare old vs new with `project.field(db) != &new_value` before calling `project.set_field(db).to(new_value)` — setters always invalidate, even if the value is the same.
- **`#[returns(ref)]`**: Use on Salsa input fields that return owned types (String, Vec, HashMap, Option<T>) — Salsa returns `&T` from these fields.
- **Project is the single source of truth**: Store config docs (`TagSpecDef`, `DiagnosticsConfig`) on `Project`, not derived artifacts (`TagSpecs`). Conversion happens in tracked queries.
- **Tracked function return types need `PartialEq`**: Salsa uses equality to decide whether to propagate invalidation ("backdate" optimization). If a tracked function returns `TagSpecs`, `TagSpecs` must derive `PartialEq`.
- **Backdate optimization in tests**: If `compute_tag_specs` returns the same value after an input change, downstream queries like `compute_tag_index` will NOT re-execute. Test invalidation cascades with inputs that produce *distinct* outputs.
- **`#[returns(ref)]` and PartialEq**: When comparing a `#[returns(ref)]` field value, Salsa returns `&T`. Compare with `project.field(db) != &new_value` (borrow the new value). Both sides must be `&T` for `PartialEq` to work — forgetting the `&` on `new_value` gives E0369.
- **Parser `Node::Tag.bits` excludes tag name**: The parser splits `{% load i18n %}` into `name: "load"`, `bits: ["i18n"]`. The tag name is NOT in `bits`. Functions processing `bits` should work with arguments only.

## Clippy Rules
- Return `&str` not `&String` from accessors — clippy flags this
- All public accessor methods need `#[must_use]` — clippy enforces `must_use_candidate`
- Merge match arms with identical bodies (`match_same_arms` lint)
- Functions over 100 lines trigger `too_many_lines` — split or extract helpers
- Methods added to `impl DjangoDatabase` that aren't called yet trigger `dead_code` — add `#[allow(dead_code)]` temporarily or wire up call sites in the same commit
- Don't pass owned types by value when not consumed — use `&str` not `String`, `&[T]` not `Vec<T>` in function params (`needless_pass_by_value`)
- Prefer `HashMap::default()` over `HashMap::new()` — clippy flags `HashMap::new()` as less clear
- Don't use explicit lifetimes when they can be elided — `fn foo<'db>(&'db self)` → `fn foo(&self)` (`explicit_lifetimes_could_be_elided`)
- Unused imports after refactoring — `cargo clippy --fix` can auto-remove, or clean up manually before committing
- **Scoping exclusions**: Only skip closers/intermediates for load scoping checks — openers like `trans` have TagSpecs (for argument validation) BUT still need scoping because they're library tags. `django_builtin_specs()` includes ALL Django tags, not just builtins.
- **Diagnostic codes**: S108 = unknown tag (not in any library), S109 = unloaded tag (known library, not loaded), S110 = ambiguous unloaded tag (multiple candidate libraries). All three are guarded by `inspector_inventory.is_some()`.
- **Completions depend on load scoping**: `generate_tag_name_completions` needs `LoadedLibraries` + inspector inventory to filter results by position. When inspector unavailable, show all tags as fallback.
- **SemanticDb trait changes**: When adding methods to `SemanticDb`, update ALL test databases: `arguments.rs`, `blocks/tree.rs`, `semantic/forest.rs`, `load_resolution.rs`, `load_resolution/validation.rs`, `djls-bench/src/db.rs`, `djls-server/src/db.rs`
- **`crate::Db` vs `SemanticDb`**: In `djls-semantic`, test databases implement `crate::Db` (Salsa jar trait). `SemanticDb` (runtime trait) is only implemented on `DjangoDatabase` in `djls-server` and `Db` in `djls-bench`. Don't confuse the two.
- **`Serialize` for insta types**: Any type used in `Node` enum variants or test helpers that appear in insta snapshots must derive `serde::Serialize`. E0277 when missing.
- **Match `ValidationError` exhaustively**: When adding or removing variants from `ValidationError`, update: `errors.rs` (definition), `diagnostics.rs` (S-code mapping + span extraction), test helpers in `lib.rs` that filter errors, any other match sites found via `grep -rn "ValidationError" crates/ --include="*.rs"`.

## Common Agent Mistakes (from session history)
- **Wrong file paths**: `crates/djls-templates/src/ast.rs` does NOT exist — node types are in `nodelist.rs`. `templatetags/mod.rs` does NOT exist — uses `templatetags.rs` convention.
- **Reading directories**: Don't `read` a directory path — use `ls` or `find` instead. `EISDIR` errors waste turns.
- **Offset errors on `read`**: Don't guess line numbers for large files — use `grep -n` to find the right offset first.
- **Test failures from wrong assumptions about parser output**: Always parse a sample and inspect actual output before writing assertions. Especially: `Node::Tag.bits` excludes tag name, spans are byte offsets not line numbers, filter `Vec<Filter>` not `Vec<String>`.
- **Salsa setter `.to()` repeatedly forgotten**: E0599 "no method named `to`" means Salsa version mismatch or missing import. Current API: `project.set_field(db).to(value)`.
- **Adding enum variants without updating all match sites**: E0004 non-exhaustive patterns. When adding a variant to `ValidationError` (or any matched enum), grep for all `match` on that type and update them. E.g., `ValidationError::ExpressionSyntaxError` was added but not covered in a test helper's match.
- **Private module access**: E0603. Semantic sub-modules like `blocks/tree.rs` need `pub` re-export from parent (`blocks.rs`) to be accessible from other crates. Check visibility before cross-crate access.
- **Test inventories need relevant builtins**: When testing tag/filter scoping, add the tags you're testing to the mock builtin inventory — otherwise `validate_tag_scoping` emits spurious S108 (unknown tag) errors that mask the real test intent.
- **`cargo test` vs `cargo test -p`**: If `cargo test` (workspace) fails with E0599 but `cargo test -p crate_name` passes, it's a shared `target/` directory issue. Stale artifacts from main repo conflict with worktree code. See Worktree Gotchas.
- **`Filter` struct has public fields, not methods**: `djls_templates::Filter` uses `pub name`, `pub arg`, `pub span` — access as `filter.name`, NOT `filter.name()`. E0599 "no method named `name`" if you use accessor syntax.
- **New types in snapshot contexts need `Serialize`**: Types used in `Node` variants or test helpers that appear in insta snapshots must derive `serde::Serialize`. E0277 "Serialize is not satisfied" otherwise.
- **Mass deletion across crates**: When removing a type used across many files (e.g., `TagArg`), grep for the type name across the entire workspace first (`grep -rn "TypeName" crates/ --include="*.rs"`). Delete consumers before the definition — removing the definition first causes cascading compile errors that are harder to track.

## Extraction Architecture Patterns
- **Two-dispatch pattern for tag rules**: `extract_tag_rule()` dispatches based on `RegistrationKind` — `@register.tag` (compile function) goes to `extract_compile_function_rule()` which uses split_contents guard analysis; `@register.simple_tag` / `@register.inclusion_tag` goes to `extract_parse_bits_rule()` which uses function signature analysis (parameter count, defaults, `takes_context`).
- **`collect_registrations()` → `extract_tag_rule()`**: Two-phase: first collect all registrations (name + kind + function ref), then extract rules per registration. Don't try to do both in one pass.
- **Clone `func_name` early**: Ruff AST types own their strings. If you need `func.name` after moving or borrowing `func` elsewhere, clone it first. E0382 (use of moved value) is common otherwise.
- **`rules.rs` is large (1580+ lines)**: If adding new extraction helpers, consider whether they belong in a separate module (e.g., block spec extraction is in `crates/djls-extraction/src/blocks.rs`).
- **`extract_rules()` is fully wired**: Parses source → `collect_registrations_from_body` → finds func defs → dispatches to `extract_tag_rule` / `extract_block_spec` / `extract_filter_arity` per registration. Takes `module_path` param for `SymbolKey` keying.

## Rule Evaluation Patterns (M8)
- **Index offset between extraction and parser**: Extraction uses `split_contents()` indices (tag name at index 0), but parser `Node::Tag.bits` excludes tag name. Rule evaluator must add +1 to `bits.len()` when comparing against `ArgumentCountConstraint` values. `RequiredKeyword.position` is split_contents-indexed — subtract 1 to index into `bits`. Negative positions index from end of `bits` (no offset needed).
- **`merge_extraction_results` scope**: Currently only merges `block_specs` from `ExtractionResult` into `TagSpec`. Phase 3 needs to extend it for `tag_rules` (→ `TagSpec.extracted_rules`) and `extracted_args` (→ `TagSpec.args` for completions).
- **S117 (`ExtractedRuleViolation`)**: Carries `tag: String`, `message: String`, `span: Span`. Message is dynamically generated from the rule context (e.g., "tag 'for' requires at least 4 arguments"). Severity configurable via `djls-conf` prefix matching (S1, S11, S117).
- **Rule evaluator file**: `crates/djls-semantic/src/rule_evaluation.rs` — `evaluate_tag_rules()` function, 546 lines with comprehensive unit tests.
- **`ExtractedArg` type**: In `djls-extraction/src/types.rs` — `name`, `required`, `kind` (`Literal`/`Variable`/`Choice`/`VarArgs`), `default_value`, `position`. Used for completions/snippets; validation uses `TagRule` constraints directly.

## M9 Deletion Scope (active)
- **`TagArg` consumers**: `completions.rs` (match arms at ~line 617), `snippets.rs` (entire file depends on `TagArg`), `arguments.rs` (`validate_args_against_spec` + `validate_argument_order`). Delete consumers BEFORE the type.
- **`TagArg` re-export chain**: defined in `specs.rs` → re-exported from `templatetags.rs` → re-exported from `djls-semantic/src/lib.rs` → imported in `djls-ide/src/completions.rs` and `djls-ide/src/snippets.rs`.
- **S104–S107 variants**: defined in `errors.rs`, mapped to codes in `diagnostics.rs` (~line 88-91), filtered in test helpers in `lib.rs` (~line 912). Remove from all three.
- **`args` field cascade**: `TagSpec.args`, `EndTag.args`, `IntermediateTag.args` in `specs.rs` → used in `arguments.rs` validation, `completions.rs` argument completion, `snippets.rs` snippet generation, `merge_extraction_results()`, `builtins.rs` construction.
- **Replacement path for completions/snippets**: After removing `TagArg`, completions/snippets should read `spec.extracted_rules.as_ref().map(|r| &r.extracted_args)` directly using `ExtractedArg` type from `djls-extraction`.

## Validation Architecture Patterns
- **`validate_nodelist` is the orchestrator**: All validation passes are called from `crates/djls-semantic/src/lib.rs` `validate_nodelist()`. New validators wire in here.
- **Validation function signature pattern**: `validate_*(db: &dyn crate::Db, nodelist: &NodeList, opaque_regions: &OpaqueRegions, ...) -> Vec<ValidationError>`. Always accept `&OpaqueRegions` and skip nodes inside opaque spans.
- **Block tree root structure**: Root-level blocks have NO `BranchKind::Opener` — the container IS the root, containing only `BranchKind::Segment` children. `Opener` branches only appear for nested blocks (added to parent's segment). To find opaque blocks, check `Segment` branches whose tag has `opaque: true`.
- **Opaque region flow**: `compute_opaque_regions(db, nodelist)` → `OpaqueRegions` → passed to each validator. The opaque check is `opaque_regions.is_opaque(node_span_start)`.
- **Diagnostic codes**: S101–S107 (argument validation), S108–S110 (tag scoping), S111–S113 (filter scoping), S114 (expression syntax), S115–S116 (filter arity), S117 (extracted rule violation).
- **Argument validation dual path**: `TagSpec.extracted_rules` (from Python AST extraction) is primary. Falls back to `TagSpec.args`-based validation ONLY for user-config specs in `djls.toml` (escape hatch). Builtins with no extracted rules skip argument validation entirely.
- **`TagSpec.args` role after M8**: Used for completions/snippets (populated from `ExtractedArg` conversion). NOT used for validation when `extracted_rules` is present. User-config `args` in `djls.toml` still trigger the old `validate_argument_order` path.

## Cross-Cutting Type Changes
- **When adding a new parallel type** (e.g., `TemplateFilter` mirroring `TemplateTag`): update Python dataclass, `queries.py` collection, Rust struct, response type, `TemplateTags` struct + `new()` + `from_response()`, tracked query, `lib.rs` re-exports. Easy to miss one step in the chain.
- **`Node::Variable` filter changes cascade widely**: Changing `filters: Vec<String>` to a structured type requires updates in: `nodelist.rs` (Node enum), `parser.rs` (parse_variable + TestNode), `context.rs` (OffsetContext::Variable), `blocks/tree.rs` (NodeView::Variable), `completions.rs` (any filter handling), plus all insta snapshots. Run `cargo insta review` or `INSTA_UPDATE=1 cargo test` after.
- **`TemplateFilter` shares `TagProvenance`**: Filters use the same `TagProvenance` enum as tags (Library/Builtin variants). Don't create a separate provenance type for filters.

## Hot Files (heavily read/edited — know these well)
- **`crates/djls-server/src/db.rs`** — Salsa database, tracked queries, `SemanticDb` impl, update/refresh methods (42 edits, 46 reads). Read before modifying any Salsa query or database interaction.
- **`crates/djls-semantic/src/lib.rs`** — `validate_nodelist` orchestrator, wires all validation passes together (41 edits, 34 reads). Read before adding new validation functions.
- **`crates/djls-ide/src/completions.rs`** — integration point for tag, library, and filter completions (34 edits, 35 reads). Read before modifying any completion logic.
- **`crates/djls-extraction/src/lib.rs`** — public API, `extract_rules()` pipeline, feature-gated re-exports (33 edits, 33 reads). Read before adding extraction entry points or changing the pipeline.
- **`crates/djls-semantic/src/templatetags/specs.rs`** — `TagSpecs`, `TagIndex`, `django_builtin_specs()`, `merge_extraction_results()` (32 edits, 64 reads) — central to tag spec management. Most-read file.
- **`crates/djls-semantic/src/arguments.rs`** — `validate_tag_arguments()` dispatcher, `validate_args_against_spec()` user-config escape hatch (26 reads). Being simplified in M9.
- **`crates/djls-extraction/src/rules.rs`** — Rule extraction from compile functions and simple_tag signatures (21 edits, 1300+ lines). Read before adding extraction logic.
- **`crates/djls-project/src/django.rs`** — `TemplateTag`, `TemplateFilter`, `TemplateTags`, `TagProvenance` (16 edits, 22 reads) — read before any type changes.
- **`crates/djls-semantic/src/load_resolution/symbols.rs`** — `AvailableSymbols`, `TagAvailability`, `FilterAvailability` (19 edits) — complex position-aware logic.
- **`crates/djls-extraction/src/types.rs`** — core extraction types (`TagRule`, `ExtractedArg`, `BlockTagSpec`, etc.) (311 lines, 17 reads). Read before adding extraction output types.
- **`crates/djls-project/src/project.rs`** — `Project` Salsa input struct (13 reads) — read before adding new fields.
- **`crates/djls-semantic/src/rule_evaluation.rs`** — extracted rule evaluator, S117 diagnostic (546 lines). Read before modifying validation pipeline.
- **`crates/djls-ide/src/snippets.rs`** — snippet generation from `TagArg` (245 lines). Heavily depends on `TagArg` — M9 Phase 2 target.
- **`crates/djls-ide/src/diagnostics.rs`** — maps `ValidationError` variants to S-codes and LSP `Diagnostic`. Update when adding/removing error variants.

## Ruff Python AST Patterns (for `djls-extraction`)
- **Parsing**: `ruff_python_parser::parse_module(source)` returns `Result<Parsed<ModModule>, ParseError>`. Call `.into_syntax()` to get the `ModModule` with `.body: Vec<Stmt>`.
- **`StmtFunctionDef` is NOT `Deref`**: Access fields directly (`func.name`, `func.body`, `func.parameters`), do NOT try `*func`.
- **Parameters have per-param defaults**: `ParameterWithDefault { parameter, default: Option<Box<Expr>> }`. There is NO top-level `defaults` field on `Parameters` (unlike Python's `ast.arguments`). Count defaults with `.iter().filter(|p| p.default.is_some()).count()`.
- **`StmtWhile.test` is `Box<Expr>`**: Dereference with `&*while_stmt.test` when pattern matching.
- **String matching**: Use `ExprStringLiteral` → `.value.to_str()` for string content. Use `ExprName` → `.id.as_str()` for identifiers.
- **Feature gate `parser`**: Ruff parser deps only compile when `parser` feature is enabled. Types in `types.rs` are always available. Downstream crates needing only types use `djls-extraction = { workspace = true }` (no feature). Crates doing extraction use `djls-extraction = { workspace = true, features = ["parser"] }`.
- **`FStringValue` iteration**: Use `.iter()` not `.parts()` to iterate over `FStringPart` values in f-string expressions.
- **`ExceptHandler::ExceptHandler` is irrefutable**: Use `let` binding, not `if let`, when destructuring — there's only one variant.
- **Ruff git dep pinned to SHA**: `0dfa810e9aad9a465596768b0211c31dd41d3e73` (Ruff 0.15.0). Both `ruff_python_parser` and `ruff_python_ast` must use the same SHA.

## Workspace Dependency Pattern
- Third-party deps go in `[workspace.dependencies]` in root `Cargo.toml` (pinned versions), then crates reference them with `dep.workspace = true` in their `Cargo.toml`
- New crates go in `members = ["crates/*"]` (already glob-based, so just creating the directory suffices)

## Insta Snapshot Testing
- After changing any serialized type (Node variants, TestNode, etc.), run `INSTA_UPDATE=1 cargo test -q` to auto-update snapshots, then `cargo insta review` to verify changes are correct
- Snapshot files live in `crates/*/src/snapshots/` directories adjacent to the source

## Corpus / Golden Tests (`djls-extraction`)
- **Corpus tests**: Gated on `find_corpus_dir()` which checks `DJLS_CORPUS_PATH` env var + relative paths from crate. Skip gracefully when not present.
- **Django golden tests**: Use `find_django_source()` which checks `DJANGO_SOURCE_PATH` + venv at project root and main repo root (for worktrees). Tests `defaulttags.py`, `defaultfilters.py`, `i18n.py`, `static.py`.
- **Worktree venv path**: Golden tests check both `../../.venv/` (main repo root) and `.venv/` (worktree root) since worktrees share the main repo's venv.

## Task Management
Use `/dex` to break down complex work, track progress across sessions, and coordinate multi-step implementations.
