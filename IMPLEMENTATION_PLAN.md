# Implementation Plan: Template Validation Port

Tracking progress for porting `template_linter/` capabilities into Rust `django-language-server`.

**Charter:** `.agents/charter/2026-02-05-template-validation-port-charter.md`
**Roadmap:** `.agents/ROADMAP.md`

---

## M1 — Payload Shape + Library Name Fix

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m1-payload-library-name-fix.md`

### Phase 1: Python Inspector Payload Changes

- [x] Update `TemplateTag` dataclass to add `provenance` (externally-tagged dict) and `defining_module` fields
- [x] Update `TemplateTagQueryData` to add top-level `libraries: dict[str, str]` and `builtins: list[str]`
- [x] Rewrite `get_installed_templatetags()` to iterate `engine.template_builtins` with `Builtin` provenance
- [x] Rewrite `get_installed_templatetags()` to iterate `engine.libraries.items()` preserving load-name keys with `Library` provenance
- [x] Verify `cargo build -q` passes (build.rs rebuilds pyz)

### Phase 2: Rust Type Updates

- [x] Add `TagProvenance` enum with `Library { load_name, module }` and `Builtin { module }` variants, serde-compatible with Python's externally-tagged dict
- [x] Update `TemplateTag` struct: replace `module` with `provenance: TagProvenance` and `defining_module: String`
- [x] Add accessors: `defining_module()`, `registration_module()`, `library_load_name()`, `is_builtin()`
- [x] Expand `TemplateTags` with `libraries: HashMap<String, String>` and `builtins: Vec<String>` + accessors
- [x] Derive `PartialEq`/`Eq` where needed
- [x] Update the `templatetags` Salsa query to construct `TemplateTags` from expanded response
- [x] Export `TagProvenance` from `crates/djls-project/src/lib.rs`
- [x] Add unit tests for `TagProvenance` deserialization and accessor methods
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Completions Fix

- [x] Fix `generate_library_completions()` to use `tags.libraries()` keys instead of module paths
- [x] Sort library names alphabetically for deterministic ordering
- [x] Update tag completion detail text with provenance info (library load-name / builtin hint)
- [x] Ensure tag iteration works with updated `TemplateTags` type
- [x] Add tests: library completions show names not paths, deterministic order, builtins excluded
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M2 — Salsa Invalidation Plumbing

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`

_Tasks to be expanded when M1 is complete._

---

## M3 — `{% load %}` Scoping Infrastructure

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m3-load-scoping.md`

_Tasks to be expanded when M2 is complete._

---

## M4 — Filters Pipeline

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m4-filters-pipeline.md`

_Tasks to be expanded when M3 is complete._

---

## M5 — Extraction Engine (`djls-extraction`)

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m5-extraction-engine.md`

_Tasks to be expanded when M4 is complete._

---

## M6 — Rule Evaluation + Expression Validation

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m6-rule-evaluation.md`

_Tasks to be expanded when M5 is complete._

---

## M7 — Documentation + Issue Reporting

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m7-docs-and-issue-template.md`

_Tasks to be expanded when M6 is complete._

---

## Discoveries / Notes

- **Incomplete library loop in queries.py**: The `engine.libraries` iteration (line ~143) still uses old `module=` field instead of `provenance=`/`defining_module=`, and `libraries={}` is a placeholder dict. This is the next task in Phase 1.
- **`target/` tracked in worktree git**: Build artifacts were committed because worktree `.gitignore` doesn't exclude `target/`. Should be fixed to avoid bloating commits.
