# Template Validation Port: Implementation Plan

**Date:** 2026-02-05  
**Charter:** [`.agents/charter/2026-02-05-template-validation-port-charter.md`](.agents/charter/2026-02-05-template-validation-port-charter.md)  
**Roadmap:** [`.agents/ROADMAP.md`](.agents/ROADMAP.md)

This document tracks progress through the milestones for porting the Python `template_linter/` prototype into Rust `django-language-server` (djls).

---

## Milestones Overview

| # | Milestone | Status | Plan File |
|---|-----------|--------|-----------|
| M1 | Payload Shape + `{% load %}` Library Name Fix | ✅ Complete | [`.agents/plans/2026-02-05-m1-payload-library-name-fix.md`](.agents/plans/2026-02-05-m1-payload-library-name-fix.md) |
| M2 | Salsa Invalidation Plumbing | ✅ Complete | [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md) |
| M3 | `{% load %}` Scoping Infrastructure | 📝 Ready | [`.agents/plans/2026-02-05-m3-load-scoping.md`](.agents/plans/2026-02-05-m3-load-scoping.md) |
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

**Status:** ✅ Complete (all 4 phases done)

**Goal:** Prevent stale template diagnostics by making external data sources explicit Salsa inputs with an explicit refresh/update path.

**Plan:** [`.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`](.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md)

### Phase 1: Extend Project Input with djls-conf Types

**Status:** ✅ Complete

Add new fields to the existing `Project` Salsa input using only types from `djls-conf`. No semantic crate dependency.

**Changes:**
- Verified `TagSpecDef` and related config types already have `PartialEq` in `djls-conf`
- Added new fields to `Project`: `inspector_inventory: Option<TemplateTags>`, `tagspecs: TagSpecDef`, `diagnostics: DiagnosticsConfig`
- Updated `Project::bootstrap()` to accept `settings: &Settings` parameter and initialize new fields
- Updated caller in `djls-server/src/db.rs` to pass the new `settings` argument

**Quality Checks:**
- [x] `cargo build -p djls-project` passes
- [x] `cargo build` (full build) passes
- [x] `cargo clippy -p djls-project --all-targets -- -D warnings` passes
- [x] `cargo test` passes (220 tests)

### Phase 2: Add Project Update APIs with Manual Comparison

**Status:** ✅ Complete

Add methods to `DjangoDatabase` that update Project fields **only when values actually change** (Ruff/RA style).

**Changes:**
- Added `PartialEq` to `TemplateTags`, `TemplateTag`, `TagProvenance` (already had it, verified)
- Exported `TemplatetagsRequest` and `TemplatetagsResponse` from `djls-project` (made public in `django.rs`, exported in `lib.rs`)
- Added `TemplateTags::from_response()` constructor in `django.rs`
- Updated `set_project()` to only create Project if none exists; use setters with manual comparison for updates
- Added `update_project_from_settings()` with manual comparison for each field (interpreter, django_settings_module, pythonpath, tagspecs, diagnostics)
- Added `refresh_inspector()` that queries Python via `templatetags()` tracked function and compares before setting
- Updated `set_settings()` to delegate to `update_project_from_settings()` with `&Settings` parameter
- Added `salsa::Setter` import for setter `.to()` method
- Updated `session.rs` and `server.rs` callers to pass `&Settings` reference

**Quality Checks:**
- [x] `cargo build -p djls-server` passes
- [x] `cargo clippy -p djls-server --all-targets -- -D warnings` passes
- [x] `cargo test -p djls-server` passes (27 tests)
- [x] `cargo build` (full build) passes
- [x] `cargo test` (all tests) passes (220 tests)
- [x] `cargo clippy --all-targets --all-features -- -D warnings` passes

**Discoveries:**
- Salsa setters require importing `salsa::Setter` trait to use `.to()` method
- The private `inspector` module can be accessed via the public `templatetags` tracked function instead
- Need to update all callers when changing function signatures from owned to reference types

### Phase 3: Make tag_specs a Tracked Query

**Status:** ✅ Complete

Add `compute_tag_specs()` as a tracked query that reads only from Salsa-tracked Project fields.

**Changes:**
- Added `TagSpecs::from_config_def()` conversion method in `crates/djls-semantic/src/templatetags/specs.rs`
  - Converts `TagSpecDef` config document to `TagSpecs` semantic artifact
  - Uses existing `(TagDef, String) -> TagSpec` conversion logic
- Added `#[salsa::tracked] fn compute_tag_specs()` in `crates/djls-server/src/db.rs`
  - Reads `project.inspector_inventory(db)` and `project.tagspecs(db)` to establish Salsa dependencies
  - Starts with `django_builtin_specs()` compile-time constant
  - Merges user specs from config via `TagSpecs::from_config_def()`
