# Agent Guidelines

## Build/Test Commands
```bash
cargo build -q                   # Build all crates
cargo test -q                    # Run all tests
cargo test test_name             # Run single test by name
cargo test -p crate_name         # Test specific crate
just test                        # Run tests via nox (with Django matrix)
just clippy                      # Lint with clippy (auto-fixes)
just fmt                         # Format code (requires nightly)
just lint                        # Run pre-commit hooks
just corpus lock                 # Resolve corpus versions and update lockfile
just corpus sync                 # Download corpus from lockfile (prunes old versions)
just corpus sync -U              # Re-resolve versions then sync
just corpus clean                # Remove all synced corpus data
just dev profile <bench> [filter] # Flamegraph + collapsed stacks for a bench
# NEVER use `cargo doc --open` - it requires browser interaction
```

**Before pushing**, always run `just clippy`, `just fmt`, and `just lint`.

## Testing
**All tests must pass.** If a test fails, it is your responsibility to fix it — even if you didn't cause the failure. Never dismiss failures as "pre-existing" or "unrelated".

Sync the corpus (`just corpus sync`) if corpus tests fail, fix broken snapshots, and do whatever else is needed to get a clean run before considering work complete.

## Code Style
- LSP: Use `tower-lsp-server` NOT `tower-lsp`. Imports are `tower_lsp_server::*` NOT `tower_lsp::*`
- LSP types: Use `tower_lsp_server::ls_types` — comes transitively, don't add `ls-types` directly
- Imports: One per line, grouped (std/external/crate), vertical layout per `.rustfmt.toml`
- Errors: `anyhow::Result` for binaries, `thiserror` for libraries
- Naming: snake_case functions/variables, CamelCase types, SCREAMING_SNAKE constants
- Comments: Avoid unless essential; use doc comments `///` for public APIs only
- Testing: Use `insta` for snapshot tests in template parser. NEVER write standalone test files — always add test cases to existing test modules in the codebase
- Python: Inspector runs via zipapp, test against all supported Django versions (see `DJ_VERSIONS` in `noxfile.py`)
- Module convention: Uses `folder.rs` NOT `folder/mod.rs` (e.g. `templatetags.rs` + `templatetags/specs.rs`)

## Project Structure
- `crates/djls/` - Main CLI binary
- `crates/djls-db/` - Concrete Salsa database (`DjangoDatabase`), queries, settings, inspector refresh
- `crates/djls-server/` - LSP server implementation
- `crates/djls-templates/` - Django template parser
- `crates/djls-workspace/` - Workspace/document management
- `crates/djls-python/` - Python AST analysis via Ruff parser
- `crates/djls-ide/` - Completions, diagnostics, snippets
- `crates/djls-semantic/` - Semantic analysis, validation, load resolution
- `crates/djls-project/` - Project/inspector types, Salsa inputs, module resolution
- `crates/djls-source/` - Source DB, File type, path utilities, LSP protocol conversions
- `crates/djls-conf/` - Settings and diagnostics configuration
- `crates/djls-bench/` - Benchmark database (implements `SemanticDb`)
- `crates/djls-corpus/` - Corpus syncing for integration tests

## Workspace and Crate Conventions
- All crates live in `crates/`, auto-discovered via `members = ["crates/*"]`
- All dependency versions (third-party and internal) go in `[workspace.dependencies]` in root `Cargo.toml`. Crates reference with `dep.workspace = true`. Never specify a version directly in a crate's `Cargo.toml`.
- Root `[workspace.dependencies]` grouping: internal path crates → pinned core deps (`salsa`, `tower-lsp-server`) → crates.io deps → git deps (`ruff_*`). Blank line between groups, alphabetical within each.
- Internal deps listed before third-party in each crate's `Cargo.toml`, separated by a blank line, both groups alphabetical
- `[lints] workspace = true` in every crate — lints are configured once in root `[workspace.lints]`
- Versioning: Only `djls` (the binary) carries the release version. All library crates use `version = "0.0.0"`.
- Adding a new crate: Add to `[workspace.dependencies]` in root `Cargo.toml` (alphabetical), create `crates/<name>/Cargo.toml` with `{ workspace = true }` deps and `[lints] workspace = true`

