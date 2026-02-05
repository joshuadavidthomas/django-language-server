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

**Status:** in-progress
**Plan:** `.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`

### Phase 1: Extend Project Input with djls-conf Types

- [x] Verify `PartialEq` is derived on `TagSpecDef`, `TagLibraryDef`, `TagDef`, `EndTagDef`, `IntermediateTagDef`, `TagArgDef` in `crates/djls-conf/src/tagspecs.rs` (already done — confirm, do NOT add `Eq` since `serde_json::Value` in `extra` prevents it)
- [x] Verify `PartialEq` + `Eq` on `DiagnosticsConfig` in `crates/djls-conf/src/diagnostics.rs` (already `PartialEq` — add `Eq` if not present)
- [x] Add three new fields to `Project` salsa input in `crates/djls-project/src/project.rs`: `inspector_inventory: Option<TemplateTags>`, `tagspecs: TagSpecDef`, `diagnostics: DiagnosticsConfig` (all with `#[returns(ref)]`)
- [x] Update `Project::bootstrap()` signature to accept `settings: &djls_conf::Settings` and pass new fields to `Project::new()`: `None` for inventory, `settings.tagspecs().clone()`, `settings.diagnostics().clone()`
- [x] Update `Project::initialize()` if needed (may no longer need to call `templatetags()` eagerly since inventory comes via `refresh_inspector`)
- [x] Update all call sites of `Project::new()` and `Project::bootstrap()` to pass the new arguments (search crates for calls)
- [x] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

### Phase 2: Add `TagSpecs::from_config_def` and Tracked Queries

- [x] Add `TagSpecs::from_config_def(def: &TagSpecDef) -> Self` in `crates/djls-semantic/src/templatetags/specs.rs` — extracts the conversion logic from `impl From<&Settings> for TagSpecs` to avoid duplication
- [x] Add `#[salsa::tracked] fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs` in `crates/djls-server/src/db.rs` — reads `project.tagspecs(db)` and `project.inspector_inventory(db)`, starts with `django_builtin_specs()`, merges user specs
- [x] Add `#[salsa::tracked] fn compute_tag_index(db: &dyn SemanticDb, project: Project) -> TagIndex<'_>` in `crates/djls-server/src/db.rs` — depends on `compute_tag_specs`
- [x] Update `SemanticDb` impl for `DjangoDatabase`: `tag_specs()` delegates to `compute_tag_specs`, `tag_index()` delegates to `compute_tag_index`, `diagnostics_config()` reads from `project.diagnostics(db)` — NO `Arc<Mutex<Settings>>` reads in any of these
- [x] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

### Phase 3: Project Update APIs with Manual Comparison

- [ ] Add `update_project_from_settings(&mut self, project: Project, settings: &Settings)` method on `DjangoDatabase` — compares each field before calling setters (Ruff/RA pattern to avoid spurious invalidation)
- [ ] Add `refresh_inspector(&mut self)` method on `DjangoDatabase` — queries Python inspector directly, compares result with `project.inspector_inventory(db)`, only calls setter if changed
- [ ] Rewrite `set_project()` to use `Project::bootstrap` with the new signature and call `refresh_inspector()` after creation
- [ ] Rewrite `set_settings()` to delegate field updates to `update_project_from_settings()` and trigger `refresh_inspector()` only when environment fields change
- [ ] Make `TemplatetagsRequest`, `TemplatetagsResponse` public in `crates/djls-project/src/django.rs` and export from `crates/djls-project/src/lib.rs` (needed for direct inspector queries)
- [ ] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

### Phase 4: Invalidation Tests with Event Capture

- [ ] Add `EventLogger` test infrastructure in `crates/djls-server/src/db.rs` `#[cfg(test)]` module — stores raw `salsa::Event` values, provides `was_executed(db, query_name)` helper using `db.ingredient_debug_name()`
- [ ] Add `TestDatabase` helper struct with `with_project()` constructor that wires up `EventLogger` to Salsa storage
- [ ] Test: `tag_specs_cached_on_repeated_access` — first call executes `compute_tag_specs`, second call uses cache
- [ ] Test: `tagspecs_change_invalidates` — modifying `project.tagspecs` via setter causes recomputation
- [ ] Test: `inspector_inventory_change_invalidates` — setting `project.inspector_inventory` causes recomputation
- [ ] Test: `same_value_no_invalidation` — comparing before setting prevents spurious invalidation
- [ ] Test: `tag_index_depends_on_tag_specs` — changing tagspecs recomputes tag_index too
- [ ] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

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
- M2: Salsa `#[salsa::tracked]` functions require `&dyn Trait` parameters, not concrete types. Used `&dyn SemanticDb` for `compute_tag_specs`/`compute_tag_index`.
- M2: `TagSpecs` needed `PartialEq` derive for use as Salsa tracked return value (Salsa requires equality comparison for memoization).
