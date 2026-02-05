---
date: 2026-02-04T18:30:00-06:00
query: "Points of contact research for porting template_linter to djls"
repository: https://github.com/joshuadavidthomas/django-language-server
branch: main
commit: 4d18e6e
cwd: /home/josh/projects/joshuadavidthomas/django-language-server
tags: [template-linter, porting, integration, tagspecs, filters, load-scoping, salsa]
---

# Integration Seams: template_linter → djls

## Executive Summary

This document identifies the concrete integration points between the Python `template_linter/` prototype and the Rust `djls` codebase. The goal is to map rule bundles, load scoping, and validation logic to existing djls concepts without full redesign.

**Key finding:** The porting effort has 6 major integration seams:
1. **TagSpecs replacement** - extracted rules replace hardcoded builtins
2. **Inspector enhancement** - add library name + builtin distinction
3. **Load scoping** - track `{% load %}` for tag/filter availability  
4. **Filter inventory** - new data path (currently non-existent in djls)
5. **Salsa boundaries** - fix untracked dependencies on TagSpecs
6. **Block structure** - BlockTagSpec maps to TagIndex/TagSpec.end_tag

---

## 1. TagSpecs Entry & Consumption Points in djls

### Where TagSpecs are Defined

| Location | Description |
|----------|-------------|
| `crates/djls-semantic/src/templatetags/builtins.rs:395-412` | `BUILTIN_SPECS: LazyLock<TagSpecs>` - hardcoded Django tags (~35 tags) |
| `crates/djls-semantic/src/templatetags/builtins.rs:418-420` | `django_builtin_specs()` - public accessor |
| `crates/djls-semantic/src/templatetags/specs.rs:63-65` | `TagSpecs` newtype over `FxHashMap<String, TagSpec>` |
| `crates/djls-semantic/src/templatetags/specs.rs:196-213` | `TagSpec` struct: `module`, `end_tag`, `intermediate_tags`, `args` |
| `crates/djls-conf/src/tagspecs.rs:11-180` | TOML config types: `TagSpecDef`, `TagDef`, `EndTagDef`, etc. |

### Where TagSpecs are Loaded/Merged

| Location | Code | Description |
|----------|------|-------------|
| `crates/djls-semantic/src/templatetags/specs.rs:177-199` | `impl From<&Settings> for TagSpecs` | Merges builtins + user TOML config |
| `crates/djls-server/src/db.rs:187-189` | `fn tag_specs(&self) -> TagSpecs` | Concrete Db impl reads from settings |

### Where TagSpecs are Consumed

| Location | Consumer | Why It Matters |
|----------|----------|----------------|
| `crates/djls-semantic/src/blocks.rs:21` | `build_block_tree()` | Uses `db.tag_index()` for block classification |
| `crates/djls-semantic/src/blocks/grammar.rs:108-142` | `TagIndex::from_specs()` | Builds opener/closer/intermediate lookup tables |
| `crates/djls-semantic/src/blocks/builder.rs:113-162` | `BlockTreeBuilder::handle_tag()` | Classifies tags during tree construction |
| `crates/djls-semantic/src/arguments.rs:40-56` | `validate_tag_arguments()` | Validates arg counts/types against specs |
| `crates/djls-server/src/server.rs:278` | Completion handler | Passes specs to IDE completion logic |
| `crates/djls-ide/src/completions.rs:291-345` | `generate_tag_name_completions()` | Uses specs for end-tag and snippet generation |
| `crates/djls-ide/src/completions.rs:409-470` | `generate_argument_completions()` | Position-aware arg completions from specs |
| `crates/djls-ide/src/snippets.rs:6-92` | Snippet generation | Converts `TagArg` specs to LSP snippets |

### Diagnostics Emitted from TagSpecs

