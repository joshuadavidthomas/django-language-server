---
name: djls-resolver-corpus
description: Maintain the ported ty resolver test corpus in crates/djls-project/tests/resolve.rs. Use when bumping the ruff git pin in the workspace Cargo.toml or when adding/reviewing module-resolution tests.
---

# DJLS Resolver Corpus

Use this when maintaining the ported module-resolution tests in `crates/djls-project/tests/resolve.rs`.

## Provenance

`crates/djls-project/tests/resolve.rs` ports module-resolution tests from ty's resolver in astral-sh/ruff:

- `crates/ty_module_resolver/src/resolve.rs`
- `crates/ty_module_resolver/src/path.rs`
- `crates/ty_module_resolver/src/list.rs`

Each ported test should carry a `// ty:<file>::<name>` provenance comment that points back to the source test.

## Runtime expectations

Correct expectations where ty's typechecker semantics diverge from CPython's import system:

- Stubs and typeshed never apply.
- Namespace packages resolve.

## Ruff pin bumps

When bumping the ruff git rev in the workspace `Cargo.toml`, diff the ty resolver test files between the old and new revs. Triage added or changed tests into one of these outcomes:

- Port as-is.
- Port with runtime-corrected expectations.
- Skip typechecker-only cases.
