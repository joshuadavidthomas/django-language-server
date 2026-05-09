---
name: djls-workspace-conventions
description: Use when adding or editing crates, Cargo.toml files, workspace dependencies, crate manifests, lints, versions, or project structure in django-language-server. Handles Rust workspace layout, dependency grouping, and new crate setup.
---

# DJLS Workspace Conventions

Use this before changing `Cargo.toml`, adding crates, or reorganizing workspace structure.

## Rules

- Crates live in `crates/` and are auto-discovered by `members = ["crates/*"]`.
- Put all dependency versions in root `[workspace.dependencies]`; crates use `dep.workspace = true`.
- Root dependency groups, in order:
  1. internal path crates
  2. pinned core deps: `salsa`, `tower-lsp-server`
  3. crates.io deps
  4. git deps: `ruff_*`
- Alphabetize within each root dependency group.
- In crate manifests, list internal deps before third-party deps, separated by a blank line.
- Every crate uses `[lints] workspace = true`.
- Only `djls` carries the release version. Library crates use `version = "0.0.0"`.

## Adding a crate

1. Add the crate under `crates/<name>/`.
2. Add it to root `[workspace.dependencies]` as an internal path dependency.
3. In `crates/<name>/Cargo.toml`, use workspace dependencies and `[lints] workspace = true`.
4. Keep module files as `folder.rs`, not `folder/mod.rs`.