| Code | Error | Source Location |
|------|-------|-----------------|
| S100 | UnclosedTag | `blocks/builder.rs:283-287` via `finish()` |
| S101 | UnbalancedStructure | `blocks/builder.rs:186-193` via `close_block()` |
| S102 | OrphanedTag | `blocks/builder.rs:274-280` via `add_intermediate()` |
| S103 | UnmatchedBlockName | `blocks/builder.rs:204-208` via `close_block()` |
| S104 | MissingRequiredArguments | `arguments.rs:82-85, 200-204` |
| S105 | TooManyArguments | `arguments.rs:73-77, 253-260` |
| S106 | InvalidLiteralArgument | `arguments.rs:147-153` |
| S107 | InvalidArgumentChoice | `arguments.rs:167-178` |

---

## 2. Runtime Template Introspection Data Flow

### Python Side (Inspector)

| Location | Function | Returns |
|----------|----------|---------|
| `crates/djls-project/inspector/queries.py:101-135` | `get_installed_templatetags()` | `TemplateTagQueryData` with `Vec<TemplateTag>` |
| `queries.py:89-92` | `TemplateTag` dataclass | `{name, module, doc}` |

**Current gaps:**
- `queries.py:125` iterates `engine.libraries.values()` **losing library name keys**
- No `is_builtin` field to distinguish `engine.template_builtins` from `engine.libraries`
- No filter data collected (only tags)

### Rust Side (Consumers)

| Location | Type/Function | Description |
|----------|---------------|-------------|
| `crates/djls-project/src/django.rs:88-96` | `templatetags()` | Salsa-tracked query returning `Option<TemplateTags>` |
| `crates/djls-project/src/django.rs:98-105` | `TemplateTags` | Newtype over `Vec<TemplateTag>` |
| `crates/djls-project/src/django.rs:107-122` | `TemplateTag` | Struct with `name`, `module`, `doc` |
| `crates/djls-ide/src/completions.rs:527-564` | `generate_library_completions()` | **BUG:** Uses `tag.module()` for `{% load %}` completions |

### The Library Name Bug

**Problem:** `{% load %}` completions show Python module paths instead of library names.

| Stage | What Happens | Example |
|-------|--------------|---------|
| Python `queries.py:125` | Iterates `.values()` not `.items()` | Loses `"static"` key |
| Python `queries.py:131` | Stores `tag_func.__module__` | `"django.templatetags.static"` |
| Rust `completions.rs:536` | `libraries.insert(tag.module())` | Shows module path in completion |

**Fix required in inspector:** Return both library name (from dict key) and module path.

---

## 3. `{% load %}` Library Resolution

### Current djls State

| Location | Current Behavior |
|----------|------------------|
| `crates/djls-templates/src/nodelist.rs:14-19` | `{% load %}` parsed as generic `Node::Tag { name: "load", bits: [...] }` |
| `crates/djls-semantic/src/templatetags/builtins.rs:232-241` | TagSpec for load: `VarArgs { name: "libraries" }` - no actual resolution |
| `crates/djls-semantic/src/arguments.rs:59` | Unknown tags pass silently |
| `crates/djls-ide/src/completions.rs:651-667` | Library completions use wrong identifier (module path) |

**No library loading is modeled.** Tags from third-party libraries show even if not loaded.

### template_linter Load Resolution

| Location | Component | Description |
|----------|-----------|-------------|
| `resolution/load.py:27-38` | `LibraryModule` | Dataclass: `{name, path, bundle}` |
| `resolution/load.py:46-56` | `LibraryIndex` | Maps library names → candidate modules |
| `resolution/load.py:174-265` | `resolve_load_tokens()` | Parses `{% load %}` syntax, returns resolved bundle |
| `resolution/load.py:92-117` | `build_library_index_from_modules()` | Converts `{lib_name: module_path}` to index |
| `resolution/runtime_registry.py:24-52` | `RuntimeRegistry` | Dataclass: `{libraries: dict, builtins: list}` |
| `resolution/runtime_registry.py:55-86` | `build_runtime_environment()` | Returns `(LibraryIndex, builtins_bundle)` |

### Key Concepts to Port

