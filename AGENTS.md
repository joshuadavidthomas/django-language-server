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

## Testing
**All tests must pass.** If a test fails, it is your responsibility to fix it — even if you didn't cause the failure. Never dismiss failures as "pre-existing" or "unrelated".

## Generated Content
- Do not edit text inside cog-generated blocks by hand. Update the source of truth, then run `just cog` to regenerate the block.

## Crate Routing
For the deeper map, read `ARCHITECTURE.md`. Quick routing:
- `djls-server`: LSP/session glue. Resolve documents, check file kind, call `djls-ide`.
- `djls-ide`: IDE feature behavior and LSP-shaped outputs.
- `djls-format`: formatter backend adapter boundary.
- `djls-conf`: config schema/loading.
- `djls-semantic`: Django/project/template meaning.
- `djls-templates`: template syntax only.
- `djls-source`: files, spans, line indexes, diagnostics primitives.
- `djls-workspace`: VFS, open buffers, file discovery.

## Code Style
- Use `tower-lsp-server`, not `tower-lsp`; import LSP types via `tower_lsp_server::ls_types`.
- Use `camino::Utf8Path`/`Utf8PathBuf` as canonical path types. Convert from `std::path` only at API boundaries.
- Imports are one per line, grouped std/external/crate, formatted by `.rustfmt.toml`.
- Formatting uses `cargo +nightly fmt` through `just fmt` because `.rustfmt.toml` enables nightly-only import formatting features. Do not run `cargo fmt --check` directly; use `just fmt --check`.
- Use `anyhow::Result` in binaries and `thiserror` in libraries.
- Prefer comments that explain why; do not write obvious doc comments.
- Use `folder.rs`, not `folder/mod.rs`.

## Task management
Use `/dex` for multi-step work that needs task tracking across sessions.
