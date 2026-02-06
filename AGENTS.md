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
- **Module convention**: Uses `folder.rs` NOT `folder/mod.rs`. E.g. `templatetags.rs` + `templatetags/specs.rs`, NOT `templatetags/mod.rs`

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
- **Filter snapshots**: `crates/djls-templates/src/snapshots/` — `parse_django_variable_with_filter.snap`, `parse_filter_chains.snap` — currently flat strings, will become structured `Filter` objects

## Django Engine Internals (for inspector work)
- `engine.builtins` — `list[str]` of module paths (e.g., `"django.template.defaulttags"`)
- `engine.template_builtins` — `list[Library]` of loaded Library objects, **parallel to `engine.builtins`**
- `engine.libraries` — `dict[str, str]` mapping load-name → module path (e.g., `{"static": "django.templatetags.static"}`)
- Use `zip(engine.builtins, engine.template_builtins)` to pair module paths with Library objects
- Use `engine.libraries.items()` (not `.values()`) to preserve load-name keys

## Worktree Gotchas
- **`target/` in `.gitignore`** — already added, but verify before `git add -A` that it's still excluded. Worktree `.gitignore` is separate from the main repo's.

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
- **Scoping exclusions**: Only skip closers/intermediates for load scoping checks — openers like `trans` have TagSpecs (for argument validation) BUT still need scoping because they're library tags. `django_builtin_specs()` includes ALL Django tags, not just builtins.
- **Diagnostic codes**: S108 = unknown tag (not in any library), S109 = unloaded tag (known library, not loaded), S110 = ambiguous unloaded tag (multiple candidate libraries). All three are guarded by `inspector_inventory.is_some()`.
- **Completions depend on load scoping**: `generate_tag_name_completions` needs `LoadedLibraries` + inspector inventory to filter results by position. When inspector unavailable, show all tags as fallback.
- **SemanticDb trait changes**: When adding methods to `SemanticDb`, update ALL test databases: `arguments.rs`, `blocks/tree.rs`, `semantic/forest.rs`, `load_resolution.rs`, `load_resolution/validation.rs`, `djls-bench/src/db.rs`, `djls-server/src/db.rs`
- **`crate::Db` vs `SemanticDb`**: In `djls-semantic`, test databases implement `crate::Db` (Salsa jar trait). `SemanticDb` (runtime trait) is only implemented on `DjangoDatabase` in `djls-server` and `Db` in `djls-bench`. Don't confuse the two.

## Cross-Cutting Type Changes
- **When adding a new parallel type** (e.g., `TemplateFilter` mirroring `TemplateTag`): update Python dataclass, `queries.py` collection, Rust struct, response type, `TemplateTags` struct + `new()` + `from_response()`, tracked query, `lib.rs` re-exports. Easy to miss one step in the chain.
- **`Node::Variable` filter changes cascade widely**: Changing `filters: Vec<String>` to a structured type requires updates in: `nodelist.rs` (Node enum), `parser.rs` (parse_variable + TestNode), `context.rs` (OffsetContext::Variable), `blocks/tree.rs` (NodeView::Variable), `completions.rs` (any filter handling), plus all insta snapshots. Run `cargo insta review` or `INSTA_UPDATE=1 cargo test` after.
- **`TemplateFilter` shares `TagProvenance`**: Filters use the same `TagProvenance` enum as tags (Library/Builtin variants). Don't create a separate provenance type for filters.

## Insta Snapshot Testing
- After changing any serialized type (Node variants, TestNode, etc.), run `INSTA_UPDATE=1 cargo test -q` to auto-update snapshots, then `cargo insta review` to verify changes are correct
- Snapshot files live in `crates/*/src/snapshots/` directories adjacent to the source

## Task Management
Use `/dex` to break down complex work, track progress across sessions, and coordinate multi-step implementations.