1. **Builtin vs Library distinction:** `engine.template_builtins` (always available) vs `engine.libraries` (require `{% load %}`)
2. **Library name mapping:** Inspector must return `{load_name: module_path}` dict, not just module paths
3. **Per-template load tracking:** Track which libraries are loaded at each position in template
4. **Collision policy:** "later wins" for `{% load %}` (Django semantics)

---

## 4. Rule Bundle Mapping: template_linter → djls

### template_linter Types

| Type | Location | Maps to djls |
|------|----------|--------------|
| `TagValidation` | `types.py:166-188` | → `TagSpec` (partially) |
| `ExtractedRule` | `types.py:137-162` | → `TagArg` validation logic |
| `ContextualRule` | `types.py:163-183` | → Preconditioned validation (NEW) |
| `ParseBitsSpec` | `types.py:95-104` | → `TagArg` signature for simple_tag |
| `BlockTagSpec` | `types.py:117-133` | → `TagSpec.end_tag` + `intermediate_tags` |
| `OpaqueBlockSpec` | `types.py:107-114` | → NEW (verbatim/comment handling) |
| `FilterSpec` | `extraction/filters.py:15-21` | → NEW (no filter validation today) |
| `ConditionalInnerTagRule` | `types.py:107-115` | → NEW (structural constraints) |

### Extraction Pipeline

| template_linter | djls Equivalent | Integration Point |
|-----------------|-----------------|-------------------|
| `extraction/api.py:14-31` `extract_from_file()` | N/A (manual builtins.rs) | Replace with extracted data |
| `extraction/rules.py` `RuleExtractor` | N/A | Port AST visitor to Rust (ruff_python_parser) |
| `extraction/structural.py` `extract_block_specs_from_file()` | `TagIndex` construction | Feed into `TagIndex::from_specs()` |
| `extraction/filters.py` `FilterExtractor` | N/A | New filter validation system |

### Validation Pipeline

| template_linter | djls Equivalent | Notes |
|-----------------|-----------------|-------|
| `validation/tags.py` | `djls-semantic/src/arguments.rs` | Richer rule evaluation needed |
| `validation/structural.py` | `djls-semantic/src/blocks/builder.rs` | Already handles structure |
| `validation/filters.py` | N/A | New system needed |
| `validation/if_expression.py` | N/A | Expression syntax validation (NEW) |

---

## 5. Salsa Boundaries & Invalidation

### Current Salsa Inputs

| Input | Location | Fields |
|-------|----------|--------|
| `File` | `djls-source/src/file.rs:10-17` | `path`, `revision` |
| `Project` | `djls-project/src/project.rs:14-28` | `root`, `interpreter`, `django_settings_module`, `pythonpath` |

### Tracked Functions for Templates

| Function | Location | Dependencies |
|----------|----------|--------------|
| `parse_template` | `djls-templates/src/lib.rs:74-92` | `File` |
| `validate_nodelist` | `djls-semantic/src/lib.rs:42-52` | `NodeList` |
| `build_block_tree` | `djls-semantic/src/blocks.rs:16-22` | `NodeList` + **untracked** `tag_index()` |
| `build_semantic_forest` | `djls-semantic/src/semantic.rs:10-22` | `BlockTree`, `NodeList` |

### Untracked Dependencies (BUG)

| Location | Call | Problem |
|----------|------|---------|
| `blocks.rs:21` | `db.tag_index()` | Inside tracked fn, not recorded as dependency |
| `arguments.rs:42` | `db.tag_specs()` | Inside tracked fn, not recorded as dependency |
| `grammar.rs:122` | `db.tag_specs()` | Inside `TagIndex::from_specs()` |

**Consequence:** Settings changes (new TagSpecs) don't invalidate cached validation results.

### Settings Storage (Outside Salsa)

| Location | Storage |
|----------|---------|
| `djls-server/src/db.rs:46` | `settings: Arc<Mutex<Settings>>` |
| `djls-server/src/db.rs:187-189` | `fn tag_specs(&self) -> TagSpecs { TagSpecs::from(&self.settings()) }` |

### What Needs to Become Salsa Inputs

