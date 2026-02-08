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

**Status:** complete
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

- [x] Add `djls-extraction` dependency to `djls-server/Cargo.toml` with `parser` feature enabled
- [x] Add `djls-extraction` dependency (types-only, no `parser` feature) to `djls-project/Cargo.toml` if needed for `ExtractionResult` storage
- [x] Create tracked query `extract_module_rules(db, file: File) -> ExtractionResult` in `djls-server/src/db.rs` for workspace files
- [x] For external modules (site-packages): extract during `refresh_inspector()`, store on `Project.extracted_external_rules: Option<ExtractionResult>` field (new `#[returns(ref)]` field)
- [x] Implement module path → file path resolver using `sys_path` from Python environment; classify as workspace vs external based on project root
- [x] Update `compute_tag_specs` to merge extracted rules into tag specs — extraction enriches/overrides `builtins.rs` defaults
- [x] Ensure workspace extraction → tracked queries → automatic Salsa invalidation; external extraction → Project field → manual refresh invalidation
- [x] Tests: verify extraction result cached on second access, file edit triggers re-extraction, external rules stored on Project field, merged tag specs include extracted block specs
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 8: Small Fixture Golden Tests

- [x] Create inline Python source fixtures for registration discovery: all decorator styles, call-based registration, name keyword arg
- [x] Create inline fixtures for rule extraction: len checks, keyword position checks, option loop patterns
- [x] Create inline fixtures for block spec extraction: simple end-tag, intermediates, opaque blocks, ambiguous → None
- [x] Create inline fixtures for filter arity: no-arg, required-arg, optional-arg
- [x] Create inline fixtures for edge cases: no split_contents call, dynamic end-tags, multiple registrations
- [x] Use `insta` for snapshot testing where appropriate (no standalone test files — tests in `#[cfg(test)]` modules)
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 9: Corpus / Full-Source Extraction Tests

- [x] Create test infrastructure that can point at a synced corpus directory (from `template_linter/corpus/`)
- [x] Run extraction against all `templatetags/**/*.py` files in corpus
- [x] Verify: no panics across entire corpus, meaningful extraction yield (tag/filter counts)
- [x] Add golden snapshots for key Django modules (e.g., `defaulttags.py`, `i18n.py`, `static.py`)
- [x] Gate tests on corpus availability (auto-detect default location `../../template_linter/corpus/`, skip gracefully if not present)
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M6 — Rule Evaluation + Expression Validation

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m6-rule-evaluation.md`

**Goal:** Apply M5's extracted validation rules: expression validation for `{% if %}`/`{% elif %}` (S114), filter arity validation (S115/S116), and opaque region handling to skip validation inside `{% verbatim %}` etc.

### Phase 1: Opaque Region Infrastructure

- [x] Add `opaque: bool` field to `TagSpec` in `crates/djls-semantic/src/templatetags/specs.rs`
- [x] Update `merge_extraction_results` to propagate `opaque` from `BlockTagSpec` to `TagSpec`
- [x] Update `From<(TagDef, String)> for TagSpec` to set `opaque: false` (config-defined tags are never opaque)
- [x] Update `django_builtin_specs()` to set `opaque: true` for `verbatim` and `comment` tags
- [x] Create `OpaqueRegions` type (sorted list of byte spans) with `is_opaque(position: u32) -> bool` method in `crates/djls-semantic/src/`
- [x] Implement `compute_opaque_regions(db, nodelist) -> OpaqueRegions`: walk block tree, find tags with `tag_spec.opaque == true`, record inner content spans
- [x] Wire `OpaqueRegions` check into `validate_nodelist` — skip argument and scoping validation for nodes inside opaque regions
- [x] Tests: verbatim block content skipped, comment block content skipped, non-opaque blocks validated normally, nested content after opaque block still validated
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Expression Parser (Pratt Parser for `{% if %}`)

- [x] Create `crates/djls-semantic/src/if_expression.rs` module with Pratt parser ported from Python prototype (`template_linter/src/template_linter/template_syntax/if_expression.py`)
- [x] Implement tokenizer: split expression into operator tokens (`and`, `or`, `not`, `in`, `not in`, `is`, `is not`, `==`, `!=`, `<`, `>`, `<=`, `>=`) and operands (variables, literals — treated opaquely)
- [x] Implement Pratt parser with operator precedence: `or` < `and` < `not` (unary) < comparison (`in`, `not in`, `is`, `is not`, `==`, `!=`, `<`, `>`, `<=`, `>=`)
- [x] Detect expression syntax errors: operator in operand position, missing right operand, missing operator between operands, dangling unary operator, incomplete membership test (`not` without `in`)
- [x] Add S114 diagnostic code (`ExpressionSyntaxError`) to diagnostic system
- [x] Implement `validate_if_expressions(db, nodelist)`: for each `{% if %}` and `{% elif %}` tag, extract expression from bits and run parser; emit S114 on syntax error
- [x] Skip validation for nodes inside opaque regions (use `OpaqueRegions` from Phase 1)
- [x] Wire `validate_if_expressions` into `validate_nodelist` in `crates/djls-semantic/src/lib.rs`
- [x] Tests: valid expressions (all operator types, complex nesting), invalid expressions (`{% if and x %}`, `{% if x == %}`, `{% if x y %}`, `{% if not %}`), `{% elif %}` validated too, opaque region skipping
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Filter Arity Validation

- [x] Create `FilterAritySpecs` type (map from filter name → `FilterArity`) in `crates/djls-semantic/src/`
- [x] Implement `compute_filter_arity_specs(db, project) -> FilterAritySpecs` tracked query: merge extraction results' `filter_arities` map, resolve builtin filters with "last wins" semantics
- [x] Add `filter_arity_specs()` accessor to `SemanticDb` trait and implement on `DjangoDatabase`
- [x] Add S115 (`FilterMissingArgument`) and S116 (`FilterUnexpectedArgument`) diagnostic codes
- [x] Implement `validate_filter_arity(db, nodelist)`: for each `Node::Variable` with filters, look up each filter's arity spec, compare against actual usage (has arg vs no arg)
- [x] Use load scoping to determine which library a filter comes from → key into extraction results via `SymbolKey`
- [x] Skip validation inside opaque regions, skip when inspector inventory unavailable
- [x] Wire `validate_filter_arity` into `validate_nodelist`
- [x] Update all test databases implementing `SemanticDb` to include `filter_arity_specs()` method
- [x] Tests: filter with required arg missing → S115, filter with unexpected arg → S116, optional arg (both ways) → no error, builtin filter "last wins" resolution, opaque region skipping, inspector unavailable → no diagnostics
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Integration Tests

- [x] Create integration test: template with mixed expression errors, filter arity errors, and opaque regions — verify correct diagnostics emitted
- [x] Snapshot tests for diagnostic output on representative templates
- [x] Corpus coverage test (if corpus available): run validation on Django admin templates, verify no false positives for expression validation
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M7 — Documentation + Issue Reporting

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m7-docs-and-issue-template.md`