- Added `#[salsa::tracked] fn compute_tag_index()` in `crates/djls-server/src/db.rs`
  - Depends on `compute_tag_specs()` for automatic invalidation cascade
  - Updated `TagIndex::from_specs()` to accept specs as parameter (not call `db.tag_specs()`)
- Updated `SemanticDb` implementation to delegate to tracked queries
  - `tag_specs()` now calls `compute_tag_specs(self, project)` when project exists
  - `tag_index()` now calls `compute_tag_index(self, project)` when project exists
  - Falls back to builtins when no project
- Added `PartialEq` impl for `TagSpecs` (required for Salsa tracked function returns)
- Updated all callers of `TagIndex::from_specs()` to pass specs parameter:
  - `djls-server/src/db.rs` (compute_tag_index)
  - `djls-bench/src/db.rs` (SemanticDb impl)
  - `djls-semantic/src/arguments.rs` (test impl)
  - `djls-semantic/src/blocks/tree.rs` (test impl)
  - `djls-semantic/src/semantic/forest.rs` (test impl)

**Quality Checks:**
- [x] `cargo build` passes
- [x] `cargo clippy --all-targets -- -D warnings` passes
- [x] `cargo test` passes (220 tests)

**Discoveries:**
- Salsa tracked functions require return types to implement `PartialEq`
- `TagIndex::from_specs()` needed signature change to accept specs as parameter rather than querying db internally - this allows tracked queries to properly establish Salsa dependencies
- `FxHashMap` doesn't have built-in `PartialEq`, so manual implementation required for `TagSpecs`

### Phase 4: Invalidation Tests with Event Capture

**Status:** ✅ Complete

Write tests that capture Salsa events and verify invalidation using stable `ingredient_debug_name()` pattern.

**Changes:**
- Added `EventLogger` test infrastructure with `was_executed()` helper in `crates/djls-server/src/db.rs`
  - Stores raw `salsa::Event` values in an `Arc<Mutex<Vec<_>>>`
  - Checks `WillExecute` events and compares `db.ingredient_debug_name(database_key.ingredient_index())` to query name
- Added `TestDatabase` helper for creating test instances with event logging
  - `TestDatabase::new()` creates empty database with logger
  - `TestDatabase::with_project()` creates database with initialized Project
- Added 6 invalidation tests:
  - `test_tag_specs_cached_on_repeated_access`: Verifies compute_tag_specs is cached after first access
  - `test_tagspecs_change_invalidates`: Verifies changing tagspecs triggers recomputation
  - `test_inspector_inventory_change_invalidates`: Verifies changing inspector inventory triggers recomputation
  - `test_same_value_no_invalidation`: Verifies no recomputation when value unchanged (no setter called)
  - `test_tag_index_depends_on_tag_specs`: Verifies tag_index properly depends on tag_specs
  - `test_update_project_from_settings_compares`: Verifies manual comparison in update_project_from_settings prevents unnecessary invalidation

**Quality Checks:**
- [x] `cargo test invalidation_tests` passes (6 tests)
- [x] `cargo test -p djls-server` passes (33 tests)
- [x] `cargo test` (full suite) passes (255 tests)
- [x] `cargo clippy --all-targets -- -D warnings` passes

**Discoveries:**
- Salsa event logging requires a callback closure passed to `salsa::Storage::new()`
- The `ingredient_debug_name()` method provides stable query identification (avoiding Debug output substring matching)
- Test database helpers make Salsa tests much cleaner and more maintainable
- `Interpreter::discover(None)` works for tests where we don't need actual Python environment detection

---

## M3: `{% load %}` Scoping Infrastructure

**Status:** 📝 Ready

**Goal:** Position-aware `{% load %}` scoping for tags and filters in diagnostics + completions.

**Plan:** [`.agents/plans/2026-02-05-m3-load-scoping.md`](.agents/plans/2026-02-05-m3-load-scoping.md)

**Overall Status:** 🔲 Not Started

### Phase 1: Load Statement Parsing and Data Structures

**Status:** 🔲 Not Started

Create the core data structures for tracking `{% load %}` statements and implement parsing of `{% load %}` bits into structured form.

**Changes:**
- Create `crates/djls-semantic/src/load_resolution.rs` with:
  - `LoadStatement` struct with span and `LoadKind`
  - `LoadKind` enum (`Libraries(Vec<String>)` or `Selective { symbols, library }`)
  - `LoadedLibraries` struct for ordered load statement collection
  - `parse_load_bits()` function to parse load tag bits
