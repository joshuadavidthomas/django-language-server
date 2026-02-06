# Template Validation Port: Implementation Plan

**Date:** 2026-02-05  
**Charter:** [`.agents/charter/2026-02-05-template-validation-port-charter.md`](.agents/charter/2026-02-05-template-validation-port-charter.md)  
**Roadmap:** [`.agents/ROADMAP.md`](.agents/ROADMAP.md)

This document tracks progress through the milestones for porting the Python `template_linter/` prototype into Rust `django-language-server` (djls).

---

## Milestones Overview

| # | Milestone | Status | Plan File |
|---|-----------|--------|-----------|
| M1 | Payload Shape + `{% load %}` Library Name Fix | 🔲 In Progress | [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](.agents/plans/2026-02-05-m1-payload-library-name-fix.md) |
| M2 | Salsa Invalidation Plumbing | 🔲 Not Started | [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md) |
| M3 | `{% load %}` Scoping Infrastructure | 🔲 Not Started | [`.agents/plans/2026-02-05-m3-load-scoping.md`](.agents/plans/2026-02-05-m3-load-scoping.md) |
| M4 | Filters Pipeline | 🔲 Not Started | [`.agents/plans/2026-02-05-m4-filters-pipeline.md`](.agents/plans/2026-02-05-m4-filters-pipeline.md) |
| M5 | Rust Extraction Engine (`djls-extraction`) | 🔲 Not Started | [`.agents/plans/2026-02-05-m5-extraction-engine.md`](.agents/plans/2026-02-05-m5-extraction-engine.md) |
| M6 | Rule Evaluation + Expression Validation | 🔲 Not Started | [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](.agents/plans/2026-02-05-m6-rule-evaluation.md) |
| M7 | Documentation + Issue Reporting | 🔲 Not Started | [`.agents/plans/2026-02-05-m7-docs-and-issue-template.md`](.agents/plans/2026-02-05-m7-docs-and-issue-template.md) |

**Legend:**
- 🔲 Not Started / Backlog
- 📝 Ready (plan exists, waiting to implement)
- 🔄 In Progress
- ✅ Complete

---

## M1: Payload Shape + `{% load %}` Library Name Fix

**Goal:** Fix the inspector payload structure to preserve Django library load-names and distinguish builtins from loadable libraries, then fix completions to show correct library names for `{% load %}`.

**Plan:** [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](.agents/plans/2026-02-05-m1-payload-library-name-fix.md)

**Overall Status:** ✅ Complete (all 3 phases done)

### Phase 1: Python Inspector Payload Changes

**Status:** ✅ Complete

Update the inspector to return library information with proper provenance distinction, plus top-level registry structures for downstream use.

**Changes:**
- Added `provenance` dict field and `defining_module` field to `TemplateTag` dataclass
- Expanded `TemplateTagQueryData` to include `libraries`, `builtins`, and `templatetags`
- Rewrote `get_installed_templatetags()` to preserve library keys using `engine.libraries` and correctly pair `engine.builtins` with `engine.template_builtins` using `zip()`
- Added runtime guard to ensure builtins/template_builtins lengths match

**Quality Checks:**
- [x] `cargo build` passes
- [x] `cargo test -p djls-project` passes
- [x] All tests pass (`cargo test -q`: 217 tests passed)

**Discoveries:**
- The `engine.builtins` provides ordered module paths while `engine.template_builtins` provides the `Library` objects - they must be paired with `zip()` for correct provenance

### Phase 2: Rust Type Updates

**Status:** ✅ Complete

Update Rust types to deserialize the new payload structure with `TagProvenance` enum and top-level registry data.

**Changes:**
- Added `TagProvenance` enum with `Library { load_name, module }` and `Builtin { module }` variants
- Updated `TemplateTag` struct with `provenance`, `defining_module` fields; added `library_load_name()`, `is_builtin()`, `registration_module()` accessors
- Expanded `TemplateTags` to hold `libraries: HashMap<String, String>`, `builtins: Vec<String>`, `tags: Vec<TemplateTag>`; removed `Deref` impl, added `tags()`, `libraries()`, `builtins()` accessors
- Updated `templatetags()` Salsa query to construct new `TemplateTags` structure from response
- Exported `TagProvenance` and `TemplateTag` in `lib.rs`
- Added 5 new tests for provenance, deserialization, registry data, and constructors
- Updated `generate_library_completions()` to use `tags.libraries()` with alphabetical sorting
- Updated tag detail generation to show `from ... ({% load X %})` for library tags and `builtin from ...` for builtins