**Goal:** Document the new validation system (S108–S116), explain the inspector + extraction architecture for users, create a structured GitHub issue template for validation mismatches, and update existing docs with links to new content.

### Phase 1: Create Template Validation Documentation Page

- [x] Read existing `docs/configuration/index.md` to understand current diagnostic code documentation format
- [x] Read existing `docs/configuration/tagspecs.md` to understand current tagspec documentation
- [x] Read `.mkdocs.yml` to understand navigation structure
- [x] Create `docs/template-validation.md` covering: how validation works (inspector + extraction), what djls validates (unknown tags/filters, unloaded library tags/filters, block structure, if-expression syntax, filter arity), what djls cannot validate (runtime behavior, variable resolution, template inheritance), inspector availability behavior, ambiguous symbols, link to issue template
- [x] Update `.mkdocs.yml` navigation to include the new page
- [x] Verify: docs structure is consistent, internal links resolve

### Phase 2: Update Diagnostic Codes Documentation

- [x] Add S108–S110 (Tag Scoping) section to `docs/configuration/index.md`: UnknownTag, UnloadedTag, AmbiguousUnloadedTag
- [x] Add S111–S113 (Filter Scoping) section: UnknownFilter, UnloadedFilter, AmbiguousUnloadedFilter
- [x] Add S114–S116 (Expression & Filter Arity) section: ExpressionSyntaxError, FilterMissingArgument, FilterUnexpectedArgument
- [x] Add link to the new `docs/template-validation.md` page for more context
- [x] Verify: diagnostic code descriptions match actual implementation behavior

### Phase 3: Create GitHub Issue Template for Validation Mismatches

- [x] Create `.github/ISSUE_TEMPLATE/config.yml` linking to documentation
- [x] Create `.github/ISSUE_TEMPLATE/template-validation-mismatch.yml` issue form requiring: djls version, Django version, minimal template snippet, relevant `{% load %}` statements, expected vs actual behavior, djls.toml excerpt, inspector status
- [x] Verify: YAML is valid syntax

### Phase 4: Update TagSpecs Documentation

- [x] Update `docs/configuration/tagspecs.md` to replace generic "open an issue" text with link to the new issue template
- [x] Add cross-reference from tagspecs page to the template validation page
- [x] Verify: all internal links resolve

### Phase 5: Final Validation

- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q` (no code changes, but confirm nothing broke)
- [x] Review all new/updated docs for accuracy and consistency

---

## M8 — Extracted Rule Evaluation

**Status:** complete
**Plan:** `.agents/plans/2026-02-06-m8-extracted-rule-evaluation.md`
**Depends on:** M5, M6

**Goal:** Replace the old hand-crafted `args` validation system with extraction-derived rule evaluation. `TagRule` conditions (from `ExtractionResult.tag_rules`) become the primary validation path for argument checking. Remove hand-crafted `args` from `builtins.rs`, wire extraction-derived argument structure into completions/snippets, and validate against real-world template corpora with zero false positives.

**Key mapping (plan terminology → actual code):**
- Plan says `ExtractedRule` → code has `TagRule` (in `djls-extraction/src/types.rs`)
- Plan says `RuleCondition` → code has `ArgumentCountConstraint`, `RequiredKeyword`, `KnownOptions`
- Plan says `TagSpec.extracted_rules` → no such field exists yet; `TagRule`s live in `ExtractionResult.tag_rules`
- `merge_extraction_results` currently merges ONLY block specs — NOT tag rules or args

### Phase 1: Argument Structure Extraction

- [x] Define `ExtractedArg` type in `djls-extraction/src/types.rs` with variants/fields for: name (String), required (bool), kind (Literal/Variable/Choice/VarArgs), default value (Option), position index
- [x] Add `extracted_args: Vec<ExtractedArg>` field to `TagRule` in `djls-extraction/src/types.rs`
- [x] Implement arg extraction for `simple_tag`/`inclusion_tag` in `extract_parse_bits_rule()` in `rules.rs`: inspect function parameters, handle `takes_context=True` (skip first param), detect `*args`/`**kwargs`, parameter defaults → optional vs required, append auto `as varname` args
- [x] Implement arg extraction for manual `@register.tag` in `extract_compile_function_rule()` in `rules.rs`: reconstruct from `RequiredKeyword` positions (literal args), tuple unpacking analysis (`tag_name, item, _in, iterable = bits`) for variable names, indexed access (`bits[1]`) for positional names, fall back to generic `arg1`/`arg2` when AST analysis can't determine names
- [x] Update golden test snapshots for extraction results to include `extracted_args`
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Extracted Rule Evaluator

- [x] Create `crates/djls-semantic/src/rule_evaluation.rs` module
- [x] Implement evaluator for `ArgumentCountConstraint` variants: `Exact(N)` → error if `split_len != N`, `Min(N)` → error if `split_len < N`, `Max(N)` → error if `split_len > N`, `OneOf(set)` → error if `split_len not in set`. **Index offset**: extraction uses `split_contents()` indices (tag name at index 0), but parser `bits` excludes tag name — evaluator must account for this (+1 offset to split_len since bits doesn't include tag name)
- [x] Implement evaluator for `RequiredKeyword { position, value }`: check that `bits[position - 1]` (adjusted for tag name offset) equals `value`. Negative positions index from end. Skip check if position is out of bounds (tag too short — already caught by arg count constraint)
- [x] Implement evaluator for `KnownOptions { values, allow_duplicates, rejects_unknown }`: scan remaining bits for option-style arguments, validate against known set
- [x] Add `S117` diagnostic code (`ExtractedRuleViolation`) to `ValidationError` enum in `crates/djls-semantic/src/errors.rs` — carries a descriptive message derived from the rule context (e.g., "tag 'for' requires at least 4 arguments" or "'in' keyword expected at position 2")
- [x] Add `S117` to diagnostic system in `crates/djls-conf/` if needed
- [x] Implement `evaluate_tag_rules(tag_name: &str, bits: &[String], rules: &TagRule) -> Vec<ValidationError>` that runs all constraint checks and accumulates errors
- [x] Unit tests: each `ArgumentCountConstraint` variant individually, `RequiredKeyword` positive and negative positions, `KnownOptions` with unknown/duplicate values, index offset correctness (`bits` excludes tag name), empty rules → no errors
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Wire Evaluator into Validation Pipeline

- [x] Add `TagRule` storage: either add `extracted_rules: Option<TagRule>` field to `TagSpec`, or pass `ExtractionResult` directly to validation functions. Decision: store on `TagSpec` for consistency (merging already happens in `merge_extraction_results`)
- [x] Extend `merge_extraction_results()` in `specs.rs` to also merge `tag_rules` from `ExtractionResult` into `TagSpec.extracted_rules`
- [x] Modify `validate_all_tag_arguments()` in `arguments.rs`: when `spec.extracted_rules` is `Some`, call `evaluate_tag_rules()` instead of `validate_args_against_spec()`. When `None`, fall back to `validate_args_against_spec()` only if `spec.args` is non-empty (user-config `args` escape hatch)
- [x] Remove hand-crafted `args:` values from ALL tag specs in `builtins.rs` — set to `Cow::Borrowed(&[])`. Keep block structure (end_tag, intermediates, module, opaque)
- [x] Remove `EndTag.args` and `IntermediateTag.args` values from builtins (set to empty) — extraction doesn't produce arg specs for closers/intermediates
- [x] Key regression test: `{% for item in items football %}` must still error (with extracted rules), `{% for item in items %}` must still pass, builtins without extracted rules skip validation
- [x] Update test infrastructure (`check_validation_errors_with_db` etc.) to handle S117 variant — removed `ExtractedRuleViolation` from error filter, converted builtin-dependent tests to use extracted rules or user-config specs
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Wire Extracted Args into Completions/Snippets

- [x] Implement `ExtractedArg` → `TagArg` conversion function (in `specs.rs` or new helper)
- [x] Update `merge_extraction_results()` (or `compute_tag_specs`) to populate `TagSpec.args` from `extracted_args` when available — completions/snippets code reads `spec.args` unchanged
- [x] Verify completion tests still pass — source of `args` changed but consumer interface unchanged
- [x] Verify snippet tests still pass if any exist
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 5: Clean Up Dead Code

- [x] Remove `TagArgSliceExt` trait if only used by deleted `validate_argument_order` internals — KEPT: still used by `validate_argument_order` (user-config escape hatch)
- [x] Update doc comments on `TagSpec.args` to reflect new role (completions/snippets only, not validation — validation uses `extracted_rules`)
- [x] Clean up `builtins.rs` — remove all the hand-crafted arg definitions that are now empty, simplify tag spec construction — ALREADY DONE in Phase 3 (all args empty, doc comment updated)
- [x] Remove any dead helper functions in `arguments.rs` that were only used by the old validation path (keep `validate_args_against_spec` for user-config escape hatch) — NO dead functions found; all remaining functions serve the user-config escape hatch
- [x] Keep `TagArg` enum and S104–S107 variants — still needed for user-config `args` in `djls.toml`
- [x] Update AGENTS.md with operational notes about M8 changes
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 6: Corpus Template Validation Tests

- [x] Create corpus template validation test infrastructure in `djls-server` or `djls-semantic` test module (NOT standalone test file — add to existing `#[cfg(test)]` module)
- [x] Implement `CorpusTestDatabase` (lightweight) that builds `TagSpecs` from extraction results rather than hand-crafted builtins
- [x] For each Django version in corpus (if synced): extract rules from that version's `defaulttags.py`/`defaultfilters.py`/etc., validate its shipped `contrib/admin` templates, assert zero false positives for argument validation
- [x] For each third-party package in corpus (Wagtail, allauth, crispy-forms, debug-toolbar, compressor): extract rules from package templatetags + Django builtins, validate templates, assert zero argument-validation false positives
- [x] Port prototype's template exclusion list (AngularJS templates, known-invalid upstream templates)
- [x] Gate all corpus tests on availability (skip gracefully when corpus not synced) using `find_corpus_dir()` / `find_django_source()` pattern from M5 Phase 9
- [x] Known-invalid templates produce expected errors (positive test cases)
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M9 — Remove User Config TagSpecs

