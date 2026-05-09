---
name: djls-testing
description: Use when running tests, fixing failing tests, updating snapshots, syncing corpus data, validating before push, or working with Django matrix testing in django-language-server. Handles cargo, just, nox, insta, corpus, clippy, fmt, and lint commands.
---

# DJLS Testing

Use this when validating changes or fixing test failures.

## Common commands

```bash
cargo build -q                    # Build all crates
cargo test -q                     # Run all tests
cargo test test_name              # Run one test by name
cargo test -p crate_name          # Test one crate
just test                         # Run nox/Django matrix
just clippy                       # Lint with clippy
just fmt                          # Format code
just lint                         # Run pre-commit hooks
just corpus lock                  # Resolve corpus versions and update lockfile
just corpus sync                  # Download corpus from lockfile
just corpus sync -U               # Re-resolve corpus and sync
just corpus clean                 # Remove synced corpus data
just dev profile <bench> [filter] # Profile a bench with callgrind
```

Never use `cargo doc --open`.

## Rules

- All tests must pass. Fix failures instead of dismissing them as pre-existing or unrelated.
- If corpus tests fail, run `just corpus sync` and fix/update snapshots as needed.
- Before pushing, run `just clippy`, `just fmt`, and `just lint`.
- Python inspector changes must be tested against all supported Django versions; see `DJ_VERSIONS` in `noxfile.py`.

## Snapshots

- Use `insta` for template parser snapshots.
- Add cases to existing test modules; do not create standalone test files.
- After changing serialized types, run:

```bash
cargo insta test --accept --unreferenced delete
```