- Export new types from `crates/djls-semantic/src/lib.rs`

**Quality Checks:**
- [ ] `cargo build -p djls-semantic` passes
- [ ] `cargo clippy -p djls-semantic --all-targets -- -D warnings` passes
- [ ] `cargo test -p djls-semantic load_resolution` passes

### Phase 2: Compute LoadedLibraries from NodeList

**Status:** 🔲 Not Started

Add a tracked Salsa query that extracts `LoadedLibraries` from a parsed template.

**Changes:**
- Add `#[salsa::tracked] fn compute_loaded_libraries()` in `load_resolution.rs`
- Iterate through nodelist, find `Node::Tag { name: "load", ... }`
- Parse load bits and build `LoadedLibraries` in document order
- Export from `lib.rs`

**Quality Checks:**
- [ ] `cargo build -p djls-semantic` passes
- [ ] `cargo clippy -p djls-semantic --all-targets -- -D warnings` passes
- [ ] Integration test: Parse template with loads, verify extraction

### Phase 3: Available Symbols Query

**Status:** 🔲 Not Started

Add a query that combines inspector inventory with load state to determine what tags are available at a given position.

**Changes:**
- Add `AvailableSymbols` struct with `tags: FxHashSet<String>`
- Add `LoadState` struct with `fully_loaded` and `selective` HashMaps for state-machine approach
- Add `available_tags_at()` function using state-machine to process loads in order
- Handle selective import then full load correctly (full load clears selective)
- Add comprehensive unit tests for all scoping scenarios

**Quality Checks:**
- [ ] `cargo build -p djls-semantic` passes
- [ ] `cargo test -p djls-semantic` passes (all unit tests)
- [ ] `cargo clippy -p djls-semantic --all-targets -- -D warnings` passes

### Phase 4: Validation Integration - Unknown Tag Diagnostics

**Status:** 🔲 Not Started

Integrate load scoping into tag validation to produce diagnostics for unknown tags and unloaded library tags.

**Changes:**
- Add new error variants in `errors.rs`: `UnknownTag`, `UnloadedLibraryTag`, `AmbiguousUnloadedTag`
- Add diagnostic codes S108, S109, S110 in `diagnostics.rs`
- Add `inspector_inventory()` method to `Db` trait and implement in `DjangoDatabase`
- Add `validate_tag_scoping()` tracked function with collision handling
- Skip tags with structural specs (openers/closers/intermediates)
- Wire into `validate_nodelist()` in `lib.rs`

**Quality Checks:**
- [ ] `cargo build` passes
- [ ] `cargo test` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] Manual: Template with `{% trans %}` without `{% load i18n %}` → S109
- [ ] Manual: Add `{% load i18n %}` → no diagnostic
- [ ] Manual: Unknown tag `{% nonexistent %}` → S108

### Phase 5: Completions Integration

**Status:** 🔲 Not Started

Update completions to filter tags based on load state at cursor position.

**Changes:**
- Update `generate_tag_name_completions()` signature with `loaded_libraries` and `cursor_byte_offset`
- Filter completions by availability at cursor position
- Add `calculate_byte_offset()` helper to convert LSP Position → byte offset
- Update `handle_completion()` and `generate_template_completions()` signatures
- Update server call site in `server.rs` to pass loaded libraries
- When inspector unavailable (None), show all tags as fallback

**Quality Checks:**
- [ ] `cargo build` passes
- [ ] `cargo test` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] Manual: Without `{% load %}`, only builtins in completions
- [ ] Manual: After `{% load i18n %}`, i18n tags appear AFTER the load
- [ ] Manual: Cursor BEFORE `{% load i18n %}` → i18n tags NOT shown
- [ ] Manual: `{% load trans from i18n %}` → only `trans`, not other i18n tags

### Phase 6: Library Completions Enhancement

**Status:** 🔲 Not Started

Update `{% load %}` completions to show available libraries and handle completion behavior correctly.

**Changes:**
- Update `generate_library_completions()` to accept `loaded_libraries` and `cursor_byte_offset`
- Filter to show libraries NOT yet loaded (or deprioritize already-loaded)
- Add sort_text to deprioritize already-loaded libraries
- Mark already-loaded libraries as deprecated (strikethrough in editors)
- Update call site in `generate_template_completions()`

**Quality Checks:**
- [ ] `cargo build` passes
- [ ] `cargo test` passes
- [ ] Manual: `{% load %}` completions show all available libraries
- [ ] Manual: Already-loaded libraries are deprioritized/marked

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