| New Input | Why |
|-----------|-----|
| `TagSpecsInput` | So validation recomputes when specs change |
| `RuntimeRegistryInput` | So load resolution updates when Django env changes |
| `FilterSpecsInput` | For filter validation caching |

### Fix Approaches

1. **Make TagSpecs a Salsa input:** Pass `TagSpecsInput` through tracked functions
2. **Bump file revisions on config change:** Force revalidation of all open files
3. **Hash specs into revision:** Include spec checksum in file identity

---

## 6. Filters: Current State & Integration Points

### djls Current State

| Aspect | Status | Location |
|--------|--------|----------|
| Filter parsing | Minimal | `parser.rs:182-202` splits on `\|` into `Vec<String>` |
| Filter AST | Flat | `nodelist.rs:27-31` `Variable { var, filters: Vec<String> }` |
| Filter validation | None | No `FilterSpec` or validation logic |
| Filter completions | Stub | `completions.rs:67-75` has TODO enum variants |
| Filter inspector | None | `queries.py` only collects tags, not filters |

### template_linter Filter System

| Component | Location | What It Does |
|-----------|----------|--------------|
| `FilterSpec` | `extraction/filters.py:15-21` | `{name, pos_args, defaults, unrestricted}` |
| `FilterExtractor` | `extraction/filters.py:24-86` | AST visitor for `@register.filter` |
| `_parse_filter_chain()` | `template_syntax/filter_syntax.py:46-60` | Quote-aware `{{ x\|f:arg }}` parsing |
| `_validate_filter_chain()` | `validation/filters.py:21-43` | Arg count checking |
| `validate_filters()` | `validation/filters.py:46-82` | Template-wide filter validation |

### Integration Points

| djls Location | Change Needed |
|---------------|---------------|
| `queries.py` | Add `get_installed_filters()` returning `{name, module, doc, arity}` |
| `django.rs` | Add `TemplateFilters` type and `templatefilters()` tracked function |
| `parser.rs:182-202` | Parse filters into structured `{name, arg}` tuples |
| `nodelist.rs:27-31` | Change `filters: Vec<String>` to `Vec<Filter>` with spans |
| `djls-semantic/` | Add `FilterSpecs` and `validate_filters()` |
| `completions.rs` | Implement `Filter` context detection and completion |

---

## 7. Test Locations for Updates/Additions

### Existing Tests by Crate

| Crate | Test File | What It Tests | Insta? |
|-------|-----------|---------------|--------|
| `djls-semantic` | `arguments.rs` | Tag argument validation (15+ tests) | No |
| `djls-semantic` | `blocks/tree.rs` | Block tree construction | Yes |
| `djls-semantic` | `templatetags/specs.rs` | TagSpec operations (10+ tests) | No |
| `djls-semantic` | `templatetags/builtins.rs` | Built-in specs (6+ tests) | No |
| `djls-semantic` | `semantic/forest.rs` | Semantic forest | Yes |
| `djls-templates` | `lexer.rs` | Tokenization (9 tests) | Yes |
| `djls-templates` | `parser.rs` | Full parsing (25+ tests) | Yes |
| `djls-ide` | `completions.rs` | Completion logic (15+ tests) | No |
| `djls-ide` | `snippets.rs` | Snippet generation (8 tests) | No |
| `djls-ide` | `context.rs` | Offset context (10+ tests) | No |

### Snapshot Directories

- `crates/djls-semantic/src/blocks/snapshots/` - 2 snapshots
- `crates/djls-semantic/src/semantic/snapshots/` - 4 snapshots
- `crates/djls-templates/src/snapshots/` - 42 snapshots

### Where to Add New Tests

| Integration Seam | Best Test Location | Test Type |
|------------------|-------------------|-----------|
| Extracted TagSpecs loading | `templatetags/specs.rs` | Unit tests for `From<ExtractedBundle>` |
| Block structure from BlockTagSpec | `blocks/tree.rs` | Insta snapshots |
| Argument validation from rules | `arguments.rs` | Unit tests per rule type |
| Filter validation | NEW `filters.rs` in `djls-semantic` | Unit tests |
| Load scoping | NEW `load_resolution.rs` | Unit tests + integration |
| Library completions fix | `completions.rs` | Unit tests |
| Inspector enhancements | `django.rs` in `djls-project` | Unit tests with mock data |

