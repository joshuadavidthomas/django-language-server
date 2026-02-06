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

**Status:** complete
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

- [x] Add `PartialEq` derive to `Interpreter` (`crates/djls-project/src/python.rs`) if not already present, to support comparison in update methods.
- [x] Implement `update_project_from_settings(&mut self, settings: &Settings)` on `DjangoDatabase`: compare each field (`interpreter`, `django_settings_module`, `pythonpath`, `tagspecs`, `diagnostics`) against current `Project` values; only call setters when values differ. Track whether environment-related fields changed.
- [x] Make `TemplatetagsRequest`, `TemplatetagsResponse` public (or add a `TemplateTags::from_response()` constructor) so `refresh_inspector` can construct inventory without going through tracked queries.
- [x] Implement `refresh_inspector(&mut self)` on `DjangoDatabase`: query Python inspector directly (not through tracked functions), compare new inventory with `project.inspector_inventory(db)`, only call setter if changed.
- [x] Update `set_settings` to delegate to `update_project_from_settings` when a project exists, keeping project identity stable (no `Project::new` recreation).
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Make tag_specs a Tracked Query

- [x] Add `TagSpecs::from_config_def(tagspec_def: &TagSpecDef) -> TagSpecs` method on `TagSpecs` in `crates/djls-semantic/src/templatetags/specs.rs` — reuse existing `From<(TagDef, String)> for TagSpec` conversion logic. Starts with `django_builtin_specs()`, merges user specs from `TagSpecDef`.
- [x] Add `compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs` as a `#[salsa::tracked]` function in `crates/djls-server/src/db.rs`. Reads `project.tagspecs(db)` and `project.inspector_inventory(db)` to establish Salsa dependencies. Does NOT read `Arc<Mutex<Settings>>`.
- [x] Add `compute_tag_index(db: &dyn SemanticDb, project: Project) -> TagIndex` as a `#[salsa::tracked]` function depending on `compute_tag_specs`. Provides automatic invalidation cascade.
- [x] Update `SemanticDb` implementation on `DjangoDatabase`: `tag_specs()` delegates to `compute_tag_specs` when project exists, falls back to `django_builtin_specs()`. `tag_index()` delegates to `compute_tag_index`. `diagnostics_config()` reads from `project.diagnostics(db)`.
- [x] Remove `Arc<Mutex<Settings>>` reads from any tracked query path (the `settings` field may remain for `set_settings` / `update_project_from_settings` only, not for tracked queries).
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Invalidation Tests with Event Capture

- [x] Build test infrastructure in `crates/djls-server/src/db.rs` (in `#[cfg(test)]` module): `EventLog` that stores `salsa::Event` values, `was_executed(db, query_name)` helper using `db.ingredient_debug_name(database_key.ingredient_index())`.
- [x] Test: `tag_specs()` cached on repeated access — second call has no `WillExecute` event for `compute_tag_specs`.
- [x] Test: updating `project.tagspecs` via setter → `compute_tag_specs` re-executes.
- [x] Test: updating `project.inspector_inventory` via setter → `compute_tag_specs` re-executes.
- [x] Test: same value = no invalidation — manual comparison prevents setter call, cache preserved.
- [x] Test: tag index depends on tag specs — changing tagspecs causes both `compute_tag_specs` and `compute_tag_index` to re-execute.
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M3 — `{% load %}` Scoping Infrastructure

**Status:** in-progress
**Plan:** `.agents/plans/2026-02-05-m3-load-scoping.md`

**Goal:** Implement position-aware `{% load %}` scoping so diagnostics and completions respect which libraries are loaded at each position. Produces S108 (unknown tag), S109 (unloaded tag), S110 (ambiguous unloaded tag) diagnostics.

### Phase 1: Load Statement Parsing and Data Structures

- [x] Create `crates/djls-semantic/src/load_resolution.rs` module
- [x] Define `LoadStatement` struct: `span` (byte range), `kind` enum distinguishing full load (list of library names) vs selective import (`{% load X from Y %}` — symbols + library name)
- [x] Define `LoadedLibraries` struct: ordered collection of `LoadStatement` values with a method `available_at(position) -> LoadState` that filters loads ending before the query position, applying the state-machine semantics (fully_loaded set + selective imports map)
- [x] Implement `parse_load_bits(bits: &[String]) -> Option<LoadKind>` that parses tag bits from a `Node::Tag` with `name == "load"` — detects `from` keyword for selective imports vs full library loads
- [x] Export `load_resolution` module from `crates/djls-semantic/src/lib.rs`
- [x] Tests: full load (`{% load i18n %}`), multi-library load (`{% load i18n static %}`), selective import (`{% load trans from i18n %}`), multi-symbol selective (`{% load trans blocktrans from i18n %}`), empty/malformed load edge cases
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Compute LoadedLibraries from NodeList

