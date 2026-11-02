# Agent Guidelines

Read `CONTEXT.md` for the domain glossary and canonical terminology. Read `ARCHITECTURE.md` for the deeper system design, crate boundaries, and request/data flow.

## Commands
```bash
cargo build -q                   # Build all crates
cargo test -q                    # Run Rust tests in the current Python environment
cargo test test_name             # Run one test by name
cargo test -p crate_name         # Test one crate
just test                        # Test with default Python and Django versions
just testall                     # Test all supported Python/Django combinations
just e2e                         # Run Python LSP end-to-end tests
just clippy                      # Lint with clippy
just fmt                         # Format code with nightly rustfmt features
just lint                        # Run pre-commit hooks
just hawk                        # Check crate-boundary visibility
just corpus sync                 # Download corpus from lockfile
just corpus sync -U              # Re-resolve corpus and sync
```

Before pushing, run `just clippy`, `just fmt`, and `just lint`. Never use `cargo doc --open`.

## Testing
**All tests must pass.** If a test fails, it is your responsibility to fix it — even if you didn't cause the failure. Never dismiss failures as "pre-existing" or "unrelated".

Prefer explicit tests over helper proliferation. Use helpers only for substantial fixture setup; avoid one-off assertion wrappers and tiny call-chain helpers. Model after `crates/djls-templates/src/lexer.rs`: clear input, direct execution, direct assertion/snapshot.

Keep tests that exercise tracked queries, invalidation, Project fixtures, or behavior spanning crate public APIs in `crates/*/tests/` when the behavior is reachable through the crate API. Inline `src/` `#[cfg(test)]` modules should test private algorithms and crate-private boundaries directly with explicit inputs. Reuse an existing shared or production database when the subject requires one; do not define one-off test databases.

## Benchmarks
A benchmark name is a stable comparison contract. Never rename, remove, or reshape a benchmark to hide a regression.

When a change slows a benchmark, first check for an accidental regression in the code, inputs, setup, and measured path. Profile the hot path and fix real performance bugs. Some features require more work and may remain slower after that review; keep measuring the operation honestly and record why the cost changed. Rename a benchmark only when it truly measures a different operation, explain that change, and preserve the old comparison when it still represents a useful path.

## Generated Content
- Do not edit text inside cog-generated blocks by hand. Update the source of truth, then run `just cog` to regenerate the block.

## Extraction First
Django template knowledge (which tags and filters exist, their roles, structure, argument syntax, how arguments are consumed, and `django.` module paths) must be an Extracted Fact: derived from Python source by static extraction in `djls-project`. This includes Django's own builtins, which are Python source in the bundled `crates/djls-project/vendor/django-*.zip`. When a fact is missing and visible in a compile function, extend extraction.

When extraction cannot prove a fact, add a Fallback Rule to `crates/djls-project/src/templates/tags/fallback.rs`. A Fallback Rule targets a Django compile function by canonical identity, not a tag name, names its Extraction Gap, and cites evidence: a bundled Django source `path:line`, or an extraction snapshot showing what extraction produces today. For Django's own libraries, check the bundled-Django snapshots (`cargo test -p djls-project --test it bundled_django`, `crates/djls-project/tests/it/snapshots/it__bundled_django__*`) and the provenance table (`crates/djls-semantic/tests/it/snapshots/it__bundled_django_provenance__*`); the corpus checks Django out as project code, so `corpus__repos__django-*` snapshots do not show what production extracts from Django. For third-party libraries, run `cargo test -p djls-project --test corpus` after `just corpus sync`. Without that evidence, do not add a Fallback Rule.

`djls-semantic`, `djls-ide`, and `djls-server` key behavior on Tag Roles and extracted facts, never on Django names. The `django_knowledge_guard` test in `djls-testing` enforces this for production code; a literal that matches a Django name without being Django knowledge goes in `crates/djls-testing/tests/django_knowledge_allowlist.toml` with a one-line reason.

## Crate Responsibilities
- `djls-conf`: config schema/loading.
- `djls-format`: formatter backend adapter boundary.
- `djls-ide`: IDE feature behavior and LSP-shaped outputs.
- `djls-lsp`: transport-independent LSP pieces shared by `djls-server` and `djls-wasm`: client negotiation, capabilities, open document buffers, overlay filesystem, and the synchronous session core (owns `DjangoDatabase`; document mutations, Project-work classification, document-request resolution, formatting gate).
- `djls-project`: project model, Python environment discovery, module resolution, template discovery/resolution, derived Django facts, static source recognizers, and Python spec extraction (tag rules, block specs, filter arities, model graph). This is where Django template knowledge comes from.
- `djls-semantic`: Django project meaning: template validation, scoping, structure, fusion of extracted tag facts into tag specs, and template-reference relationships.
- `djls-server`: native LSP transport around the session core: tokio session lock, readiness generations, snapshots, diagnostics queue, reload worker. Map `file://` URIs, call `djls-ide`.
- `djls-source`: files, filesystem access/discovery, spans, line indexes, diagnostics primitives.
- `djls-templates`: template syntax only.
- `djls-wasm`: browser entry point. wasm-bindgen exports, a synchronous LSP server wrapping the session core over an in-memory project, non-`file://` URI mapping, and runtime Django source registration.

Run `just hawk` when changing public APIs, moving code across crates, or cleaning up visibility. It is compile-intensive and may run multiple Cargo analysis passes. Review visibility findings against crate responsibilities before using `--fix`; run normal lint/test checks afterward for dead-code cleanup.

## Code Style
- Use `tower-lsp-server`, not `tower-lsp`; import LSP types via `tower_lsp_server::ls_types`.
- Use `camino::Utf8Path`/`Utf8PathBuf` as canonical path types. Convert from `std::path` only at API boundaries.
- Treat `lib.rs` as the external crate API. Internal code should import from the owning module path, not from crate-root re-exports.
- Module façade files may re-export their intended boundary API, but avoid re-exporting items through multiple layers unless that layer is a real domain boundary.
- Prefer `crate::<owning_module>::...` for internal imports. Use `super::...` only inside local test modules or when reaching private siblings is clearer than exposing a broader module API.
- Formatting uses `just fmt` because `.rustfmt.toml` needs nightly rustfmt. Do not run `cargo fmt` directly. Use `just fmt --check` only for explicit verification gates.
- Use `anyhow::Result` in binaries and `thiserror` in libraries.
- Prefer comments that explain why; do not write obvious doc comments.
- Use `folder.rs`, not `folder/mod.rs`.

## Task management
Use `/dex` for multi-step work that needs task tracking across sessions.
