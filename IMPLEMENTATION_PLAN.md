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

**Status:** complete
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

- [x] Update `generate_tag_name_completions` to accept `LoadedLibraries` and inspector inventory as parameters
- [x] When inspector available: only show builtins + tags from loaded libraries at cursor position
- [x] When inspector unavailable: show all tags (fallback, no filtering)
- [x] Update call sites in the server to pass the new parameters
- [x] Tests: before any load only builtins appear, after `{% load i18n %}` i18n tags appear, selective load only shows imported symbols, inspector unavailable shows all tags
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 6: Library Completions Enhancement

- [x] Update `generate_library_completions` to check inspector availability — return empty when inspector unavailable
- [x] Verify library completions behavior when inspector is healthy (already done in M1, confirm no regressions)
- [x] Tests: library completions with healthy inspector show correct names, inspector unavailable returns empty list
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M4 — Filters Pipeline

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m4-filters-pipeline.md`

**Goal:** Implement complete filter support: inspector collection, structured parser representation, completions in `{{ x| }}` context, and unknown/unloaded filter diagnostics (S111/S112/S113) with load scoping.

### Phase 1: Inspector Filter Inventory

- [x] Add `TemplateFilter` dataclass to `queries.py` with same shape as `TemplateTag`: `name`, `provenance` (externally-tagged dict), `defining_module`, `doc`
- [x] Update `TemplateTagQueryData` to include `templatefilters: list[TemplateFilter]` field
- [x] In `get_installed_templatetags()`, iterate `library.filters.items()` for builtins (alongside `library.tags.items()`) and append `TemplateFilter` with `Builtin` provenance
- [x] In `get_installed_templatetags()`, iterate `library.filters.items()` for library entries and append `TemplateFilter` with `Library` provenance
- [x] Add Rust `TemplateFilter` struct in `crates/djls-project/src/django.rs` mirroring `TemplateTag` but for filters (same `TagProvenance`, same accessors: `name()`, `provenance()`, `defining_module()`, `is_builtin()`, `library_load_name()`)
- [x] Expand `TemplatetagsResponse` with `templatefilters: Vec<TemplateFilter>` field
- [x] Expand `TemplateTags` with `filters: Vec<TemplateFilter>` field and add `filters()` accessor returning `&[TemplateFilter]`
- [x] Update `TemplateTags::new()`, `TemplateTags::from_response()`, and the `templatetags` tracked query to pass through filters
- [x] Export `TemplateFilter` from `crates/djls-project/src/lib.rs`
- [x] Add unit tests: `TemplateFilter` deserialization, accessor methods, `TemplateTags` with filters round-trip
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Structured Filter Representation in Parser

- [x] Define `Filter` struct in `crates/djls-templates/src/nodelist.rs` (alongside `Node` enum) with `name: String`, `arg: Option<String>`, `span: Span` — handles simple filters (`title`), filters with args (`default:'nothing'`), colon inside quoted args (`default:'time:12:30'`)
- [x] Implement `parse_filter(raw: &str, base_offset: u32) -> Filter` helper in `parser.rs` that splits name from argument at first unquoted colon
- [x] Update `Node::Variable` in `nodelist.rs` from `filters: Vec<String>` to `filters: Vec<Filter>`
- [x] Update `parse_variable()` in `parser.rs` (~line 182) to produce `Vec<Filter>` with correct per-filter spans (each filter span is relative to the variable expression)
- [x] Update `TestNode::Variable` in `parser.rs` test module to use `Vec<Filter>` or a simplified representation for snapshot compatibility
- [x] Update `NodeView::Variable` in `crates/djls-semantic/src/blocks/tree.rs` (~line 332) to use `Vec<Filter>`
- [x] `blocks/builder.rs` line 434 uses `Node::Variable { span, .. }` — no change needed (ignores filters via `..`)
- [x] Update `OffsetContext::Variable` in `crates/djls-ide/src/context.rs` (line 23) — has `filters: Vec<String>`, needs `Vec<Filter>`
- [x] Run `INSTA_UPDATE=1 cargo test -q` then `cargo insta review` — affected snapshots: `parse_django_variable_with_filter.snap`, `parse_filter_chains.snap`, `parse_mixed_content.snap`, `parse_full.snap`
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Filter Completions

- [x] Update `analyze_template_context()` in `crates/djls-ide/src/completions.rs` to detect `{{ var|` context — cursor after pipe inside variable expression, extract partial filter name
- [x] Implement `generate_filter_completions()` that shows builtin filters always + library filters only if their library is loaded at cursor position (reuse M3 `LoadedLibraries`)
- [x] When inspector unavailable, show all known filters as fallback (consistent with tag completion behavior)
- [x] Wire `TemplateCompletionContext::Filter { partial }` case to call `generate_filter_completions()`
- [x] Sort results alphabetically for deterministic ordering
- [x] Tests: `{{ value|` context detected, partial prefix filtering (`{{ value|def`), builtins always appear, library filters excluded when not loaded, inspector unavailable shows all, selective import only shows imported filter symbols
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Filter Validation with Load Scoping

- [x] Add `FilterAvailability` enum (or reuse existing `TagAvailability` pattern) in load_resolution symbols module
- [x] Extend `AvailableSymbols` (or create `AvailableFilterSymbols`) to track filter availability using the same `LoadedLibraries` + inspector inventory pattern as tags
- [x] Add diagnostic codes S111 (`UnknownFilter`), S112 (`UnloadedFilter`), S113 (`AmbiguousUnloadedFilter`) to the diagnostic system
- [x] Wire filter validation into semantic analysis: for each `Filter` in `Node::Variable`, check availability via load scoping
- [x] Guard: skip all filter scoping diagnostics when `inspector_inventory` is `None`
- [x] Tests: unknown filter → S111, unloaded library filter → S112 with library name, filter in multiple libraries → S113, filter after `{% load %}` → valid, builtin filter → always valid, inspector unavailable → no diagnostics
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M5 — Extraction Engine (`djls-extraction`)

**Status:** in-progress
**Plan:** `.agents/plans/2026-02-05-m5-extraction-engine.md`

**Goal:** Implement Rust-side rule mining using Ruff AST to derive validation semantics (argument counts, block structure, option constraints) from Python template tag/filter registration modules. Enriches inspector inventory for M6 rule evaluation.

**Key principles:** Python inspector = authoritative inventory; Rust = AST extraction; Salsa inputs stay minimal (`File` + `Project` only); extraction keyed by `SymbolKey` to avoid cross-library collisions.

### Phase 1: Create `djls-extraction` Crate with Ruff Parser

- [x] Create `crates/djls-extraction/` directory with `Cargo.toml`, `src/lib.rs`
- [x] Add `ruff_python_parser` and `ruff_python_ast` as workspace-level git dependencies in root `Cargo.toml` (pin to specific SHA from a stable Ruff release, e.g., v0.9.x tag)
- [x] Add a Cargo feature gate `parser` in `djls-extraction/Cargo.toml` so downstream crates can depend on types-only without pulling in Ruff parser transitively
- [x] Define core types: `SymbolKey { registration_module: String, name: String, kind: SymbolKind }`, `SymbolKind` enum (`Tag`/`Filter`), `ExtractionResult` (map from `SymbolKey` to extracted rules), `TagRule` (argument validation), `FilterArity` (arg count info), `BlockTagSpec` (end_tag, intermediates, opaque)
- [x] Stub the public API: `extract_rules(source: &str) -> ExtractionResult` (behind `parser` feature)
- [x] Add `djls-extraction` to workspace members in root `Cargo.toml`
- [x] Write a smoke test: parse a trivial Python file with `ruff_python_parser` and verify no panics
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Registration Discovery

- [x] Implement AST walker to find `@register.tag` / `@register.simple_tag` / `@register.inclusion_tag` / `@register.filter` decorators
- [x] Handle `register.tag("name", func)` call expression registration style
- [x] Extract registration name: from decorator keyword arg `name=`, explicit string positional arg, or decorated function name (in that priority order)
- [x] Build `RegistrationInfo` struct: `name`, `kind` (tag/simple_tag/inclusion_tag/filter), reference to the decorated/called function AST node
- [x] Implement `collect_registrations(source: &str) -> Vec<RegistrationInfo>`
- [x] Tests: decorator-based tag, simple_tag with `name=` kwarg, inclusion_tag, filter, call-style `register.tag("name", func)`, function name fallback, multiple registrations in one file
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Function Context Detection

- [x] Implement `detect_split_var(func_body: &[Stmt]) -> Option<String>` that scans function body for `token.split_contents()` (or `parser.token.split_contents()` via `parser` parameter) call and returns the variable name it's bound to
- [x] Handle common patterns: `bits = token.split_contents()`, `args = token.split_contents()`, tuple unpacking `tag_name, *args = token.split_contents()`
- [x] Handle indirect access: function parameter `parser` → `parser.token.split_contents()` → same detection
- [x] Return `None` if no `split_contents()` call found (function doesn't use token-splitting)
- [x] Tests: `bits = token.split_contents()`, `args = token.split_contents()`, `parts = token.split_contents()`, tuple unpacking, no split_contents → None, split_contents via parser.token
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Rule Extraction

- [x] Implement `RuleExtractor` that walks function body looking for `raise TemplateSyntaxError(...)` statements and extracts guard conditions
- [x] Extract token count checks: `if len(bits) < N`, `if len(bits) > N`, `if len(bits) != N`, `if len(bits) not in (...)` → `ArgumentCountConstraint`
- [x] Extract keyword position checks: `if bits[N] != "keyword"` → `RequiredKeyword { position, value }`
- [x] Extract option validation: while loops checking known option sets, duplicate detection → `KnownOptions { values, allow_duplicates }`
- [x] Handle `simple_tag`/`inclusion_tag` `takes_context` and `func` parameter analysis (from `parse_bits` signatures)
- [x] Use dynamically-detected split variable name (from Phase 3) for all comparisons — NOT hardcoded `bits`
- [x] Represent results as structured `TagRule { arg_constraints, required_keywords, known_options }`
- [x] Tests: len check patterns, keyword position patterns, option loops, simple_tag params, non-`bits` variable names, multiple raise statements in one function
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 5: Block Spec Extraction (Control-Flow Based)

- [x] Implement `extract_block_spec(func_body: &[Stmt]) -> Option<BlockTagSpec>` that finds `parser.parse((...))` calls with tuple arguments containing stop-token strings
- [x] Determine end-tag vs intermediate: if a stop-token leads to another `parser.parse()` call → intermediate; if it leads to return/node construction → terminal (end-tag)
- [x] Handle dynamic end-tag patterns like `f"end{tag_name}"` (best-effort extraction)
- [x] Detect opaque blocks: `parser.skip_past(...)` patterns → content should not be parsed
- [x] **Non-negotiable**: infer closers from control flow only — NEVER from `end*` string prefix matching
- [x] **Non-negotiable**: return `None` for `end_tag` when inference is ambiguous (multiple candidates, unclear control flow)
- [x] **Tie-breaker only**: `end{tag_name}` Django convention used ONLY to select among candidates already found via control flow, never invented from thin air
- [x] Tests: simple end-tag (`{% for %}...{% endfor %}`), intermediates (`{% if %}...{% else %}...{% endif %}`), opaque block (verbatim-like), non-conventional closer names found correctly, ambiguous → None, dynamic `f"end{name}"`, multiple parser.parse() chains
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 6: Filter Arity Extraction

- [x] Implement `extract_filter_arity(func_def: &StmtFunctionDef) -> FilterArity` that inspects function signature
- [x] Determine required arg count (exclude `self` and the value parameter — first positional after `self` if method, or first positional if function)
- [x] Detect optional arguments (has default value) → `FilterArity { expects_arg: bool, arg_optional: bool }`
- [x] Handle `@stringfilter` and `@register.filter(is_safe=True)` decorator kwargs (informational, not arity-changing)
- [x] Tests: no-arg filter (`{{ value|title }}`), required-arg filter (`{{ value|default:"nothing" }}`), optional-arg filter, method-style filters
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 7: Salsa Integration

- [ ] Add `djls-extraction` dependency to `djls-server/Cargo.toml` with `parser` feature enabled
- [ ] Add `djls-extraction` dependency (types-only, no `parser` feature) to `djls-project/Cargo.toml` if needed for `ExtractionResult` storage
- [ ] Create tracked query `extract_module_rules(db, file: File) -> ExtractionResult` in `djls-server/src/db.rs` for workspace files
- [ ] For external modules (site-packages): extract during `refresh_inspector()`, store on `Project.extracted_external_rules: Option<ExtractionResult>` field (new `#[returns(ref)]` field)
- [ ] Implement module path → file path resolver using `sys_path` from Python environment; classify as workspace vs external based on project root
- [ ] Update `compute_tag_specs` to merge extracted rules into tag specs — extraction enriches/overrides `builtins.rs` defaults
- [ ] Ensure workspace extraction → tracked queries → automatic Salsa invalidation; external extraction → Project field → manual refresh invalidation
- [ ] Tests: verify extraction result cached on second access, file edit triggers re-extraction, external rules stored on Project field, merged tag specs include extracted block specs
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 8: Small Fixture Golden Tests

- [ ] Create inline Python source fixtures for registration discovery: all decorator styles, call-based registration, name keyword arg
- [ ] Create inline fixtures for rule extraction: len checks, keyword position checks, option loop patterns
- [ ] Create inline fixtures for block spec extraction: simple end-tag, intermediates, opaque blocks, ambiguous → None
- [ ] Create inline fixtures for filter arity: no-arg, required-arg, optional-arg
- [ ] Create inline fixtures for edge cases: no split_contents call, dynamic end-tags, multiple registrations
- [ ] Use `insta` for snapshot testing where appropriate (no standalone test files — tests in `#[cfg(test)]` modules)
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 9: Corpus / Full-Source Extraction Tests

- [ ] Create test infrastructure that can point at a synced corpus directory (from `template_linter/corpus/`)
- [ ] Run extraction against all `templatetags/**/*.py` files in corpus
- [ ] Verify: no panics across entire corpus, meaningful extraction yield (tag/filter counts)
- [ ] Add golden snapshots for key Django modules (e.g., `defaulttags.py`, `i18n.py`, `static.py`)
- [ ] Gate tests on corpus availability (auto-detect default location `../../template_linter/corpus/`, skip gracefully if not present)
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

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
- **M4 Phase 2**: `Node::Variable.filters` changed from `Vec<String>` to `Vec<Filter>`. `split_variable_expression()` handles quote-aware pipe splitting. `parse_filter()` splits name from arg at first unquoted colon. Affected: `nodelist.rs`, `parser.rs`, `blocks/tree.rs` (NodeView), `context.rs` (OffsetContext), 4 snapshots. `blocks/builder.rs` unaffected (uses `..` wildcard).
- **M5 Phase 1**: Ruff 0.15.0 (SHA `0dfa810e9aad9a465596768b0211c31dd41d3e73`) used for `ruff_python_parser` and `ruff_python_ast`. API: `ruff_python_parser::parse_module(source)` returns `Result<Parsed<ModModule>, ParseError>`. Use `.into_syntax()` on parsed result to get the `ModModule` AST. Feature gate `parser` keeps ruff deps optional for types-only consumers.
- **M5 Phase 4**: Ruff's `Parameters` struct does NOT have a `defaults` field like Python's `ast.arguments`. Instead, defaults are per-parameter: `ParameterWithDefault { parameter, default: Option<Box<Expr>> }`. Also `StmtWhile.test` is `Box<Expr>` so dereference with `&*while_stmt.test` when pattern matching. `extract_tag_rule()` dispatches to `extract_compile_function_rule()` for `@register.tag` (uses split_contents guards) vs `extract_parse_bits_rule()` for `@register.simple_tag` / `@register.inclusion_tag` (uses function signature analysis).
- **M5 Phase 5**: Block spec extraction in `blocks.rs`. Classification strategy: (1) Collect all `parser.parse((...))` stop-tokens, (2) Use control flow (if/elif/else/while branches) to classify — tokens leading to another `parser.parse()` → intermediate, others → end-tag. (3) Tokens not classified as intermediate default to end-tag candidates. (4) `end*` convention used ONLY as tie-breaker for single-call multi-token ambiguity. Also handles `parser.skip_past("endverbatim")` → opaque block, and `parser.parse((f"end{tag_name}",))` → dynamic end-tag (returns `end_tag: None`). Ruff's `FStringValue` uses `.iter()` not `.parts()` to iterate over `FStringPart` values. `ExceptHandler::ExceptHandler` is irrefutable — use `let` not `if let`. `startswith` pattern (`while token.contents.startswith("elif"):`) needed dedicated detection separate from `==` comparison detection.
