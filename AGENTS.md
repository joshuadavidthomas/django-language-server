# Agent Guidelines

## Commands
```bash
cargo build -q                   # Build all crates
cargo test -q                    # Run all tests
cargo test test_name             # Run one test by name
cargo test -p crate_name         # Test one crate
just test                        # Run nox/Django matrix
just clippy                      # Lint with clippy
just fmt                         # Format code with nightly rustfmt features
just fmt --check                 # Check formatting with nightly rustfmt features
just lint                        # Run pre-commit hooks
just corpus sync                 # Download corpus from lockfile
just corpus sync -U              # Re-resolve corpus and sync
```

Before pushing, run `just clippy`, `just fmt`, and `just lint`. Never use `cargo doc --open`.

Formatting uses `cargo +nightly fmt` through `just fmt` because `.rustfmt.toml` enables nightly-only import formatting features. Do not run `cargo fmt --check` directly; use `just fmt --check`.

## Testing
**All tests must pass.** If a test fails, it is your responsibility to fix it — even if you didn't cause the failure. Never dismiss failures as "pre-existing" or "unrelated".

## Code Style
- Use `tower-lsp-server`, not `tower-lsp`; import LSP types via `tower_lsp_server::ls_types`.
- Use `camino::Utf8Path`/`Utf8PathBuf` as canonical path types. Convert from `std::path` only at API boundaries.
- Imports are one per line, grouped std/external/crate, formatted by `.rustfmt.toml`.
- Use `anyhow::Result` in binaries and `thiserror` in libraries.
- Prefer comments that explain why; do not write obvious doc comments.
- Use `folder.rs`, not `folder/mod.rs`.

## Task management
Use `/dex` for multi-step work that needs task tracking across sessions.