**Quality Checks:**
- [x] `cargo build -p djls-project` passes
- [x] `cargo clippy -p djls-project --all-targets -- -D warnings` passes
- [x] `cargo test -p djls-project` passes

**Discoveries:**
- The `Deref` implementation on `TemplateTags` had to be removed because the struct now has multiple fields; use `.iter()` or `.tags()` instead
- Using inline format args (`format!("{module_path}")`) required by clippy

### Phase 3: Completions Fix

**Status:** ✅ Complete

Update completions to use library load-name and exclude builtins from `{% load %}` completions.

**Changes:**
- Rewrote `generate_library_completions()` to use `tags.libraries()` with alphabetical sorting for deterministic ordering
- Changed completion labels to show load names (`static`, `i18n`) instead of module paths (`django.templatetags.static`)
- Updated detail text to show `from {module} ({% load {name} %})` for library tags, `builtin from {module}` for builtins
- Updated `generate_tag_name_completions()` to use new `tag.defining_module()` and `tag.library_load_name()` accessors

**Quality Checks:**
- [x] `cargo build -p djls-ide` passes
- [x] `cargo clippy -p djls-ide --all-targets -- -D warnings` passes
- [x] `cargo test -p djls-ide` passes
- [x] `cargo build` (full build) passes
- [x] `cargo test` (all tests) passes

**Discoveries:**
- Library completions are now properly filtered to exclude builtins (they're not in `libraries()` map)
- Deterministic ordering (alphabetical) ensures consistent test results

---

## M2: Salsa Invalidation Plumbing

**Status:** 🔲 Not Started

**Goal:** Prevent stale template diagnostics by making external data sources explicit Salsa inputs with an explicit refresh/update path.

**Plan:** [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md)

### Tasks (TBD - will expand when M1 complete)

---

## M3: `{% load %}` Scoping Infrastructure

**Status:** 🔲 Not Started

**Goal:** Position-aware `{% load %}` scoping for tags and filters in diagnostics + completions.

**Plan:** [`.agents/plans/2026-02-05-m3-load-scoping.md`](.agents/plans/2026-02-05-m3-load-scoping.md)

### Tasks (TBD - will expand when M2 complete)

---

## M4: Filters Pipeline

**Status:** 🔲 Not Started

**Goal:** Filter inventory-driven completions + unknown-filter diagnostics, with load scoping correctness, and a structured filter representation in `djls-templates`.

**Plan:** [`.agents/plans/2026-02-05-m4-filters-pipeline.md`](.agents/plans/2026-02-05-m4-filters-pipeline.md)

### Tasks (TBD - will expand when M3 complete)

---

## M5: Rust Extraction Engine

**Status:** 🔲 Not Started

**Goal:** Implement `djls-extraction` using Ruff AST to mine validation semantics from Python registration modules, keyed by SymbolKey.

**Plan:** [`.agents/plans/2026-02-05-m5-extraction-engine.md`](.agents/plans/2026-02-05-m5-extraction-engine.md)

### Tasks (TBD - will expand when M4 complete)

---

## M6: Rule Evaluation + Expression Validation

**Status:** 🔲 Not Started

**Goal:** Apply extracted rules to templates (argument constraints, block structure, opaque blocks) and add `{% if %}` / `{% elif %}` expression syntax validation.

**Plan:** [`.agents/plans/2026-02-05-m6-rule-evaluation.md`](.agents/plans/2026-02-05-m6-rule-evaluation.md)

### Tasks (TBD - will expand when M5 complete)

---

## M7: Documentation + Issue Reporting

**Status:** 🔲 Not Started

**Goal:** Update documentation to reflect the new template validation behavior and add a high-signal issue template for reporting mismatches.

**Plan:** [`.agents/plans/2026-02-05-m7-docs-and-issue-template.md`](.agents/plans/2026-02-05-m7-docs-and-issue-template.md)

### Tasks (TBD - will expand when M6 complete)

---

## Progress Notes

*Use this section to record discoveries, blockers, and decisions made during implementation.*
