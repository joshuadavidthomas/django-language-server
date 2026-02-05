# Agent Guidelines

## Build/Test Commands
```bash
cargo build                      # Build all crates
cargo clippy --all-targets --all-features --fix -- -D warnings  # Lint with fixes
cargo +nightly fmt              # Format code (requires nightly)
cargo test                      # Run all tests  
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

## Operational Notes

### API Shape
- `TemplateTags` does not implement `Deref` — use `.iter()`, `.tags()`, `.len()`, `.is_empty()`
- `TemplateTag` has no `.module()` — use `.defining_module()`, `.registration_module()`, or `.library_load_name()`
- Return `&str` not `&String` from new accessors — clippy flags this
- All public accessors/constructors need `#[must_use]` — clippy enforces `must_use_candidate`
- Pass `&Settings` not `Settings` — clippy flags needless pass by value on large types

### Salsa Patterns
- `#[salsa::tracked]` functions require `&dyn Trait` — cannot use concrete `&DjangoDatabase`
- Tracked return types need `PartialEq` — add derive if missing (e.g., `TagSpecs`)
- Input setters require `use salsa::Setter` — the `.to()` method is a trait method, not inherent
- `DjangoDatabase` already has `#[cfg(test)]` event logging via `logs: Arc<Mutex<Option<Vec<String>>>>` in `db.rs` — reuse for invalidation tests
- `DjangoDatabase::default()` (test-only) creates an `InMemoryFileSystem` and wires up event logging

### Build & Inspector
- After editing `queries.py`, `cargo build` triggers pyz rebuild via `build.rs`
- Inspector rebuild warnings in clippy output (`Building Python inspector...`) are expected, not errors
- `TemplatetagsRequest`/`TemplatetagsResponse` and `inspector_query` are exported from `djls-project`

### Module Layout
- This project uses `foo.rs` + `foo/` sibling pattern — NEVER `foo/mod.rs`
- `djls-semantic` templatetags module: `src/templatetags.rs` (re-exports) + `src/templatetags/` dir (contains `specs.rs`, `builtins.rs`)
- `djls-conf` tagspec types have `PartialEq` but NOT `Eq` — `serde_json::Value` in `extra` field prevents `Eq`

### File Locations (avoid repeated lookups)
- Salsa database + tracked queries: `crates/djls-server/src/db.rs`
- Project salsa input: `crates/djls-project/src/project.rs`
- Tag specs + `from_config_def`: `crates/djls-semantic/src/templatetags/specs.rs`
- Django builtins specs: `crates/djls-semantic/src/templatetags/builtins.rs`
- Inspector Python queries: `crates/djls-project/inspector/queries.py`
- Session/server wiring: `crates/djls-server/src/session.rs`, `crates/djls-server/src/server.rs`
- Settings/config types: `crates/djls-conf/src/`

## Task Management
Use `/dex` to break down complex work, track progress across sessions, and coordinate multi-step implementations.
