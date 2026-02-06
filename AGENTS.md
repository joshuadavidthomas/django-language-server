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
- **Rust Django types**: `crates/djls-project/src/django.rs` — `TemplateTag`, `TemplateTags`, `TagProvenance` types and accessors
- **Project Salsa input**: `crates/djls-project/src/project.rs` — `Project` struct with all Salsa input fields
- **Database + queries**: `crates/djls-server/src/db.rs` — `DjangoDatabase`, update/refresh methods, tracked queries go here
- **Project lib.rs exports**: `crates/djls-project/src/lib.rs` — re-exports for `TagProvenance`, `TemplateTags`, inspector request/response types
- **Completions**: `crates/djls-ide/src/completions.rs` — `generate_library_completions()` at ~line 526
- **Semantic templatetags module**: `crates/djls-semantic/src/templatetags.rs` (NOT `templatetags/mod.rs`)
- **Semantic specs**: `crates/djls-semantic/src/templatetags/specs.rs` — `TagSpecs`, `TagIndex`, `django_builtin_specs()`
- **Semantic builtins**: `crates/djls-semantic/src/templatetags/builtins.rs` — builtin tag spec definitions
- **Block grammar**: `crates/djls-semantic/src/blocks/grammar.rs` — block structure parsing (read 5x in sessions)
- **Config types**: `crates/djls-conf/` — `TagSpecDef`, `DiagnosticsConfig`, `Settings`; `tagspecs.rs` for `TagSpecDef`

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

## Task Management
Use `/dex` to break down complex work, track progress across sessions, and coordinate multi-step implementations.