**Status:** complete
**Plan:** `.agents/plans/2026-02-06-m9-tagspec-simplification.md`
**Depends on:** M8

**Goal:** Remove the entire user-config `[tagspecs]` system from `djls.toml`. After M8, Python AST extraction handles all tag validation — the tagspec config types, `TagArg` enum, old validation engine, and S104–S107 error codes are dead weight. Users suppress false positives via `diagnostics.severity.S117 = "off"`.

### Phase 1: Remove TagSpecs Config System

- [x] Delete `crates/djls-conf/src/tagspecs.rs` and `crates/djls-conf/src/tagspecs/legacy.rs` — removes all config types (`TagSpecDef`, `TagLibraryDef`, `TagDef`, `EndTagDef`, `IntermediateTagDef`, `TagTypeDef`, `PositionDef`, `TagArgDef`, `ArgKindDef`, `ArgTypeDef`, and legacy equivalents)
- [x] Remove `pub mod tagspecs` and all tagspec re-exports from `crates/djls-conf/src/lib.rs`
- [x] Remove `tagspecs` field from `Settings` struct, remove `deserialize_tagspecs` function, remove `Settings::tagspecs()` accessor, remove tagspec override logic in `Settings`
- [x] Delete all tagspec-related tests in `crates/djls-conf/src/lib.rs`
- [x] Remove `tagspecs: TagSpecDef` field from `Project` salsa input in `crates/djls-project/src/project.rs` — update `Project::new()` and `Project::bootstrap()` signatures (one fewer argument)
- [x] Update all call sites of `Project::new` / `Project::bootstrap` in `crates/djls-server/src/db.rs` to remove tagspecs argument
- [x] In `compute_tag_specs` in `db.rs`, remove the user-config merge layer (layer 4 that reads `project.tagspecs(db)` and calls `TagSpecs::from_config_def`)
- [x] In `update_project_from_settings`, remove the tagspec diff/set logic
- [x] Delete `TagSpecs::from_config_def()` and all `From<conf types>` impls in `crates/djls-semantic/src/templatetags/specs.rs` (`From<(TagDef, String)> for TagSpec`, `From<EndTagDef> for EndTag`, `From<IntermediateTagDef> for IntermediateTag`, `From<TagArgDef> for TagArg`)
- [x] Delete tests that use conf types in specs.rs
- [x] Update invalidation tests in `db.rs`: remove `tagspecs_change_invalidates` test, update `tag_index_invalidation` if it uses `set_tagspecs`
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Remove `TagArg` System and Old Validation Engine

- [x] Delete from `specs.rs`: `TokenCount` enum, `LiteralKind` enum, `TagArg` enum (all 7 variants + constructors), `TagArgSliceExt` trait and impl, `From<ExtractedArg> for TagArg` impl
- [x] Remove `args: L<TagArg>` field from `TagSpec`, `EndTag`, and `IntermediateTag` in `specs.rs` — update all constructors, `merge_block_spec`, and `merge_extraction_results`
- [x] Remove re-exports of `TagArg`, `TagArgSliceExt`, `LiteralKind`, `TokenCount` from `templatetags.rs` and `crates/djls-semantic/src/lib.rs`
- [x] Strip all `args:` lines from `builtins.rs` (including `BLOCKTRANS_ARGS` constant if present) — keep block structure (end_tag, intermediates, module, opaque)
- [x] Gut `arguments.rs`: delete `validate_args_against_spec` and `validate_argument_order` functions; simplify `validate_tag_arguments` to only dispatch to extracted rule evaluator (no fallback path)
- [x] Delete all tests in `arguments.rs` that construct `TagArg` specs — keep structural tests that use extracted rules
- [x] Update `completions.rs`: replace `TagArg`-based argument completion logic with `ExtractedArg`-based logic (read `spec.extracted_rules.extracted_args` directly instead of `spec.args`); update `TemplateCompletionContext::TagArgument` variant if needed
- [x] Update any snippet generation code that uses `TagArg`
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Remove Dead Error Variants and Diagnostic Codes

