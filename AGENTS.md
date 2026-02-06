# Agent Guidelines

## Build/Test Commands
```bash
cargo build -q                   # Build all crates
cargo clippy -q --all-targets --all-features --fix -- -D warnings  # Lint with fixes
cargo +nightly fmt               # Format code (requires nightly)
cargo test -q                    # Run all tests
cargo test test_name             # Run single test by name
cargo test -p crate_name         # Test specific crate
just test                        # Run tests via nox (with Django matrix)
just lint                        # Run pre-commit hooks
# NEVER use `cargo doc --open` - it requires browser interaction
```

## Code Style
- **IMPORTANT LSP**: Use `tower-lsp-server` NOT `tower-lsp`. Imports are `tower_lsp_server::*` NOT `tower_lsp::*`
- **LSP Types**: Use `tower_lsp_server::lsp_types` — comes transitively, don't add `lsp-types` directly
- **Imports**: One per line, grouped (std/external/crate), vertical layout per `.rustfmt.toml`
- **Errors**: Use `anyhow::Result` for binaries, `thiserror` for libraries
- **Naming**: snake_case functions/variables, CamelCase types, SCREAMING_SNAKE constants
- **Comments**: Avoid unless essential; use doc comments `///` for public APIs only
- **Testing**: Use `insta` for snapshot tests in template parser. NEVER write standalone test files — always add test cases to existing test modules in the codebase
- **Python**: Inspector runs via zipapp, test against Django 4.2/5.1/5.2/main
- **Module convention**: Uses `folder.rs` NOT `folder/mod.rs` (e.g. `templatetags.rs` + `templatetags/specs.rs`)

## Project Structure
- `crates/djls/` - Main CLI binary and PyO3 interface
- `crates/djls-server/` - LSP server implementation
- `crates/djls-templates/` - Django template parser
- `crates/djls-workspace/` - Workspace/document management
- `crates/djls-extraction/` - Python AST analysis via Ruff parser (feature-gated)
- `crates/djls-ide/` - Completions, diagnostics, snippets
- `crates/djls-semantic/` - Semantic analysis, validation, load resolution
- `crates/djls-project/` - Project/inspector types, Salsa inputs, module resolution
- `crates/djls-conf/` - Settings and diagnostics configuration
- `crates/djls-corpus/` - Corpus syncing for integration tests

## Salsa Patterns
- **Setter API**: `project.set_field(db).to(value)` — NOT `.set_field(db, value)`. The `.to()` call is required.
- **Compare before setting**: `project.field(db) != &new_value` before calling setter — setters always invalidate.
- **`#[returns(ref)]`**: Use on fields returning owned types. Salsa returns `&T`, so compare with `&new_value`.
- **Tracked return types need `PartialEq`**: Salsa uses equality for backdate optimization.

## Key Conventions
- **Parser `Node::Tag.bits` excludes tag name**: `{% load i18n %}` → `name: "load"`, `bits: ["i18n"]`. Functions processing `bits` work with arguments only.
- **Workspace deps**: Third-party deps go in `[workspace.dependencies]` in root `Cargo.toml`, crates reference with `dep.workspace = true`.
- **Insta snapshots**: After changing serialized types, run `INSTA_UPDATE=1 cargo test -q` then `cargo insta review`.
- **Extraction feature gate**: `djls-extraction` has a `parser` feature gating Ruff deps. Types in `types.rs` are always available. Crates doing extraction use `features = ["parser"]`; crates needing only types use default features off.
- **`ValidationError` is exhaustive**: When adding/removing variants, update `errors.rs`, `diagnostics.rs` (S-code mapping), and test helpers. Grep: `grep -rn "ValidationError" crates/ --include="*.rs"`.
- **`SemanticDb` trait**: When adding methods, update impls in `djls-server/src/db.rs` and `djls-bench/src/db.rs`.
- **`crate::Db` in `djls-semantic`**: When adding methods, update ALL test databases (~9 files). E0046 if you miss one. Grep: `grep -rn "impl crate::Db" crates/djls-semantic/ --include="*.rs"`.

## Task Management
Use `/dex` to break down complex work, track progress across sessions, and coordinate multi-step implementations.
