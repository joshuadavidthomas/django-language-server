# Implementation Plan: Template Validation Port

**Program:** Port `template_linter/` capabilities to Rust (`django-language-server`)
**Charter:** `.agents/charter/2026-02-05-template-validation-port-charter.md`
**Roadmap:** `.agents/ROADMAP.md`

---

## M1 - Payload Shape + Library Name Fix

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m1-payload-library-name-fix.md`

### Phase 1: Python Inspector Payload Changes

- [x] Update `TemplateTag` dataclass in `queries.py` to include `provenance` dict and `defining_module` field
- [x] Add `TemplateTagQueryData` dataclass with `libraries`, `builtins`, and `templatetags` fields
- [x] Rewrite `get_installed_templatetags()` to preserve library load-name keys from `engine.libraries`
- [x] Collect builtins using `zip(engine.builtins, engine.template_builtins)` with length guard
- [x] Collect library tags preserving `load_name` from `engine.libraries` iteration
- [x] Verify inspector payload manually: `libraries` dict, `builtins` list, provenance on each tag
- [x] Run full `cargo build`, `cargo clippy`, `cargo test`

### Phase 2: Rust Type Updates

- [x] Add `TagProvenance` enum (`Library { load_name, module }` / `Builtin { module }`) in `crates/djls-project/src/django.rs`
- [x] Update `TemplateTag` struct: replace `module` with `provenance` + `defining_module`
- [x] Add accessors: `library_load_name()`, `is_builtin()`, `registration_module()`, `defining_module()`
- [x] Add `TemplatetagsResponse` struct with `libraries`, `builtins`, `templatetags`
- [x] Update `TemplateTags` to hold `libraries: HashMap<String, String>`, `builtins: Vec<String>`, `tags: Vec<TemplateTag>`
- [x] Add `TemplateTags` accessors: `libraries()`, `builtins()`, `tags()`, `iter()`, `len()`, `is_empty()`
- [x] Add test constructors: `TemplateTag::new_library()`, `TemplateTag::new_builtin()`, `TemplateTags::new()`
- [x] Update `templatetags()` Salsa query to use new response structure
- [x] Export `TagProvenance` and `TemplateTag` from `crates/djls-project/src/lib.rs`
- [x] Add unit tests: deserialization, accessors, registry data
- [x] Fix all compilation errors in downstream crates (`djls-ide`, `djls-server`, `djls-semantic`)
- [x] Run full `cargo build`, `cargo clippy`, `cargo test`

### Phase 3: Completions Fix

- [x] Rewrite `generate_library_completions()` to use `tags.libraries()` keys instead of `tag.module()`
- [x] Sort library names alphabetically for deterministic completion ordering
- [x] Exclude builtins from `{% load %}` completions (they're always available)
- [x] Update tag name completion detail to show provenance info ("builtin from ..." / "from ... ({% load X %})")
- [x] Update any remaining `tag.module()` calls to use new accessors
- [x] Add completion tests for library name completions
- [x] Run full `cargo build`, `cargo clippy`, `cargo test`

---

## M2 - Salsa Invalidation Plumbing

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`

_Tasks to be expanded when M1 is complete._

---

## M3 - `{% load %}` Scoping Infrastructure

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m3-load-scoping.md` (overview), phases in `m3.1` through `m3.6`

_Tasks to be expanded when M2 is complete._

---

## M4 - Filters Pipeline

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m4-filters-pipeline.md` (overview), phases in `m4.1` through `m4.4`

_Tasks to be expanded when M3 is complete._

---

## M5 - Extraction Engine (`djls-extraction`)

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m5-extraction-engine.md` (overview), phases in `m5.1` through `m5.9`

_Tasks to be expanded when M4 is complete._

---

## M6 - Rule Evaluation + Expression Validation

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m6-rule-evaluation.md` (overview), phases in `m6.1` through `m6.2`

_Tasks to be expanded when M5 is complete._

---

## M7 - Documentation + Issue Reporting

**Status:** backlog
**Plan:** `.agents/plans/2026-02-05-m7-docs-and-issue-template.md`

_Tasks to be expanded when M6 is complete._

---

## Discoveries / Notes

- M1: `TemplateTags` no longer implements `Deref<Target=Vec<TemplateTag>>`. Use `.iter()`, `.tags()`, `.len()`, `.is_empty()` instead.
- M1: `TemplateTag` no longer has `.module()`. Use `.defining_module()` (where function is defined), `.registration_module()` (library/builtin module), or `.library_load_name()` (load name for `{% load %}`).
- M1: Clippy requires `#[must_use]` on all public accessors and constructors in this project.
- M1: `TemplateTag` and `TagProvenance` are now exported from `djls-project`.