- [x] Delete from `errors.rs`: `MissingRequiredArguments` (S104), `TooManyArguments` (S105), `MissingArgument` (S104), `InvalidLiteralArgument` (S106), `InvalidArgumentChoice` (S107) variants
- [x] Remove corresponding span extraction arms and code mapping arms from `diagnostics.rs` (or wherever S-code mappings live)
- [x] Fix match exhaustiveness in any remaining files that match on `ValidationError`
- [x] Remove S104–S107 from `DiagnosticsConfig` default severity mapping if present — N/A, no hardcoded defaults for these codes
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Update Documentation

- [x] Delete `docs/configuration/tagspecs.md`
- [x] Remove tagspecs entry from `.mkdocs.yml` nav
- [x] In `docs/configuration/index.md`: remove `[tagspecs]` config section, remove S104–S107 from diagnostic codes table, rename "Block Structure (S100-S107)" section header to "Block Structure (S100-S103)"
- [x] Add note in config docs that template tag validation is handled automatically by Python AST extraction
- [x] Update `docs/template-validation.md` to remove tagspec references, note argument validation uses Django's own error messages via extraction
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M10 — Dataflow Analyzer (Replace Pattern-Matching Extraction)

**Status:** complete
**Plan:** `.agents/plans/2026-02-06-m10-dataflow-analyzer.md`
**Depends on:** M5, M8
**Design doc:** `docs/dev/extraction-dataflow-analyzer.md`

**Goal:** Replace the ad-hoc pattern matching in `context.rs` + `rules.rs` (~2190 lines) with a domain-specific abstract interpreter that tracks `token` and `parser` through compile functions. Handles Django 6.0 `match` statements, helper function delegation, and all corpus patterns from a unified framework.

**What's replaced:** `context.rs` (572 lines — `detect_split_var`, `token_delegated_to_helper`) + `rules.rs` (1618 lines — `extract_compile_function_rule`, `extract_parse_bits_rule`, `extract_args_from_compile_function`, `extract_option_loop`)

**What stays unchanged:** `registry.rs`, `blocks.rs`, `filters.rs`, `types.rs`, `lib.rs` (public API)

**What's new:** `dataflow/` module (domain types, eval, constraints, calls) + `signature.rs` (extracted from `rules.rs`)

### Phase 1: Module Structure, Domain Types, and Basic Environment

- [x] Create `crates/djls-extraction/src/signature.rs` — move `extract_parse_bits_rule` and helpers (`has_takes_context`, `is_true_constant`) from `rules.rs`, along with their tests
- [x] Create `crates/djls-extraction/src/dataflow.rs` parent module with `analyze_compile_function` stub (returns empty `TagRule`)
- [x] Create `crates/djls-extraction/src/dataflow/domain.rs` — `AbstractValue` enum (`Unknown`, `Token`, `Parser`, `SplitResult`, `SplitElement`, `SplitLength`, `Int`, `Str`, `Tuple`, `List`), `Index` enum (`Forward(usize)`, `Backward(usize)`), `Env` struct (HashMap bindings, `for_compile_function`, `get`, `set`, `mutate`)
- [x] Create stub files: `dataflow/eval.rs`, `dataflow/constraints.rs`, `dataflow/calls.rs`
- [x] Register new modules in `lib.rs` with `#[cfg(feature = "parser")]` gates, add public re-exports
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Expression Evaluation and Statement Processing

- [x] Implement `eval_expr(expr, env) -> AbstractValue` in `dataflow/eval.rs`: handle Name lookup, int/string literals, `token.split_contents()` → `SplitResult`, `token.contents.split()` → `SplitResult`, `len(x)` → `SplitLength`, subscript `x[N]` → `SplitElement`, slice `x[N:]` → offset-adjusted `SplitResult`, `list(x)` passthrough, tuple literals
- [x] Implement `process_statements(stmts, env, ctx)` in `dataflow/eval.rs`: simple assignment, tuple unpack, star unpack (`tag_name, *rest = bits`), if/elif/else recursion, for/try/with recursion (while/match skipped for now)
- [x] Define `AnalysisContext` struct bundling `module_funcs`, `caller_name`, `call_depth`, `cache` (cache unused until Phase 5)
- [x] Wire `analyze_compile_function` to extract parser/token param names from function signature, create `Env`, call `process_statements`
- [x] Tests: env initialization, split_contents binding, subscript/negative subscript, slice, slice with existing offset, len(), list() wrapping, star unpack, tuple unpack, contents.split(None, 1), unknown variable
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Constraint Extraction from If/Raise

- [x] Implement `Constraints` struct and `extract_constraints(stmts, env) -> Constraints` in `dataflow/constraints.rs`
- [x] Implement `eval_condition(expr, env, constraints)` for condition → constraint mapping: `len(sr) < N` → `Min(N + base_offset)`, `len(sr) > N` → `Max`, `len(sr) != N` → `Exact`, `<=`/`>=` variants, reversed comparisons (`N < len(sr)`), `not in` → `OneOf`, `elem != "kw"` → `RequiredKeyword`
- [x] Handle compound conditions: `or` → extract from both, `and` → extract keywords only (discard length), negated range `not (A <= len(sr) <= B)` → Min + Max
- [x] Detect `raise TemplateSyntaxError(...)` in if-bodies (reuse pattern from `rules.rs`)
- [x] Recurse into nested if/elif/else to find nested if-raise patterns
- [x] Wire constraints into `analyze_compile_function` → populate `TagRule.arg_constraints` and `TagRule.required_keywords`
- [x] Tests: each comparison operator, reversed comparisons, RequiredKeyword with forward/backward index, compound or/and, negated range, `not in`, offset adjustment after slice, multiple raises, nested if-raise, elif raise, non-TemplateSyntaxError ignored, end-to-end `regroup` pattern
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Side Effects (pop, mutation)