- [x] Add `compute_loaded_libraries(db, nodelist) → LoadedLibraries` as a `#[salsa::tracked]` function that iterates all nodes in a nodelist, identifies `Node::Tag { name: "load" }`, parses each into a `LoadStatement`, returns ordered `LoadedLibraries`
- [x] Wire into the Salsa dependency graph so results are cached per file revision (tracked function on NodeList, which depends on File revision)
- [x] Tests: given a nodelist with load tags at various positions, verify `LoadedLibraries` is correctly constructed and position queries return expected results
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Available Symbols Query

- [x] Define `AvailableSymbols` type representing the set of tags available at a given position, plus a mapping of unavailable-but-known tags to their required library/libraries
- [x] Implement query logic: start with all builtin tags (always available), add tags from fully-loaded libraries (load span < position), add selectively-imported symbols, handle selective→full load ordering
- [x] Handle tag-name collision: track ALL candidate libraries for each tag name from inspector inventory
- [x] Tests: tag before load (unavailable), tag after load (available), selective imports, full load overriding selective, multiple libraries for same tag name → multiple candidates, builtins always available regardless of position
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Validation Integration — Unknown Tag Diagnostics

- [x] Add new error variants to diagnostic system: `S108` (UnknownTag), `S109` (UnloadedTag — requires specific library), `S110` (AmbiguousUnloadedTag — multiple candidate libraries)
- [x] Add diagnostic codes and messages for S108, S109, S110
- [x] Extend `SemanticDb` trait with `inspector_inventory()` accessor so validation can check inspector health
- [x] In tag validation, after checking TagSpecs (structural tags), check available symbols set — if tag not available, classify as S108/S109/S110 based on inspector knowledge
- [x] Guard: if `inspector_inventory` is `None`, skip all S108/S109/S110 diagnostics entirely
- [x] Structural tag exclusion: skip scoping checks for closers/intermediates (not openers — library openers like `trans` still need scoping)
- [x] Tests: unknown tag → S108, unloaded library tag → S109 with correct library name, tag in multiple libraries → S110, inspector unavailable → no scoping diagnostics, structural tags (endif, else) skip scoping checks, builtin tags always available, selective imports, tag before/after load
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 5: Completions Integration

- [ ] Update `generate_tag_name_completions` to accept `LoadedLibraries` and inspector inventory as parameters
- [ ] When inspector available: only show builtins + tags from loaded libraries at cursor position
- [ ] When inspector unavailable: show all tags (fallback, no filtering)
- [ ] Update call sites in the server to pass the new parameters
- [ ] Tests: before any load only builtins appear, after `{% load i18n %}` i18n tags appear, selective load only shows imported symbols, inspector unavailable shows all tags
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 6: Library Completions Enhancement

- [ ] Update `generate_library_completions` to check inspector availability — return empty when inspector unavailable
- [ ] Verify library completions behavior when inspector is healthy (already done in M1, confirm no regressions)
- [ ] Tests: library completions with healthy inspector show correct names, inspector unavailable returns empty list
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

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

- **`target/` tracked in worktree git**: Fixed — `.gitignore` now excludes `target/`.
- **M2 Phase 2 complete**: `set_settings` now delegates to `update_project_from_settings` + `refresh_inspector` when a project exists, keeping Salsa identity stable. No more `Project::new` recreation on config changes.
- **M2 Phase 3**: `TagSpecs` needed `PartialEq` derive for Salsa tracked function return type memoization. Refactored `From<&Settings> for TagSpecs` to delegate to new `TagSpecs::from_config_def`. Added `TagIndex::from_tag_specs` to build index from explicit specs without going through `db.tag_specs()` trait method.
- **M2 Phase 4**: Salsa's "backdate" optimization means `compute_tag_index` won't re-execute if `compute_tag_specs` returns the same value even after input changes. Tests must use `TagSpecDef` with actual tags to produce distinct `TagSpecs` output. Also, `Interpreter::discover(None)` reads real `$VIRTUAL_ENV` in non-test crates — test projects must match by using `Interpreter::discover()` rather than hardcoding `Auto`.
- **M3 Phase 2**: `Node::Tag.bits` does NOT include the tag name — the parser separates `name` and `bits`. So for `{% load i18n %}`, `name == "load"` and `bits == ["i18n"]`. Fixed `parse_load_bits` to accept argument-only bits (no "load" prefix).
