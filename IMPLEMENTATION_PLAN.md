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

**Status:** complete
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

- [x] Add `update_project_from_settings(&mut self, project: Project, settings: &Settings)` method on `DjangoDatabase` — compares each field before calling setters (Ruff/RA pattern to avoid spurious invalidation)
- [x] Add `refresh_inspector(&mut self)` method on `DjangoDatabase` — queries Python inspector directly, compares result with `project.inspector_inventory(db)`, only calls setter if changed
- [x] Rewrite `set_project()` to use `Project::bootstrap` with the new signature and call `refresh_inspector()` after creation
- [x] Rewrite `set_settings()` to delegate field updates to `update_project_from_settings()` and trigger `refresh_inspector()` only when environment fields change
- [x] Make `TemplatetagsRequest`, `TemplatetagsResponse` public in `crates/djls-project/src/django.rs` and export from `crates/djls-project/src/lib.rs` (needed for direct inspector queries)
- [x] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

### Phase 4: Invalidation Tests with Event Capture

- [x] Add `EventLogger` test infrastructure in `crates/djls-server/src/db.rs` `#[cfg(test)]` module — stores raw `salsa::Event` values, provides `was_executed(db, query_name)` helper using `db.ingredient_debug_name()`
- [x] Add `TestDatabase` helper struct with `with_project()` constructor that wires up `EventLogger` to Salsa storage
- [x] Test: `tag_specs_cached_on_repeated_access` — first call executes `compute_tag_specs`, second call uses cache
- [x] Test: `tagspecs_change_invalidates` — modifying `project.tagspecs` via setter causes recomputation
- [x] Test: `inspector_inventory_change_invalidates` — setting `project.inspector_inventory` causes recomputation
- [x] Test: `same_value_no_invalidation` — comparing before setting prevents spurious invalidation
- [x] Test: `tag_index_depends_on_tag_specs` — changing tagspecs recomputes tag_index too
- [x] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

---

## M3 - `{% load %}` Scoping Infrastructure

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m3-load-scoping.md` (overview), phases in `m3.1` through `m3.6`

### Phase 1: Load Statement Parsing and Data Structures

- [x] Create `crates/djls-semantic/src/load_resolution.rs` with `LoadStatement`, `LoadKind` (Libraries/Selective), and `LoadedLibraries` types
- [x] Implement `parse_load_bits(bits, span) -> Option<LoadStatement>` — handles `{% load lib1 lib2 %}` and `{% load sym from lib %}` syntax
- [x] Implement `LoadedLibraries` methods: `new()`, `push()`, `loads()`, `libraries_before(position)`, `selective_symbols_before(position)`, `is_library_loaded_before(library, position)`
- [x] Add unit tests: single library, multiple libraries, selective single, selective multiple, empty bits, invalid from syntax, libraries_before position, selective_symbols_before
- [x] Export `LoadKind`, `LoadStatement`, `LoadedLibraries`, `parse_load_bits` from `crates/djls-semantic/src/lib.rs`
- [x] Run `cargo build -p djls-semantic`, `cargo clippy -p djls-semantic --all-targets --all-features -- -D warnings`, `cargo test -p djls-semantic`

### Phase 2: Compute LoadedLibraries from NodeList (Tracked Query)

- [x] Add `djls-project` dependency to `crates/djls-semantic/Cargo.toml` (needed for `TemplateTags`/`TagProvenance` in Phase 3)
- [x] Add `#[salsa::tracked] fn compute_loaded_libraries(db, nodelist) -> LoadedLibraries` — iterate over nodelist, extract `{% load %}` tags, parse bits, sort by span start
- [x] Export `compute_loaded_libraries` from `crates/djls-semantic/src/lib.rs`
- [x] Run `cargo build -p djls-semantic`, `cargo clippy -p djls-semantic --all-targets --all-features -- -D warnings`, `cargo test -p djls-semantic`

### Phase 3: Available Symbols Query

- [x] Add `AvailableSymbols` type with `tags: FxHashSet<String>` and `has_tag(name)` method
- [x] Add `LoadState` internal struct with state-machine approach: `fully_loaded: FxHashSet`, `selective: FxHashMap<lib, FxHashSet<sym>>`, `process(stmt)`, `is_tag_available(tag, lib)`
- [x] Implement `available_tags_at(loaded, inventory, position) -> AvailableSymbols` — processes loads in order, builtins always available, library tags require loaded library or selective import
- [x] Add `inspector_inventory() -> Option<TemplateTags>` method to `crate::Db` trait in `crates/djls-semantic/src/db.rs`
- [x] Implement `inspector_inventory()` in `DjangoDatabase` (`crates/djls-server/src/db.rs`) — reads `project.inspector_inventory(db)`
- [x] Also implemented `inspector_inventory()` in `djls-bench` `Db` (returns `None`) and three test `TestDatabase` impls in djls-semantic
- [x] Export `available_tags_at` and `AvailableSymbols` from `crates/djls-semantic/src/lib.rs`
- [x] Add comprehensive tests: builtins_always_available, library_tag_after_load, selective_import, selective_then_full_load, full_then_selective_no_effect, multiple_selective_same_lib
- [x] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