- [x] Extend `SplitResult` and `SplitLength` with `pops_from_end: usize` field, update all construction sites
- [x] Handle `bits.pop(0)` in `process_statements`: increment `base_offset`, optionally assign popped element as `SplitElement(Forward(old_offset))`
- [x] Handle `bits.pop()` in `process_statements`: increment `pops_from_end`, optionally assign as `SplitElement(Backward(0))`
- [x] Update constraint offset formula in `constraints.rs`: `Min(N + base_offset + pops_from_end)`, etc.
- [x] Tests: pop(0) offset, pop(0) with assignment, pop() from end, multiple pops, offset-adjusted constraint after pop, end-pop-adjusted constraint
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 5: Intra-Module Function Calls

- [x] Implement `HelperCache` in `dataflow/calls.rs` with `AbstractValueKey` for hashable cache keys
- [x] Implement `resolve_call(callee_name, args, module_funcs, caller_name, call_depth, cache) -> AbstractValue`: check cache, find callee in module_funcs, create env with parameter bindings, process callee body, extract return value, cache result
- [x] Add depth limit (`MAX_CALL_DEPTH = 2`) and self-recursion guard
- [x] Add hardcoded external summaries: `token_kwargs(bits, parser)` → Unknown + mark bits Unknown, `parser.compile_filter(expr)` → Unknown, `parser.parse(tags)` → Unknown
- [x] Wire into `eval_expr`: on Call expression, check Django API calls, then try module-local resolution
- [x] Thread `HelperCache` through `AnalysisContext`
- [x] Tests: simple helper returning split_contents, tuple return destructuring, allauth `parse_tag` pattern (no constraints), depth limit, self-recursion, helper not found, token_kwargs, parser.compile_filter, cache hit (same args), cache miss (different args)
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 6: While-Loop Option Parsing

- [x] Implement `try_extract_option_loop(while_stmt, env) -> Option<KnownOptions>` in `eval.rs`: detect `while remaining:` where remaining is SplitResult-derived, find `option = var.pop(0)`, scan if/elif/else for option value checks, detect `else: raise` → `rejects_unknown`, detect duplicate detection → `allow_duplicates`
- [x] Wire into `process_statements` for `Stmt::While`
- [x] Wire `KnownOptions` into `analyze_compile_function` → populate `TagRule.known_options`
- [x] Tests: basic option loop, duplicate detection, no else raise, Django `do_include` pattern, translate tag pattern
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 7: Match Statement Support (Python 3.10+)

- [x] Implement `extract_match_constraints(match_stmt, env) -> Option<(Vec<ArgumentCountConstraint>, Vec<RequiredKeyword>)>` in `eval.rs`: check subject is SplitResult, analyze `PatternMatchSequence` patterns for length and literal positions, separate error cases (body raises) from valid cases, derive constraints from union of valid case shapes
- [x] Handle pattern types: `PatternMatchValue` (literal at position), `PatternMatchAs` (capture/wildcard), `PatternMatchStar` (variable length)
- [x] Wire into `process_statements` for `Stmt::Match` and `extract_from_body` in `constraints.rs`
- [x] Tests: Django 6.0 `partialdef` pattern → `OneOf([2, 3])`, `partial` pattern → `Exact(2)`, match on non-SplitResult → no constraints, star pattern → `Min(1)`, multiple valid lengths → `OneOf([2, 4])`, all-error cases → no constraints, env updates propagate through match bodies
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 8: Integration, Extracted Arg Names, and Corpus Validation

- [x] Implement `extract_arg_names(env, constraints, stmts) -> Vec<ExtractedArg>` in `dataflow.rs`: scan env for `SplitElement` bindings → variable names at positions, combine with `RequiredKeyword` positions, fall back to generic `arg1`/`arg2`
- [x] Wire `analyze_compile_function` into `lib.rs`: replace `rules::extract_tag_rule` dispatch — `Tag`/`SimpleBlockTag` → `dataflow::analyze_compile_function`, `SimpleTag`/`InclusionTag` → `signature::extract_parse_bits_rule`
- [x] Update `lib.rs` public re-exports: old exports kept for Phase 9 deletion, new dispatch active
- [x] Run `INSTA_UPDATE=1 cargo test -q -p djls-extraction` + review — all snapshot diffs are equal-or-better (more extracted args, correct keyword positions)
- [x] Run golden corpus tests (18/18), full corpus extraction test (4/4), corpus template validation (same pre-existing failures only)
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 9: Delete Old Code

- [x] Delete `crates/djls-extraction/src/context.rs` and `crates/djls-extraction/src/rules.rs`
- [x] Remove `mod context` and `mod rules` declarations from `lib.rs`, remove their re-exports
- [x] Clean up orphaned snapshot files: `cargo insta test --delete-unreferenced-snapshots -p djls-extraction`
- [x] Verify no external consumers of deleted APIs: `grep -rn "detect_split_var\|token_delegated_to_helper\|extract_compile_function_rule" crates/`
- [x] Run `cargo +nightly fmt`
- [x] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M11 — Environment-Aware Tag/Filter Resolution

**Status:** planning
**Plan:** (derived from roadmap — no separate plan file)
**Depends on:** M3/M4 (load scoping), M5 (extraction crate with Ruff parser)

