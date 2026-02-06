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

**Status:** in-progress
**Plan:** `.agents/plans/2026-02-05-m2-salsa-invalidation-plumbing.md`

**Goal:** Eliminate stale template diagnostics by making external data sources explicit Salsa-visible fields within the existing `Project` input. Maintain exactly 2 Salsa inputs (`File` + `Project`).

### Phase 1: Extend Project Input with djls-conf Types

- [x] Derive `PartialEq` on `DiagnosticsConfig` (it already has it — verify). Verify `TagSpecDef` has `PartialEq` (it already derives it — confirm no blockers). Do NOT add `Eq` — `TagSpecDef` contains `serde_json::Value` in `extra` fields which lacks `Eq`.
- [x] Add `Eq` to `DiagnosticsConfig` if not present (its `HashMap<String, DiagnosticSeverity>` supports `Eq`).
- [x] Add three new fields to `Project` (`#[salsa::input]` in `crates/djls-project/src/project.rs`): `inspector_inventory: Option<TemplateTags>` (with `#[returns(ref)]`), `tagspecs: TagSpecDef` (with `#[returns(ref)]`), `diagnostics: DiagnosticsConfig` (with `#[returns(ref)]`).
- [x] Add `djls-conf` dependency to `djls-project/Cargo.toml` if not already present (check — it's already there for `Settings`).
- [x] Update `Project::bootstrap` to accept `&Settings` and initialize the three new fields: `inspector_inventory` as `None`, `tagspecs` from `settings.tagspecs().clone()`, `diagnostics` from `settings.diagnostics().clone()`.
- [x] Update all call sites of `Project::new` and `Project::bootstrap` in `crates/djls-server/src/db.rs` to pass the new fields.
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Add Project Update APIs with Manual Comparison

- [ ] Add `PartialEq` derive to `Interpreter` (`crates/djls-project/src/python.rs`) if not already present, to support comparison in update methods.
- [ ] Implement `update_project_from_settings(&mut self, settings: &Settings)` on `DjangoDatabase`: compare each field (`interpreter`, `django_settings_module`, `pythonpath`, `tagspecs`, `diagnostics`) against current `Project` values; only call setters when values differ. Track whether environment-related fields changed.
- [ ] Make `TemplatetagsRequest`, `TemplatetagsResponse` public (or add a `TemplateTags::from_response()` constructor) so `refresh_inspector` can construct inventory without going through tracked queries.
- [ ] Implement `refresh_inspector(&mut self)` on `DjangoDatabase`: query Python inspector directly (not through tracked functions), compare new inventory with `project.inspector_inventory(db)`, only call setter if changed.
- [ ] Update `set_settings` to delegate to `update_project_from_settings` when a project exists, keeping project identity stable (no `Project::new` recreation).
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Make tag_specs a Tracked Query

- [ ] Add `TagSpecs::from_config_def(tagspec_def: &TagSpecDef) -> TagSpecs` method on `TagSpecs` in `crates/djls-semantic/src/templatetags/specs.rs` — reuse existing `From<(TagDef, String)> for TagSpec` conversion logic. Starts with `django_builtin_specs()`, merges user specs from `TagSpecDef`.
- [ ] Add `compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs` as a `#[salsa::tracked]` function in `crates/djls-server/src/db.rs`. Reads `project.tagspecs(db)` and `project.inspector_inventory(db)` to establish Salsa dependencies. Does NOT read `Arc<Mutex<Settings>>`.
- [ ] Add `compute_tag_index(db: &dyn SemanticDb, project: Project) -> TagIndex` as a `#[salsa::tracked]` function depending on `compute_tag_specs`. Provides automatic invalidation cascade.
- [ ] Update `SemanticDb` implementation on `DjangoDatabase`: `tag_specs()` delegates to `compute_tag_specs` when project exists, falls back to `django_builtin_specs()`. `tag_index()` delegates to `compute_tag_index`. `diagnostics_config()` reads from `project.diagnostics(db)`.
- [ ] Remove `Arc<Mutex<Settings>>` reads from any tracked query path (the `settings` field may remain for `set_settings` / `update_project_from_settings` only, not for tracked queries).
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Invalidation Tests with Event Capture

- [ ] Build test infrastructure in `crates/djls-server/src/db.rs` (in `#[cfg(test)]` module): `EventLogger` that stores `salsa::Event` values, `was_executed(db, query_name)` helper using `db.ingredient_debug_name(database_key.ingredient_index())`.
- [ ] Test: `tag_specs()` cached on repeated access — second call has no `WillExecute` event for `compute_tag_specs`.
- [ ] Test: updating `project.tagspecs` via setter → `compute_tag_specs` re-executes.
- [ ] Test: updating `project.inspector_inventory` via setter → `compute_tag_specs` re-executes.
- [ ] Test: same value = no invalidation — manual comparison prevents setter call, cache preserved.
- [ ] Test: tag index depends on tag specs — changing tagspecs causes both `compute_tag_specs` and `compute_tag_index` to re-execute.
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

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