---

## template_linter Key Modules Reference

### Extraction

| Module | Key Classes/Functions | Purpose |
|--------|----------------------|---------|
| `extraction/api.py` | `extract_from_file()`, `extract_from_django()` | Entry points |
| `extraction/rules.py` | `RuleExtractor`, `extract_rule()` | AST → validation rules |
| `extraction/structural.py` | `extract_block_specs_from_file()` | Block structure extraction |
| `extraction/filters.py` | `FilterExtractor`, `FilterSpec` | Filter signature extraction |
| `extraction/registry.py` | `collect_registered_tags()` | Find `@register.tag` decorators |
| `extraction/opaque.py` | Opaque block detection | verbatim/comment handling |

### Resolution

| Module | Key Classes/Functions | Purpose |
|--------|----------------------|---------|
| `resolution/load.py` | `LibraryModule`, `LibraryIndex`, `resolve_load_tokens()` | `{% load %}` resolution |
| `resolution/bundle.py` | `ExtractionBundle`, `merge_bundles()` | Rule aggregation |
| `resolution/runtime_registry.py` | `RuntimeRegistry`, `build_runtime_environment()` | Django Engine state |
| `resolution/module_paths.py` | `resolve_module_to_path()` | Dotted path → filesystem |
| `resolution/compat.py` | `LEGACY_UNRESTRICTED_TAGS` | Removed tag stubs |

### Validation

| Module | Key Functions | Purpose |
|--------|---------------|---------|
| `validation/tags.py` | Tag rule validation | Apply extracted rules |
| `validation/filters.py` | `validate_filters()` | Filter arg checking |
| `validation/structural.py` | Block nesting validation | Delimiter ordering |
| `validation/template.py` | `validate_template_with_load_resolution()` | Full template validation |
| `validation/if_expression.py` | Expression parsing | `{% if %}` syntax |

### Types

| Type | Location | djls Mapping |
|------|----------|--------------|
| `TagValidation` | `types.py:166-188` | `TagSpec` + validation rules |
| `ExtractedRule` | `types.py:137-162` | Validation logic |
| `ContextualRule` | `types.py:163-183` | Preconditioned rules (NEW) |
| `ParseBitsSpec` | `types.py:95-104` | `TagArg` for simple_tag |
| `BlockTagSpec` | `types.py:117-133` | `TagSpec.end_tag` + intermediates |
| `OpaqueBlockSpec` | `types.py:107-114` | NEW (skip content parsing) |
| `FilterSpec` | `filters.py:15-21` | NEW filter type |
| `TokenView` | `types.py:50-58` | Internal extraction state |
| `TokenEnv` | `types.py:61-69` | Variable tracking during extraction |

---

## Recommended Porting Order

1. **Inspector enhancement** - Add `template_registry` query with library names + builtins
2. **Fix library completion bug** - Use library name not module path
3. **Filter data path** - Add `templatefilters` query and types
4. **Load scoping infrastructure** - Track loaded libraries per-template
5. **Salsa input for TagSpecs** - Fix invalidation bug
6. **Import extracted rules** - Replace `builtins.rs` with JSON/generated data
7. **Port rule evaluation** - Handle `ContextualRule` preconditions
8. **Filter validation** - New validation pass
9. **Expression validation** - `{% if %}` syntax checking

---

## Open Questions

1. **Code generation vs runtime JSON?** For extracted TagSpecs, generate Rust code or load JSON at startup?
2. **Salsa complexity?** Making TagSpecs a Salsa input adds parameters to many functions. Alternative: bump all file revisions on config change?
3. **Filter scoping semantics?** template_linter uses "final filter set" not position-aware. Match this or improve?
4. **Third-party library extraction?** Run extraction as build step, or ship pre-extracted specs for common libraries?