**Goal:** Distinguish three layers of tag/filter availability: not in Python environment (S108/S111), in environment but not in `INSTALLED_APPS` (new diagnostic), and in `INSTALLED_APPS` but not loaded (S109/S112). Also validate `{% load %}` library names against inspector inventory.

**Three-layer resolution model:**
```
Python Environment  →  Django Configuration  →  Template Load  →  Available
(pip install)          (INSTALLED_APPS)          ({% load %})
```

| Layer | Failure | Diagnostic | Fix |
|---|---|---|---|
| Not in environment | Package not installed | S108/S111 UnknownTag/Filter | `pip install ...` |
| In env, not in INSTALLED_APPS | App not activated | **New S118/S119** | Add app to `INSTALLED_APPS` |
| In INSTALLED_APPS, not loaded | No `{% load %}` | S109/S112 UnloadedTag/Filter | Add `{% load X %}` |

### Phase 1: `{% load %}` Library Name Validation (Quick Win — No Environment Scan)

- [ ] Add `S120` diagnostic code (`UnknownLibrary`) to `ValidationError` in `crates/djls-semantic/src/errors.rs` with message "Unknown template tag library '{name}'"
- [ ] Add `S121` diagnostic code (`AmbiguousUnknownLibrary`) — reserved for Phase 4 when environment scan can distinguish "unknown" from "not in INSTALLED_APPS"
- [ ] Add S120 to diagnostic system in `crates/djls-conf/src/diagnostics.rs`
- [ ] Implement `validate_load_libraries()` in `crates/djls-semantic/src/load_resolution/validation.rs`: for each `Node::Tag { name: "load" }`, parse bits to get library names (full load) or the `from` library (selective), check each against `TemplateTags.libraries()` keys
- [ ] Guard: skip when `inspector_inventory` is `None`
- [ ] Handle selective imports: `{% load trans from i18n %}` → validate `i18n` is a known library
- [ ] Wire `validate_load_libraries` into `validate_nodelist` in `crates/djls-semantic/src/lib.rs`
- [ ] Tests: known library valid, unknown library → S120, selective import with known library valid, selective import with unknown library → S120, inspector unavailable → no diagnostics, multiple libraries in one load (`{% load i18n static %}`) — each validated independently
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Environment Scanner — File Discovery

- [ ] Create `crates/djls-extraction/src/environment.rs` module (behind `parser` feature gate)
- [ ] Define `EnvironmentLibrary` struct: `load_name: String`, `app_module: String`, `module_path: PathBuf`, `source_path: PathBuf`
- [ ] Define `EnvironmentInventory` struct: map from load_name → `Vec<EnvironmentLibrary>` (Vec because name collisions across packages are possible), with accessors `libraries()`, `has_library(name)`, `libraries_for_name(name)`
- [ ] Implement `scan_environment(sys_paths: &[PathBuf]) -> EnvironmentInventory`: glob each sys_path entry for `*/templatetags/*.py`, skip `__init__.py` and `__pycache__`, derive `load_name` from filename stem, derive `app_module` from parent directory structure (e.g., `django/contrib/humanize/templatetags/humanize.py` → app `django.contrib.humanize`)
- [ ] Handle edge cases: `templatetags/` without `__init__.py` (skip — not a valid Python package), symlinks, namespace packages
- [ ] Export types from `crates/djls-extraction/src/lib.rs`
- [ ] Tests: scan with mock directory structure, name collision detection, `__init__.py` filtering, empty directory handling
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Environment Scanner — Symbol-Level Extraction

- [ ] Extend `EnvironmentLibrary` with `tags: Vec<String>` and `filters: Vec<String>` fields
- [ ] Implement `scan_environment_with_symbols(sys_paths: &[PathBuf]) -> EnvironmentInventory`: for each discovered `templatetags/*.py`, parse with `ruff_python_parser::parse_module`, call `collect_registrations_from_body` (existing M5 function), separate into tags/filters by `RegistrationKind`
- [ ] Handle parse failures gracefully: if Ruff can't parse a file, still include the library at library-level (empty tags/filters lists) — don't skip entirely
- [ ] Define `EnvironmentSymbol` struct: `name: String`, `library_load_name: String`, `app_module: String` — for reverse lookup ("which library provides tag X?")
- [ ] Add `tags_by_name()` and `filters_by_name()` methods on `EnvironmentInventory` returning `HashMap<String, Vec<EnvironmentSymbol>>` for quick reverse lookup
- [ ] Tests: extract registrations from real-ish templatetag files, parse failure → library still discovered, symbol-level reverse lookup works
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Salsa Integration — Store Environment Inventory on Project

- [ ] Add `environment_inventory: Option<EnvironmentInventory>` field to `Project` input in `crates/djls-project/src/project.rs` with `#[returns(ref)]`
- [ ] Add `djls-extraction` dependency to `djls-project/Cargo.toml` for `EnvironmentInventory` type (types-only, no `parser` feature) — or define a separate types module if needed to avoid pulling in extraction types
- [ ] Derive `PartialEq` + `Eq` on `EnvironmentInventory`, `EnvironmentLibrary`, `EnvironmentSymbol`
- [ ] Initialize `environment_inventory` as `None` in `Project::bootstrap`
- [ ] Implement environment scan in `refresh_inspector()` on `DjangoDatabase`: after inspector query completes, run `scan_environment_with_symbols` using `pythonpath` + interpreter's `sys.path`, compare with current value, set only if changed
- [ ] Add `environment_inventory()` accessor to `SemanticDb` trait and implement on `DjangoDatabase`
- [ ] Update all test databases implementing `SemanticDb` to include `environment_inventory()` method
- [ ] Tests: environment inventory stored on Project, refresh updates inventory, same value → no Salsa invalidation
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 5: Three-Layer Resolution — Tags and Filters