## Salsa Patterns
- Setter API: `project.set_field(db).to(value)` — NOT `.set_field(db, value)`. The `.to()` call is required.
- Compare before setting: `project.field(db) != &new_value` before calling setter — setters always invalidate.
- `#[returns(ref)]`: Use on fields returning owned types. Salsa returns `&T`, so compare with `&new_value`.
- Tracked return types need `PartialEq`: Salsa uses equality for backdate optimization.

## Key Conventions
- Parser `Node::Tag.bits` excludes tag name: `{% load i18n %}` → `name: "load"`, `bits: ["i18n"]`. Functions processing `bits` work with arguments only.
- Paths: Use `camino::Utf8Path`/`Utf8PathBuf` as the canonical path types. Avoid `std::path::Path`/`PathBuf` except at FFI boundaries or when interfacing with APIs that require them (e.g., `walkdir` results — convert at the boundary).
- Insta snapshots: After changing serialized types, run `cargo insta test --accept --unreferenced delete` to update snapshots and clean orphans.
- Environment layout: Environment scan functions (`scan_environment`, `scan_environment_with_symbols`) live in `djls-project/src/scanning.rs`; environment types (`EnvironmentInventory`, `EnvironmentLibrary`, `EnvironmentSymbol`) in `djls-python/src/environment/types.rs`.
- `ValidationError` is exhaustive: When adding/removing variants, update `errors.rs`, `djls-ide/src/diagnostics.rs` (S-code mapping), and test helpers. Grep: `grep -rn "ValidationError" crates/ --include="*.rs"`.
- `SemanticDb` trait: When adding methods, update impls in `djls-db/src/db.rs` and `djls-bench/src/db.rs`.
- `crate::Db` in `djls-semantic`: When adding methods, update ALL test databases (~10 files). E0046 if you miss one. Grep: `grep -rn "impl crate::Db" crates/djls-semantic/ --include="*.rs"`.

## LSP Server Logs
The server writes daily log files to `~/.cache/djls/djls.log.YYYY-MM-DD`. Inspector response caches live in `~/.cache/djls/inspector/`.

## Changelog
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/) format.
- **Every user-facing change needs a changelog entry.** Add one as part of the same commit or PR — don't leave it for later.
- Entries go under `[Unreleased]` in the appropriate section (`Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`)
- Short and factual — what changed, not why. No rationale or future plans.
- Past tense verbs: "Added", "Fixed", "Removed", "Bumped"
- Prefix internal-only changes with `**Internal**:`, list after user-facing entries
- Backtick-wrap code identifiers: crate names, types, commands, config keys

## Ruff AST API (djls-extraction)
- Parse: `ruff_python_parser::parse_module(source)` → `.into_syntax()` for `ModModule` AST
- Parameters: No top-level `defaults` field — defaults are per-parameter: `ParameterWithDefault { parameter, default: Option<Box<Expr>> }`
- Box fields: `StmtWhile.test`, `StmtIf.test` etc. are `Box<Expr>` — dereference with `&*` for pattern matching
- FString: `FStringValue` uses `.iter()` not `.parts()` for `FStringPart` iteration
- ExceptHandler: `ExceptHandler::ExceptHandler` is irrefutable — use `let` not `if let`

## Task Management
Use `/dex` to break down complex work, track progress across sessions, and coordinate multi-step implementations.

Dex tasks vs GitHub issues:
- GitHub issues are the intake — bug reports, feature ideas, brainstorms from users and long-term thinking.
- Dex tasks are actionable work items: things to do now or soon.
- A dex task may be derived from a GitHub issue (via `dex import`), but not every issue becomes a task, and tasks can exist without an issue. Don't treat them as the same thing.
