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
- **Clippy**: Use inline format args - `format!("{var}")` not `format!("{}", var)` (clippy::uninlined_format_args)

## Project Structure
- `crates/djls/` - Main CLI binary and PyO3 interface
- `crates/djls-server/` - LSP server implementation  
- `crates/djls-templates/` - Django template parser
- `crates/djls-workspace/` - Workspace/document management
- `crates/djls-project/` - Inspector integration, Django queries, template tags
- `crates/djls-ide/` - LSP handlers (completions, hover, etc.)
- `crates/djls-semantic/` - Tag specifications and semantic analysis
- `crates/djls-project/inspector/` - Python inspector source files

## Struct Design Patterns
- Remove `Deref` impl when a struct gains multiple fields - use explicit accessor methods instead
- Use `#[must_use]` on pure accessor methods that return borrowed data
- For collections in structs, provide `.iter()` or typed accessors rather than `Deref` to inner Vec
- When struct fields need deterministic ordering (e.g., for tests), sort explicitly rather than relying on HashMap iteration order

## Task Management
Use `/dex` to break down complex work, track progress across sessions, and coordinate multi-step implementations.

## Code Editing
- `edit` tool requires exact text match including all whitespace and newlines
- When modifying complex files, read the exact section first to ensure whitespace matches
- Prefer smaller, surgical edits over large replacements to avoid matching errors

## Completion Implementation Notes
- Library completions (`{% load %}`) use `tags.libraries()` HashMap, not all tags - builtins are excluded
- Sort completion items alphabetically for deterministic test results
- Use `filter_text` for completion items to improve matching behavior
- Detail text format: `from {module} ({% load {name} %})` for library tags, `builtin from {module}` for builtins

## TagIndex Construction
- `TagIndex::from_specs()` takes 2 parameters: `db: &'dyn crate::Db` and `specs: &TagSpecs`
- When calling from tracked functions, pass specs explicitly rather than querying internally
- Required signature in all implementations (db.rs, test impls, bench db)

## Project Bootstrap
- `Project::bootstrap()` takes 6 arguments: `db`, `root`, `venv_path`, `django_settings_module`, `pythonpath`, AND `settings` — when adding new Project fields, update all callers (e.g., `djls-server/src/db.rs:144`)

## Module Visibility
- The `djls-project` crate has private modules that are re-exported at crate root
- Import from `djls_project::` directly, not from internal module paths (e.g., `djls_project::django::TemplatetagsResponse` is private, use `djls_project::TemplatetagsResponse`)

## Salsa Tracked Functions
- Return types must implement `PartialEq` (Salsa requirement for memoization)
- Salsa tracked functions should use `&dyn SemanticDb` (not `&dyn salsa::Database`) for proper trait bounds
- To establish proper Salsa dependencies, pass data as parameters rather than querying internally — e.g., `TagIndex::from_specs(db, &specs)` instead of having `from_specs` call `db.tag_specs()` internally
- Types containing `FxHashMap` require manual `PartialEq` impl for Salsa tracked function returns (auto-derive fails)
- Import `salsa::Setter` trait to use `.to()` method on Salsa input setters