- [ ] Add `S118` diagnostic code (`TagNotInInstalledApps`) to `ValidationError`: "Tag '{name}' requires '{app}' in INSTALLED_APPS" — carries tag name, app module, and load name
- [ ] Add `S119` diagnostic code (`FilterNotInInstalledApps`) to `ValidationError`: "Filter '{name}' requires '{app}' in INSTALLED_APPS"
- [ ] Add S118, S119 to diagnostic system in `crates/djls-conf/`
- [ ] Update `validate_tag_scoping()` in `load_resolution/validation.rs`: when a tag is currently classified as S108 (UnknownTag), check `environment_inventory` — if found there, reclassify as S118 (TagNotInInstalledApps) with the app module info
- [ ] Update `validate_filter_scoping()` similarly: S111 → S119 when filter found in environment inventory
- [ ] Handle ambiguity: tag/filter found in multiple environment libraries from different apps → include all candidates in diagnostic message
- [ ] Guard: when `environment_inventory` is `None`, fall through to existing S108/S111 behavior (no regression)
- [ ] Tests: tag in environment but not INSTALLED_APPS → S118 with correct app name, filter in environment but not INSTALLED_APPS → S119, tag truly unknown (not in environment) → S108 unchanged, environment unavailable → S108/S111 unchanged, tag in multiple environment packages → S118 with multiple candidates
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 6: Three-Layer Resolution — `{% load %}` Libraries

- [ ] Repurpose `S121` diagnostic code (`LibraryNotInInstalledApps`): "Template tag library '{name}' requires '{app}' in INSTALLED_APPS"
- [ ] Update `validate_load_libraries()` (from Phase 1): when a library is not in inspector's `libraries()`, check `environment_inventory.has_library(name)` — if found, emit S121 instead of S120
- [ ] Include the app module in the S121 diagnostic message so users know exactly what to add
- [ ] Handle ambiguity: library name exists in multiple apps in environment → include all candidates
- [ ] Tests: library in environment but not INSTALLED_APPS → S121 with app name, library truly unknown → S120, environment unavailable → S120, ambiguous library name across apps → S121 with candidates
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 7: Documentation and Integration Tests

- [ ] Update `docs/template-validation.md` with three-layer resolution explanation
- [ ] Add S118–S121 to diagnostic codes documentation in `docs/configuration/index.md`
- [ ] Integration test: template with tags/filters from all three layers — verify correct diagnostic codes emitted for each layer
- [ ] Update `.github/ISSUE_TEMPLATE/template-validation-mismatch.yml` if needed for new diagnostic codes
- [ ] Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M12 — `{% extends %}` Structural Validation

**Status:** backlog
**Plan:** `.agents/plans/YYYY-MM-DD-m12-extends-structural-validation.md` (not yet created)
**Depends on:** None

**Goal:** Validate that `{% extends %}` is the first tag in a template (no tags or variables before it) and appears at most once. Both are parse-time Django rules that can be checked from the `NodeList`.

*(Tasks to be expanded when this milestone is next up for implementation.)*

---

## M13 — Complete Extraction Coverage + Remove `builtins.rs`

**Status:** backlog
**Plan:** `.agents/plans/YYYY-MM-DD-m13-extraction-completeness.md` (not yet created)
**Depends on:** M10 (dataflow analyzer)

**Goal:** Extend extraction to handle `blocktrans`/`blocktranslate` block specs (parser.next_token() loops) and value-in-set constraints (`ChoiceAt`). Remove `builtins.rs` entirely — `compute_tag_specs` populates purely from extraction results.

*(Tasks to be expanded when this milestone is next up for implementation.)*

---

## Discoveries / Notes

- **M10 dataflow analyzer architecture**: `analyze_compile_function` extracts parser/token param names from function signature, creates `Env`, processes statements. `eval_expr` has `_with_ctx` variant for call resolution. `HelperCache` keyed by `(func_name, Vec<AbstractValueKey>)` with bounded inlining (depth 2) and self-recursion guard.
- **Django API summaries in dataflow**: `token_kwargs(bits, parser)` → side-effect call (marks bits `Unknown`). `parser.compile_filter`/`parser.parse`/`parser.delete_first_token` → `Unknown`. Test helper `analyze_with_helpers` uses `starts_with("do_")` to find compile function.
- **Corpus test expectations**: Pre-existing corpus test failures (`test_repo_templates_zero_arg_false_positives`, `test_third_party_templates_zero_arg_false_positives`) are known — third-party tests use warn-only, not assertion failure.
- **Inline constraint extraction**: Constraints must be extracted during `process_statements` (inline), NOT after. The original design ran `extract_constraints` on the final env state, which was wrong for functions that reassign the split variable (e.g., `bits = bits[2:]` in Django's `url` tag). Now `AnalysisContext` carries a `Constraints` field, and `extract_from_if_inline` is called during if-statement processing so constraints see the env at the point they appear in code.