### Phase 4: Validation Integration - Unknown Tag Diagnostics

- [x] Add new `ValidationError` variants: `UnknownTag { tag, span }`, `UnloadedLibraryTag { tag, library, span }`, `AmbiguousUnloadedTag { tag, libraries, span }`
- [x] Add diagnostic codes in `crates/djls-ide/src/diagnostics.rs`: S108 (UnknownTag), S109 (UnloadedLibraryTag), S110 (AmbiguousUnloadedTag)
- [x] Add `TagInventoryEntry` enum (Builtin / Libraries(Vec<String>)) and `build_tag_inventory(inventory) -> FxHashMap<String, TagInventoryEntry>` for collision handling
- [x] Implement `#[salsa::tracked] fn validate_tag_scoping(db, nodelist)` — skip if inspector unavailable, skip tags with structural specs (openers/closers/intermediates), emit S108/S109/S110
- [x] Wire `validate_tag_scoping` into `validate_nodelist` in `crates/djls-semantic/src/lib.rs`
- [x] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

### Phase 5: Completions Integration

- [x] Add `loaded_libraries: Option<&LoadedLibraries>` and `cursor_byte_offset: u32` params to `generate_tag_name_completions`, `generate_template_completions`, and `handle_completion`
- [x] Add `calculate_byte_offset(document, position, encoding) -> u32` helper for UTF-16 → byte offset conversion
- [x] Filter tag name completions by `available_tags_at` when load info is present; show all tags when unavailable (fallback)
- [x] Update server call site (`crates/djls-server/src/server.rs`) to compute `LoadedLibraries` from nodelist and pass to completion handler
- [x] Update completion tests to cover load-scoped filtering
- [x] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

### Phase 6: Library Completions Enhancement

- [x] Update `generate_library_completions` to accept `loaded_libraries` and `cursor_byte_offset` params
- [x] Deprioritize already-loaded libraries (sort_text `"1_"` prefix, mark deprecated for strikethrough)
- [x] Update call site in `generate_template_completions` to pass new params
- [x] Run `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`

---

## M4 - Filters Pipeline

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m4-filters-pipeline.md` (overview), phases in `m4.1` through `m4.4`

### Phase 1: Inspector Filter Inventory and Unified Types

**Goal:** Add filter collection to Python inspector, create unified `InspectorInventory` type replacing `TemplateTags`, update Project input and refresh_inspector.

- [x] Add `TemplateFilter` dataclass in `crates/djls-project/inspector/queries.py` with `name`, `provenance`, `defining_module`, `doc` fields
- [x] Add `TemplateInventoryQueryData` dataclass with `libraries`, `builtins`, `templatetags`, `templatefilters` fields
- [x] Add `TEMPLATE_INVENTORY = "template_inventory"` to `Query` enum in `queries.py`
- [x] Implement `get_template_inventory()` — iterate both `library.tags` and `library.filters` for builtins and libraries, return unified payload
- [x] Wire `TEMPLATE_INVENTORY` query to `get_template_inventory()` in the query dispatch
- [x] Add `FilterProvenance` enum in `crates/djls-project/src/django.rs` (mirrors `TagProvenance`: `Library { load_name, module }` / `Builtin { module }`)
- [x] Add `TemplateFilter` struct in `crates/djls-project/src/django.rs` with accessors: `name()`, `provenance()`, `defining_module()`, `doc()`, `library_load_name()`, `is_builtin()`, `registration_module()`
- [x] Add `InspectorInventory` struct in `crates/djls-project/src/django.rs` with `libraries`, `builtins`, `tags`, `filters` fields and accessors
- [x] Add `TemplateInventoryRequest` / `TemplateInventoryResponse` types in `crates/djls-project/src/django.rs`
- [x] Change `Project.inspector_inventory` field type from `Option<TemplateTags>` to `Option<InspectorInventory>` in `crates/djls-project/src/project.rs`
- [x] Update `Project::bootstrap()` to pass `None` for the new type
- [x] Export `FilterProvenance`, `TemplateFilter`, `InspectorInventory`, `TemplateInventoryRequest`, `TemplateInventoryResponse` from `crates/djls-project/src/lib.rs`
- [x] Update `SemanticDb::inspector_inventory()` trait method return type to `Option<InspectorInventory>` in `crates/djls-semantic/src/db.rs`
- [x] Update `DjangoDatabase::inspector_inventory()` impl in `crates/djls-server/src/db.rs` to return `InspectorInventory`
- [x] Update `refresh_inspector()` in `crates/djls-server/src/db.rs` to use `TemplateInventoryRequest` and build `InspectorInventory`
- [x] Update `compute_tag_specs()` to read tags from `InspectorInventory` instead of `TemplateTags`
- [x] Update all test `inspector_inventory()` impls (bench db, 3 semantic test databases) to return `Option<InspectorInventory>`
- [x] Update all M3 code in `load_resolution.rs` that reads from `TemplateTags` to use `InspectorInventory` instead
- [x] Update completions code in `djls-ide` that passes tag inventory — adapt to unified `InspectorInventory`
- [x] Update server completion call site to pass `InspectorInventory`
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 2: Structured Filter Representation (Parser Breakpoint)

**Goal:** Transform `filters: Vec<String>` → `Vec<Filter>` with name/arg/span in the parser, updating all downstream consumers.

- [x] Add `Filter` and `FilterArg` structs in `crates/djls-templates/src/nodelist.rs` with `name`, `arg`, `span` fields
- [x] Change `Node::Variable { filters: Vec<String> }` to `Node::Variable { filters: Vec<Filter> }` in `crates/djls-templates/src/nodelist.rs`
- [x] Implement quote-aware `VariableScanner` in `crates/djls-templates/src/parser.rs` — handles `|` inside quotes, escape sequences, whitespace
- [x] Rewrite `parse_variable()` to use `VariableScanner` producing `Vec<Filter>` with byte-accurate spans
- [x] Export `Filter` and `FilterArg` from `crates/djls-templates/src/lib.rs`
- [x] Update `NodeView::Variable` in `crates/djls-semantic/src/blocks/tree.rs` to use `Vec<djls_templates::Filter>`
- [x] Update pattern matches in `crates/djls-semantic/src/blocks/builder.rs` (no change needed — uses `Node::Variable { span, .. }`)
- [x] Update `OffsetContext::Variable` in `crates/djls-ide/src/context.rs` to use `Vec<djls_templates::Filter>`
- [x] Update `TestNode::Variable` and `convert_nodelist_for_testing` in parser tests to serialize structured filters
- [x] Update all existing snapshot files affected by the filter format change (4 snapshots updated)
- [x] Add parser tests: pipe inside double quotes, pipe inside single quotes, colon inside quotes, whitespace around pipes, no whitespace, trailing pipe, empty between pipes, filter span accuracy, variable arg, numeric arg, complex chain
- [x] Add parser tests for escape sequences: escaped quote in double quotes, escaped quote in single quotes, escaped backslash, escaped pipe in quotes
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Filter Completions

**Goal:** Show filter completions in `{{ var|` context, scoped by `{% load %}` state.

- [x] Add `VariableClosingBrace` enum to `crates/djls-ide/src/completions.rs` (`None`/`Partial`/`Full`)
- [x] Update `TemplateCompletionContext::Filter` variant to include `partial: String` and `closing: VariableClosingBrace`
- [x] Implement `analyze_variable_context(prefix) -> Option<TemplateCompletionContext>` — detect `{{ var|` pattern, extract partial filter name
- [x] Wire `analyze_variable_context` into `analyze_template_context` (check variable context before tag context)
- [x] Add `AvailableFilters` struct and `available_filters_at()` function in `crates/djls-semantic/src/load_resolution.rs` — reuses `LoadState` from M3
- [x] Export `AvailableFilters` and `available_filters_at` from `crates/djls-semantic/src/lib.rs`
- [x] Implement `generate_filter_completions()` in `crates/djls-ide/src/completions.rs` — filters by partial match and availability, adds closing braces
- [x] Update `handle_completion` signature to accept `Option<&InspectorInventory>` (unified type for both tags and filters)
- [x] Wire `generate_filter_completions` into the `Filter` match arm
- [x] Update server completion call site in `crates/djls-server/src/server.rs` to pass inventory and load info for filters
- [x] Add completion tests: filter context detection after pipe, partial filter name, builtin filters always visible, scoped filters require load
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Filter Validation with Load Scoping

**Goal:** Add S111/S112/S113 diagnostics for unknown/unloaded/ambiguous filters.

- [x] Add `UnknownFilter { filter, span }`, `UnloadedLibraryFilter { filter, library, span }`, `AmbiguousUnloadedFilter { filter, libraries, span }` variants to `ValidationError` in `crates/djls-semantic/src/errors.rs`
- [x] Add S111/S112/S113 diagnostic codes in `crates/djls-ide/src/diagnostics.rs`
- [x] Add `FilterInventoryEntry` enum and `build_filter_inventory()` helper in `crates/djls-semantic/src/load_resolution.rs`
- [x] Implement `validate_filter_scoping(db, nodelist)` — iterates `Node::Variable` nodes, checks each `Filter` against inventory and load state
- [x] Wire `validate_filter_scoping` into `validate_nodelist` in `crates/djls-semantic/src/lib.rs` (after `validate_tag_scoping`)
- [x] Add validation tests: unknown filter produces S111, unloaded library filter produces S112, ambiguous filter produces S113, builtin filter always valid, filter valid after load
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M5 - Extraction Engine (`djls-extraction`)

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m5-extraction-engine.md` (overview), phases in `m5.1` through `m5.9`

### Phase 1: Create `djls-extraction` Crate with Ruff Parser

- [x] Choose a recent stable Ruff release tag, resolve to 40-char commit SHA
- [x] Add workspace dependencies in root `Cargo.toml`: `djls-extraction`, `ruff_python_parser`, `ruff_python_ast`, `ruff_text_size` (all pinned to SHA)
- [x] Create `crates/djls-extraction/` directory structure with all module files
- [x] Create `crates/djls-extraction/Cargo.toml` with Ruff parser deps + workspace deps (rustc-hash, serde, thiserror, tracing, insta)
- [x] Implement `crates/djls-extraction/src/types.rs` — `SymbolKey`, `ExtractionResult`, `ExtractedTag`, `ExtractedFilter`, `DecoratorKind`, `ExtractedRule`, `RuleCondition`, `ComparisonOp`, `BlockTagSpec`, `IntermediateTagSpec`, `FilterArity`
- [x] Implement `crates/djls-extraction/src/error.rs` — `ExtractionError` with `ParseError`, `UnsupportedSyntax`, `UnresolvedReference`
- [x] Implement `crates/djls-extraction/src/parser.rs` — `ParsedModule` wrapper around `ruff_python_parser::parse_module`
- [x] Create stub modules: `registry.rs`, `context.rs`, `rules.rs`, `structural.rs`, `filters.rs`, `patterns.rs` (empty or minimal)
- [x] Implement `crates/djls-extraction/src/lib.rs` — public API with `extract_rules()` calling parser→registry→rules→structural→filters
- [x] Run `cargo build -q -p djls-extraction`, `cargo clippy -q -p djls-extraction --all-targets --all-features -- -D warnings`

### Phase 2: Implement Registration Discovery

- [x] Implement `crates/djls-extraction/src/registry.rs` — `RegistrationInfo`, `FoundRegistrations`, `find_registrations()`, decorator analysis for `@register.tag`, `@register.simple_tag`, `@register.inclusion_tag`, `@register.simple_block_tag`, `@register.filter`
- [x] Handle bare decorators (`@register.tag`), call decorators (`@register.tag("name")`), keyword name (`name="custom"`)
- [x] Handle `simple_block_tag` `end_name` keyword extraction
- [x] Handle helper/wrapper decorators (e.g., `@register_simple_block_tag`)
- [x] Add `is_register_attribute()` to accept `register`, `lib`, `library`, `*register` names
- [x] Add unit tests: bare decorator, decorator with name, simple_block_tag kind, simple_block_tag with end_name, helper wrapper decorator
- [x] Run `cargo build -q -p djls-extraction`, `cargo clippy -q -p djls-extraction --all-targets --all-features -- -D warnings`, `cargo test -q -p djls-extraction`

### Phase 3: Implement Function Context Detection

- [x] Implement `crates/djls-extraction/src/context.rs` — `FunctionContext` with `split_var`, `parser_var`, `token_var` detection
- [x] Implement `find_split_contents_var()` — recurse into if/try to find `<var> = <token>.split_contents()`
- [x] Implement `is_split_contents_call()` — verify method name and optional token variable match
- [x] Add unit tests: detect `bits`, detect `args`, detect `parts`, no split_contents for simple_tag
- [x] Run `cargo build -q -p djls-extraction`, `cargo clippy -q -p djls-extraction --all-targets --all-features -- -D warnings`, `cargo test -q -p djls-extraction`

### Phase 4: Implement Rule Extraction

- [x] Implement `crates/djls-extraction/src/patterns.rs` — `is_len_call()`, `is_name()`, `extract_int_literal()`, `extract_string_literal()`, `extract_subscript_index()`, `extract_string_tuple()`
- [x] Implement `crates/djls-extraction/src/rules.rs` — `extract_tag_rules()` finding TemplateSyntaxError guards
- [x] Handle `len(<split>) == N`, `len(<split>) != N`, `len(<split>) < N`, `len(<split>) > N`, `len(<split>) >= N`, `len(<split>) <= N`
- [x] Handle reversed comparisons: `N <op> len(<split>)`
- [x] Handle `<split>[N] == "keyword"`, `<split>[N] != "keyword"`, `"keyword" in <split>`, `<split>[N] in ("opt1", "opt2")`
- [x] Handle `not` unary operator negation
- [x] Emit `RuleCondition::Opaque` for unrecognized conditions
- [x] Add unit tests: extraction with `bits`, `args`, `parts` variable names
- [x] Run `cargo build -q -p djls-extraction`, `cargo clippy -q -p djls-extraction --all-targets --all-features -- -D warnings`, `cargo test -q -p djls-extraction`

### Phase 5: Implement Block Spec Extraction (Control-Flow Based)

- [x] Implement `crates/djls-extraction/src/structural.rs` — `extract_block_spec()` with three inference strategies: singleton pattern, unique stop tag, Django convention fallback
- [x] Handle explicit `end_name` from decorator (highest priority)
- [x] Handle `simple_block_tag` Django semantic default (`end{function_name}`)
- [x] Implement `collect_parse_calls()` — recurse into if/while/for/try to find `parser.parse((...))` calls
- [x] Implement `infer_end_tag_from_control_flow()` — singleton → unique → convention → None
- [x] Detect opaque blocks (no compile_filter, no intermediates)
- [x] Add unit tests: singleton closer (if→endif), single stop tag, non-conventional closer, for with empty, no block spec for simple_tag, simple_block_tag with/without end_name, helper wrapper, ambiguous returns None, convention fallback, convention not invented, convention blocked by ambiguity
- [x] Run `cargo build -q -p djls-extraction`, `cargo clippy -q -p djls-extraction --all-targets --all-features -- -D warnings`, `cargo test -q -p djls-extraction`

### Phase 6: Implement Filter Arity Extraction

- [x] Implement `crates/djls-extraction/src/filters.rs` — `extract_filter_arity()` from function parameter count and defaults
- [x] Handle: 0-1 params → None, 2 params no default → Required, 2 params with default → Optional, vararg → Unknown
- [x] Add unit tests: no arg filter, required arg filter, optional arg filter
- [x] Run `cargo build -q -p djls-extraction`, `cargo clippy -q -p djls-extraction --all-targets --all-features -- -D warnings`, `cargo test -q -p djls-extraction`

### Phase 7: Salsa Integration

- [x] Add `parser` Cargo feature to `djls-extraction` — gate parsing behind feature, types always available
- [x] Add `djls-extraction` dependency to `djls-project` (without `parser` feature — types only)
- [x] Add `djls-extraction` dependency to `djls-server` (with `parser` feature)
- [x] Add `sys_path: Vec<Utf8PathBuf>` and `extracted_external_rules: FxHashMap<String, ExtractionResult>` fields to `Project` salsa input
- [x] Create `crates/djls-project/src/resolve.rs` — `resolve_module()`, `resolve_modules()`, `ModuleLocation`, `ResolvedModule`
- [x] Export resolve types from `crates/djls-project/src/lib.rs`
- [x] Add `#[salsa::tracked] fn extract_workspace_module_rules(db, file) -> ExtractionResult` in `djls-server/src/db.rs`
- [x] Add `#[salsa::tracked] fn collect_workspace_extraction_results(db, project) -> Vec<(String, ExtractionResult)>` in `djls-server/src/db.rs`
- [x] Update `refresh_inspector()` to: query sys_path, refresh inventory, extract external module rules
- [x] Update `compute_tag_specs()` to merge workspace + external extraction results + user overrides
- [x] Add `opaque: bool` and `extracted_rules: Vec<ExtractedRule>` fields to `TagSpec` in `djls-semantic`
- [x] Implement `TagSpec::merge_extracted_rules()`, `TagSpec::merge_block_spec()`, `TagSpec::from_extraction()`
- [x] Update all `Project::new()` call sites with new fields (`sys_path: Vec::new()`, `extracted_external_rules: FxHashMap::default()`)
- [x] Add module resolution unit tests with tempdir
- [x] Add `PythonEnvRequest`/`PythonEnvResponse` types in `djls-project` for `sys_path` query
- [x] Add `djls-extraction` dependency to `djls-semantic` (types only, for `TagSpec` fields)
- [x] Add invalidation tests: external rules change triggers recompute, external rules stable without setter, workspace extraction cached for unchanged file, revision bump triggers re-evaluation
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 8: Small Fixture Golden Tests (Tier 1)

- [x] Create `crates/djls-extraction/tests/fixtures/defaulttags_subset.py` — subset of Django defaulttags with `args`/`parts` variable names
- [x] Create `crates/djls-extraction/tests/golden.rs` — golden snapshot test, autoescape `args` variable test, for tag `parts` variable test, filter extraction test, block spec extraction test
- [x] Run `cargo test -q -p djls-extraction`, review snapshots with `cargo insta review`
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 9: Corpus / Full-Source Extraction Tests

- [x] Create `crates/djls-corpus/` crate with `manifest.toml`, sync logic, file enumeration
- [x] Add `.gitignore` entry for `crates/djls-corpus/.corpus/`
- [x] Add `corpus-sync` and `corpus-clean` just targets
- [x] Create `crates/djls-extraction/tests/corpus.rs` — no-panics test, yields test, no-hardcoded-bits test, Django versions golden test, unsupported patterns summary
- [x] Add parity oracle test (temporary — gated by `DJLS_PY_ORACLE=1`)
- [x] Add `walkdir` and `serde_json` dev-dependencies to `djls-extraction`
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M6 - Rule Evaluation + Expression Validation

**Status:** complete
**Plan:** `.agents/plans/2026-02-05-m6-rule-evaluation.md` (overview), phases in `m6.1` through `m6.2`

### Phase 1: Expression Validation Module

**Goal:** Port Django's `smartif.py` Pratt parser to Rust for validating `{% if %}`/`{% elif %}` expression syntax.

- [x] Create `crates/djls-semantic/src/if_expression.rs` with `IfToken`, `Operator`, `IfExpressionParser` types
- [x] Implement operator precedence table matching Django's `smartif.py`: `or(6)`, `and(7)`, `not(8)`, `in(9)`, `not in(9)`, `is(10)`, `is not(10)`, `==(10)`, `!=(10)`, `>(10)`, `>=(10)`, `<(10)`, `<=(10)`
- [x] Implement two-word operator tokenization: `"not" + "in"` → `"not in"`, `"is" + "not"` → `"is not"`
- [x] Implement Pratt parser `expression(rbp)` with prefix (`not`) and infix operator handling
- [x] Implement `validate_if_expression(bits: &[String]) -> Option<String>` public API — returns error message or None
- [x] Handle error cases: operator at start position, missing RHS, unused trailing tokens, empty expression
- [x] Add unit tests: valid expressions (simple, compound, not, not-in, is-not), invalid expressions (and at start, missing RHS, trailing token, empty)
- [x] Register module in `crates/djls-semantic/src/lib.rs` and export `validate_if_expression`
- [x] Run `cargo build -q -p djls-semantic`, `cargo clippy -q -p djls-semantic --all-targets --all-features -- -D warnings`, `cargo test -q -p djls-semantic`

### Phase 2: New Error Types (S114-S116)

**Goal:** Add `ValidationError` variants and diagnostic code mappings for expression and filter arity errors.

- [x] Add `ExpressionSyntaxError { tag, message, span }` variant to `ValidationError` in `crates/djls-semantic/src/errors.rs`
- [x] Add `FilterMissingArgument { filter, span }` variant to `ValidationError`
- [x] Add `FilterUnexpectedArgument { filter, span }` variant to `ValidationError`
- [x] Add S114 (ExpressionSyntaxError), S115 (FilterMissingArgument), S116 (FilterUnexpectedArgument) diagnostic code mappings in `crates/djls-ide/src/diagnostics.rs`
- [x] Add span extraction for S114/S115/S116 in `DiagnosticError::span()` impl
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 3: Extend Db Trait for M6 Queries

**Goal:** Add `filter_arity_specs()` and `opaque_tag_map()` methods to `djls-semantic::Db` trait and implement them in `djls-server`.

- [x] Add `FilterAritySpecs` struct (wraps `FxHashMap<SymbolKey, FilterArity>`) with `new()`, `get()`, `is_empty()` in `crates/djls-semantic/src/db.rs`
- [x] Add `OpaqueTagMap` type alias (`FxHashMap<String, String>`) in `crates/djls-semantic/src/db.rs`
- [x] Add `filter_arity_specs(&self) -> FilterAritySpecs` method to `Db` trait
- [x] Add `opaque_tag_map(&self) -> OpaqueTagMap` method to `Db` trait
- [x] Implement `#[salsa::tracked] fn compute_filter_arity_specs(db, project) -> FilterAritySpecs` in `crates/djls-server/src/db.rs` — collects from workspace + external extraction results
- [x] Implement `#[salsa::tracked] fn compute_opaque_tag_map(db, project) -> OpaqueTagMap` in `crates/djls-server/src/db.rs` — iterates `TagSpecs` for `opaque == true` entries with end tags
- [x] Implement `filter_arity_specs()` and `opaque_tag_map()` on `DjangoDatabase` in `crates/djls-server/src/db.rs`
- [x] Add `filter_arity_specs()` and `opaque_tag_map()` stubs to bench db (`crates/djls-bench/src/db.rs`) returning defaults
- [x] Add `filter_arity_specs()` and `opaque_tag_map()` stubs to all 4 test `TestDatabase` impls in `djls-semantic` (arguments.rs, blocks/tree.rs, semantic/forest.rs, load_resolution.rs) returning defaults
- [x] Export `FilterAritySpecs` and `OpaqueTagMap` from `crates/djls-semantic/src/lib.rs`
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 4: Opaque Regions and Validation Pipeline Wiring

**Goal:** Add opaque region detection, wire expression validation and filter arity validation into `validate_nodelist`.

- [x] Create `crates/djls-semantic/src/opaque.rs` with `OpaqueRegions` struct (holds `Vec<Span>`) and `is_opaque(span) -> bool` method
- [x] Implement `compute_opaque_regions(db, nodelist) -> OpaqueRegions` — scans nodelist for opener→closer pairs from `db.opaque_tag_map()`
- [x] Create `crates/djls-semantic/src/filter_arity.rs` with `validate_filter_arity(db, nodelist)` — iterates `Node::Variable` nodes, resolves filter symbols via `LoadState`, checks arity
- [x] Implement `resolve_filter_symbol(filter_name, position, loaded, db) -> Option<SymbolKey>` — handles builtin tiebreak (later in builtins list wins), library shadowing builtins, ambiguity returns None
- [x] Add `validate_if_expressions(db, nodelist)` function in `crates/djls-semantic/src/lib.rs` — iterates `Node::Tag` for `if`/`elif` names, calls `validate_if_expression(bits)`, emits S114
- [x] Update `validate_nodelist` to call new `validate_if_expressions` and `validate_filter_arity`
- [x] Update existing validation functions (`validate_all_tag_arguments`, `validate_tag_scoping`, `validate_filter_scoping`) to compute and respect opaque regions — skip nodes inside opaque regions (each function computes `OpaqueRegions` internally since Salsa tracked functions can't accept non-tracked parameters)
- [x] Register `opaque` and `filter_arity` modules in `crates/djls-semantic/src/lib.rs`
- [x] Export `OpaqueRegions`, `compute_opaque_regions`, `validate_filter_arity` from `crates/djls-semantic/src/lib.rs`
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

### Phase 5: Testing

**Goal:** Comprehensive tests for expression validation, filter arity, opaque regions, and no regressions.

- [x] Add expression parser unit tests in `if_expression.rs`: all operator combinations, complex chains (`x and y or not z`), comparison operators with not-in/is-not
- [x] Add opaque region unit tests in `opaque.rs`: basic containment, no opaque tags returns empty, nested opaque blocks, unclosed opaque block (no match)
- [x] Add filter arity integration tests: S115 for missing required arg, S116 for unexpected arg, Optional arity allows both, Unknown arity allows both
- [x] Add filter arity scoping tests: selective import resolves to correct arity, ambiguous library filters emit no arity error, library filter shadows builtin arity, builtin tiebreak (later module wins)
- [x] Add opaque region integration tests: no errors inside `{% verbatim %}...{% endverbatim %}`, errors after `{% endverbatim %}` still reported
- [x] Verify all existing S100-S113 tests still pass (no regressions)
- [x] Run `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`

---

## M7 - Documentation + Issue Reporting

**Status:** in-progress
**Plan:** `.agents/plans/2026-02-05-m7-docs-and-issue-template.md`

### Phase 1: Create Template Validation Documentation Page

- [x] Create `docs/template-validation.md` with sections: How Validation Works (Inspector + Extraction), What djls Validates (table of S108-S116), Filter Arity Validation note, What djls Cannot Validate (runtime-only, dynamic tag behavior), Inspector Availability (healthy vs unavailable), Ambiguous Symbols explanation, Reporting Mismatches link, See Also links
- [x] Update `.mkdocs.yml` nav to add "Template Validation: template-validation.md" under Configuration section
- [x] Verify documentation builds: `just docs build` or equivalent mkdocs command

### Phase 2: Update Diagnostic Codes Documentation

- [ ] Add tip admonition with link to template-validation.md in `docs/configuration/index.md` (after opening paragraph, before "## Options")
- [ ] Replace the "Available diagnostic codes" section in `docs/configuration/index.md` with expanded table format covering T-series (T100, T900, T901) and all S-series (S100-S116) grouped by category: Block Structure (S100-S107), Tag Scoping (S108-S110), Filter Scoping (S111-S113), Expression & Filter Arity (S114-S116) — each with Code, Error, Description, Typical Fix, and Suppression columns where appropriate
- [ ] Verify documentation builds without errors

### Phase 3: Create GitHub Issue Template Directory

- [ ] Create `.github/ISSUE_TEMPLATE/config.yml` with `blank_issues_enabled: true` and contact links for documentation and discussions
- [ ] Validate YAML syntax

### Phase 4: Create Template Validation Mismatch Issue Form

- [ ] Create `.github/ISSUE_TEMPLATE/template-validation-mismatch.yml` with form fields: mismatch type dropdown, template snippet, expected behavior, djls diagnostic, version info (djls, Django, Python), OS dropdown, inspector health dropdown, server logs, installed apps, tagspecs, additional context, and required checklist
- [ ] Validate YAML syntax

### Phase 5: Update TagSpecs Documentation

- [ ] Add validation mismatch link in `docs/configuration/tagspecs.md` after the existing "open an issue" text — points to the new template-validation-mismatch issue form
- [ ] Verify documentation builds without errors

---

## Discoveries / Notes

- M1: `TemplateTags` no longer implements `Deref<Target=Vec<TemplateTag>>`. Use `.iter()`, `.tags()`, `.len()`, `.is_empty()` instead.
- M1: `TemplateTag` no longer has `.module()`. Use `.defining_module()` (where function is defined), `.registration_module()` (library/builtin module), or `.library_load_name()` (load name for `{% load %}`).
- M1: Clippy requires `#[must_use]` on all public accessors and constructors in this project.
- M1: `TemplateTag` and `TagProvenance` are now exported from `djls-project`.
- M2: Salsa `#[salsa::tracked]` functions require `&dyn Trait` parameters, not concrete types. Used `&dyn SemanticDb` for `compute_tag_specs`/`compute_tag_index`.
- M2: `TagSpecs` needed `PartialEq` derive for use as Salsa tracked return value (Salsa requires equality comparison for memoization).
- M2: Salsa input setters require `use salsa::Setter` trait import — the `.to()` method is a trait method.
- M2: `set_settings` signature changed from `Settings` to `&Settings` — clippy flags needless pass by value. Updated callers in `session.rs` and `server.rs`.
- M2: Exported `inspector_query` (re-export of `inspector::query`) from `djls-project` for direct inspector access outside tracked queries.
- M2: Salsa `ingredient_debug_name()` returns the function name (e.g., `"compute_tag_specs"`) — use this in `WillExecute` event matching for stable invalidation tests (not Debug format strings).
- M4: `InspectorInventory` replaces `TemplateTags` as the Project input field type. `TemplateTags` still exists but is only used by the legacy `templatetags` tracked query. All downstream code (completions, load resolution, semantic db trait) now uses `InspectorInventory`.
- M4: `refresh_inspector()` now uses the unified `TemplateInventoryRequest` ("template_inventory" query) which returns both tags and filters in a single IPC round trip.
- M4: Server completion handler reads from `db.inspector_inventory()` (SemanticDb trait method) instead of calling `djls_project::templatetags()` directly.
- M4: `Node::Variable { filters }` changed from `Vec<String>` to `Vec<Filter>`. `Filter` has `name: String`, `arg: Option<FilterArg>`, `span: Span`. The `VariableScanner` is quote-aware — pipes inside `'...'` or `"..."` are not treated as filter separators.
- M4: `blocks/builder.rs` only pattern-matches `Node::Variable { span, .. }` — no code change needed for the filter type change.
- M5: `extract_name_from_call` must be decorator-kind-aware: only `@register.tag("name")` and `@register.filter("name")` use first positional arg as the tag/filter name. For `inclusion_tag`, the first positional is the template path; for `simple_tag`/`simple_block_tag`, there's no positional name. All types support `name="custom"` keyword.
- M5: `RegistrationInfo` fields `function_name`, `offset`, `explicit_end_name` are not yet consumed by downstream stubs (context, rules, structural, filters) — `#[allow(dead_code)]` on the struct until those phases implement.
- M5: Ruff AST `Parameters` has no `defaults` field — defaults are inline on each `ParameterWithDefault` as `default: Option<Box<Expr>>`. Check `arg.default.is_some()` instead of `params.defaults.is_empty()`.
- M5.7: `djls-extraction` parser feature gating: Ruff parser deps are optional, types (`ExtractionResult`, etc.) always available. `djls-project` uses `default-features = false`, `djls-server` uses default (parser enabled).
- M5.7: `Vec::new()` IS const-stable — used in static `TagSpec` definitions for `extracted_rules` field with no issues.
- M5.7: Module resolution tests must use separate temp dirs for workspace vs external paths — everything under the same temp dir gets classified as Workspace.
- M5.7: `InMemoryFileSystem` only has `add_file(&mut self, ...)` — can't write through `Arc<dyn FileSystem>`. Extraction invalidation tests use Salsa input setters instead of filesystem writes.
- M5.7: Salsa early cutoff: if `file.source(db)` returns the same value after revision bump, `extract_workspace_module_rules` correctly skips re-execution.
- M5.9: `djls-corpus` crate uses `reqwest` (blocking + json), `flate2`, `tar` for downloading and extracting PyPI sdists and GitHub tarballs. No `zip` crate needed — all sources are `.tar.gz`.
- M5.9: Corpus tests in `djls-extraction/tests/corpus.rs` skip gracefully when corpus is not synced — they check for corpus root existence and return early with a message.
- M6: Salsa `#[salsa::tracked]` functions cannot accept non-tracked parameters like `&OpaqueRegions`. Each validation function (`validate_all_tag_arguments`, `validate_tag_scoping`, `validate_filter_scoping`, `validate_if_expressions`, `validate_filter_arity`) computes opaque regions internally via `compute_opaque_regions(db, nodelist)`.
- M6: `LoadState` in `load_resolution.rs` changed from private to `pub(crate)` so `filter_arity.rs` can reuse the same load-scoping state machine for filter symbol resolution.
- M6: The arguments test `test_expr_stops_at_literal` uses a contrived `{% if x > 0 reversed %}` which now triggers S114 (expression syntax error). The test filters out `ExpressionSyntaxError` to only check argument-spec validation.
