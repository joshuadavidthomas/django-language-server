# Template Validation Port Charter

**Date:** 2026-02-05  
**Status:** Draft  
**Scope:** Porting `template_linter/` capabilities to Rust (`django-language-server`)

---

## 1. Desired End State

When "done," djls provides **runtime-aware, load-scoped template validation** derived from actual Python source—not hand-authored specs.

### 1.1 Runtime-Aware Tag/Filter Inventory

- **Tags and filters discovered from Django/third-party source**, not hardcoded in `builtins.rs`
- **Correct identifiers**: library load-name (e.g., `i18n`) vs module path (e.g., `django.templatetags.i18n`)
- **Filter inventory exists**: djls validates filter usage and argument counts (currently missing entirely)

**Separation of concerns:**
| Component | Responsibility |
|-----------|----------------|
| **Python inspector** | **Inventory** - authoritative runtime truth: which tags/filters exist, which library they belong to, builtins vs `{% load %}` required. Django is the authority on what's registered. |
| **Rust AST extraction** | **Rules/validation** - enriches inventory with validation semantics: TemplateSyntaxError constraints, block structure, expression grammar, filter arity (as best-effort enrichment, not inventory). |

**Inspector payload preserves Django's registry structure:**
- `engine.libraries` mapping (`{load_name: module_path}`) with keys preserved
- Ordered builtin module paths (from `engine.template_builtins` / configured `OPTIONS["builtins"]`)
- Tag/filter inventory grouped by library load-name (for `{% load %}` libraries) and by builtin module (for builtins)
- Environment/path info to resolve module paths to source files
- Django's "later wins" collision semantics are derivable from this data

**Inventory item data model:** Each tag/filter in the inventory must carry:
- `name` — the tag/filter name as used in templates
- `provenance` — **exactly one of**:
  - `Library { load_name, module }` — requires `{% load X %}`, e.g., `Library { load_name: "i18n", module: "django.templatetags.i18n" }`
  - `Builtin { module }` — always available, e.g., `Builtin { module: "django.template.defaulttags" }`
- `defining_module` — the module where the function is defined (`tag_func.__module__`); needed for docs/jump-to-def

The `provenance` field is mutually exclusive (either library or builtin, never both). For both variants, the `module` field indicates where registration happens (needed for rule extraction). This prevents reintroducing the "module path == library name" bug and makes downstream Rust types simpler (enum).

**Clarification on Django's "builtins" concepts:**
| Django Concept | Type | What It Is |
|----------------|------|------------|
| `engine.libraries` | `dict[str, str]` | Load-name → module path mapping |
| `engine.template_builtins` | `list[Library]` | Runtime Library objects, ordered |
| `TEMPLATES[...]["OPTIONS"]["builtins"]` | `list[str]` | Configured builtin module paths |

What we need to transport: **ordered builtin module paths** (ordering = semantics for "later wins") plus the **libraries mapping with keys intact**.

### 1.2 `{% load %}` Scoping Correctness

- **Diagnostics respect load scope**: A tag/filter from library `foo` produces an error if `{% load foo %}` hasn't preceded it
- **Completions respect load scope**: Tag/filter completions only show symbols available at cursor position (builtins + loaded libraries)
- **`{% load X from Y %}` handled**: Selective imports correctly scope only the named symbols
- **Builtin vs library distinction preserved**: Tags from ordered builtins (from `template_builtins` + configured builtins) available without `{% load %}`

**Unknown tag/filter behavior:**
- **Pre-M3 (current)**: Unknown tags pass silently; no scoping validation
- **Post-M3 (inspector healthy)**: Unknown tags/filters produce diagnostics **by default** (not behind a flag)
- **Inspector unavailable/Django init fails**: Downgrade unknown diagnostics to warning/info severity or suppress entirely to avoid noisy false positives
- **Truly unknown**: A tag/filter not in the inspector inventory at all is an error: "Unknown tag `{% xyz %}`" / "Unknown filter `|xyz`"

**Collision handling:**
- **With inspector**: No ambiguity at runtime—Django resolves `{% load foo %}` to a single module via `engine.libraries[foo]`. If Django detected a collision (W003-style warning), djls may surface that as a warning, but still treats the resolved `engine.libraries[foo]` mapping as authoritative for validation/completions.
- **Without inspector (corpus/filesystem-scan mode)**: When multiple libraries define the same name and no registry ordering is available, djls emits a warning ("Cannot resolve `foo`: defined in multiple libraries") and skips validation for that symbol rather than guessing.

### 1.3 Semantic Validation from Python Source

- **Tag validation rules extracted from AST**, not hand-authored:
  - Argument count/position constraints from `do_xxx()` functions
  - Keyword requirements (e.g., `bits[2] != "as"`)
  - Option validation (known options, duplicates, required args)
  - `parse_bits` signatures for `simple_tag`/`inclusion_tag`
- **Block structure extracted from AST**:
  - End tags and intermediate tags from `parser.parse((...))`
  - Dynamic patterns like `f"end{tag_name}"` handled
  - Opaque blocks (verbatim/comment-like) honored
- **Filter specs extracted from AST**: Argument counts from `@register.filter` signatures
- **Expression validation for `{% if %}`/`{% elif %}`**: Operator/operand syntax checking

### 1.4 Salsa/Caching Correctness

- **No stale diagnostics**: When extracted rules, runtime registry, or config changes, validation results invalidate
- **TagSpecs as Salsa input**: Changes to specs trigger recomputation of dependent queries
- **Registry changes propagate**: Inspector refresh (e.g., new Django version, new libraries installed) invalidates affected caches

---

## 2. Explicit Non-Goals

| Non-Goal | Rationale |
|----------|-----------|
| **Runtime template execution** | Static analysis only; never render templates |
| **Import/execute tag/filter code** | All extraction from AST; no Python evaluation of tag logic |
| **Variable type checking** | Requires runtime context (request, db, template context) |
| **Render-time error detection** | Type coercion, format strings, missing partials—all runtime |
| **Template inheritance resolution** | `{% extends %}` / `{% include %}` resolution is future work |
| **Full Python semantic analysis** | Extract what's statically provable; don't build a Python type checker |
| **Cross-template state** | `cycle` names, `partialdef` tracking deferred to later phase |
| **Parity with Python prototype test suite** | Rust tests should verify behavior; don't port Python test code |

---

## 3. Milestones (Vertical Slices)

Each milestone delivers user-visible value and can be shipped independently.

**Ordering rationale:** Salsa invalidation (M2) comes early because load scoping (M3) and filters (M4) depend on inspector data and evolving rule sources. Building on top of broken caching amplifies bugs. Filters (M4) come after scoping because they reuse the same infrastructure. Extraction (M5+) comes last because it's the largest lift and benefits from stable foundations.

### M1: Payload Shape + Library Name Fix

**User-visible outcome:**
- `{% load %}` completions show correct library names (`i18n`, `static`) instead of module paths
- Inspector returns builtins vs libraries distinction
- Tag inventory comes from Django (authoritative runtime truth)

**Main system boundary touched:**
- Inspector payload: expand `templatetags` to preserve library-name keys + ordered builtins (don't drop keys when iterating `engine.libraries`)
- Inspector reports tag inventory per library (Django is authoritative on what's registered)
- Rust types: ensure `TemplateTag` carries library load-name
- Completions: use library name, not module path

**How we verify:**
- Unit test: Library name available as distinct field, not derived from module path
- Integration test: LSP completion request for `{% load |` returns library names
- Manual test: Completions in editor show correct identifiers

---

### M2: Salsa Invalidation Plumbing

**User-visible outcome:**
- Changing config triggers revalidation without reopening files
- Inspector refresh (e.g., after `pip install`) updates diagnostics
- Future: extracted rule changes propagate correctly

**Main system boundary touched:**
- `crates/djls-server/src/db.rs` — fix untracked dependencies on `tag_specs()`/`tag_index()`
- Options: explicit Salsa input for specs, OR bump file revisions on config change (coarser but simpler)
- Goal: establish the invalidation contract before building features that depend on it

**Invalidation must cover all rule/registry sources:**
1. Settings/config (djls.toml, pyproject.toml)
2. Inspector registry payload (libraries mapping, ordered builtins)
3. Extracted bundle/ruleset (once extraction exists)

**How we verify:**
- Test: Change config → file's cached validation invalidated
- Test: Simulate inspector refresh → diagnostics recompute
- Test: (Future) Extracted rules change → validation recomputes
- No regression: Performance benchmark shows acceptable overhead

**Why this comes early:** Load scoping, filters, and extraction will add new data sources. If invalidation is broken, those features build on a lie.

---

### M3: Load Scoping Infrastructure

**User-visible outcome:**
- Diagnostic: "Tag `{% trans %}` requires `{% load i18n %}`"
- Completions for tags respect which libraries are loaded at cursor position
- Builtins available without `{% load %}`

**Main system boundary touched:**
- New module: `crates/djls-semantic/src/load_resolution.rs`
- Per-template library tracking in semantic analysis
- Position-aware tag availability in completions

**How we verify:**
- Unit test: Tag before `{% load %}` → diagnostic; after → no diagnostic
- Unit test: `{% load trans from i18n %}` scopes only `trans`, not all i18n tags
- Integration test: Completions at position before load differ from after load

---

### M4: Filters Pipeline

**User-visible outcome:**
- Filter completions appear in `{{ var|` context
- Basic filter validation (unknown filter = diagnostic)
- Filter scoping respects `{% load %}`

**Main system boundary touched:**
- Inspector payload: add filter inventory (name, library, module) — Django is authoritative on what's registered
- `crates/djls-templates/src/parser.rs` — parse filters into structured representation (name, arg, span)
- `crates/djls-semantic/` — filter scoping plumbed through load resolution

**Note on filter inventory vs filter signatures:** The inspector reports the authoritative runtime inventory of filters (per library load-name + builtins), as Django has registered them. Rust then optionally enriches those items by statically extracting signatures/arity from source via Ruff AST (in M5); when enrichment is ambiguous, djls stays conservative (skip arg-validation rather than guess).

**Core breakpoint:** Parser/nodelist filter representation change touches many layers. Plan as explicit breakpoint with compatibility strategy (update all callers in one PR, or add adapter layer).

**How we verify:**
- Unit test: Filter parsing produces structured representation with spans
- Completion test: `{{ x|` context detection works
- Scoping test: Filter from unloaded library → diagnostic
- Integration test: Inspector returns filter inventory per library

**Why this comes after M2/M3:** Parser churn amplifies caching bugs; invalidation should be solid first. Filters reuse load scoping infrastructure from M3.

---

### M5: Extraction Engine (Rules/Validation Enrichment)

**User-visible outcome:**
- Tag validation rules populated from Django source, not `builtins.rs`
- Third-party library tags validated with extracted rules
- Filter arity/signatures extracted as enrichment

**Main system boundary touched:**
- New crate: `djls-extraction` (depends on `ruff_python_parser`)
- Extraction outputs: `TagValidation`, `ParseBitsSpec`, `BlockTagSpec`, filter signatures
- Enriches inspector inventory with validation semantics
- Replaces hardcoded `builtins.rs` validation logic

**Note:** This is about **rules/validation**, not inventory. The inspector (M1/M4) provides the authoritative inventory of what tags/filters exist. This milestone extracts *how to validate them* by mining TemplateSyntaxError paths, parse_bits signatures, block structure, etc.

**How we verify:**
- Golden test: Extract Django 5.2 tag rules → compare to baseline JSON
- Parity test: Extracted rules match template_linter's extraction
- Unit tests: Specific patterns (registration styles, block specs, opaque blocks)
- Integration: Rules applied to tags from inspector inventory

---

### M6: Rule Evaluation + Expression Validation

**User-visible outcome:**
- Rich argument validation: "Missing required argument `as`", "Unknown option `foo`"
- Block structure validation: "Unclosed `{% if %}`", "Unexpected `{% else %}` outside block"
- Filter argument validation: "Filter `date` expects 0-1 arguments, got 2"
- Expression validation: `{% if and x %}` → "Expected operand, found operator 'and'"

**Main system boundary touched:**
- `crates/djls-semantic/src/arguments.rs` — handle extracted `ContextualRule` preconditions
- `crates/djls-semantic/src/blocks/builder.rs` — use extracted `BlockTagSpec`
- Expression parsing for `{% if %}`/`{% elif %}` (folds into rule evaluation)

**How we verify:**
- Snapshot tests: Various error cases produce expected diagnostics
- Corpus test: Run against Django admin templates with strict unknowns
- Regression: No new false positives vs template_linter on same corpus
- Expression tests: Each operator, valid/invalid expressions

---

## 4. Decision Inventory

These decisions must be made before or during implementation. Each has trade-offs.

### D1: Extraction Data Format

| Option | Pros | Cons |
|--------|------|------|
| **Runtime JSON** (load at startup) | Hot-reload rules without rebuild; third-party rules as data files | Startup cost; schema versioning |
| **Code generation** (build-time) | Zero runtime cost; type-safe; IDE support | Rebuild for new rules; harder third-party story |
| **Hybrid** (codegen builtins, JSON third-party) | Fast for common case; flexible for extensions | Two code paths |

**Recommendation:** Start with **Runtime JSON**. Enables rapid iteration during port, supports third-party libraries naturally, and can be optimized later if startup cost matters.

### D2: Rust Python Parser

| Option | Pros | Cons |
|--------|------|------|
| **ruff_python_parser** | Actively maintained; fast; Python 3.13+; used by Ruff/ty | Git dependency; no stability guarantees |
| **rustpython-parser** | On crates.io; stable API | Stale (18mo); may lack Python 3.12+ syntax |
| **tree-sitter-python** | Incremental; error-tolerant | CST not AST; wrong abstraction |

**Recommendation:** Use **ruff_python_parser** via git with SHA pinning. It's the only actively maintained Python AST parser in Rust and powers all Astral tooling. Pin to a known-good SHA and update deliberately.

### D3: Salsa Invalidation Model

| Option | Pros | Cons |
|--------|------|------|
| **TagSpecs as Salsa input** | Precise invalidation; correct | Adds parameter to many functions; complexity |
| **Bump file revisions on config change** | Simple; minimal code changes | Over-invalidates (all files, not just affected) |
| **Hash specs into file identity** | Automatic invalidation | Hacky; conflates file identity with config |

**Recommendation:** Start with **bump file revisions** for simplicity. If profiling shows it's too expensive, migrate to explicit Salsa input.

### D4: Filter Scoping Semantics

| Option | Pros | Cons |
|--------|------|------|
| **"Final filter set"** (template_linter behavior) | Simple; matches current prototype | Position before `{% load %}` incorrectly allows filter |
| **Position-aware** (correct semantics) | Accurate load scoping | Requires filter tokens to carry positions |

**Recommendation:** Implement **position-aware** from the start. The Rust tokenizer already has spans; don't regress from correct semantics.

### D5: Collision Resolution Without Registry

| Option | Pros | Cons |
|--------|------|------|
| **Conservative: omit ambiguous** | No false positives | May miss real errors |
| **Best-effort: pick first** | More coverage | May produce wrong diagnostics |
| **Warn about ambiguity** | User learns about issue | Noisy if many collisions |

**Recommendation:** **Conservative (omit ambiguous)** for validation; **warn about ambiguity** for completions. User can provide runtime registry for accuracy.

### D6: Hardcodes/Overrides Location

| Option | Pros | Cons |
|--------|------|------|
| **Single Rust module** (`overrides.rs`) | Centralized; auditable | Rust rebuild for changes |
| **Config file** (TOML/JSON) | User-editable; hot-reload | Schema complexity |
| **Both** (Rust defaults + config overrides) | Flexibility | Two sources of truth |

**Recommendation:** **Single Rust module** initially, with config override capability as future enhancement.

---

## 5. Architecture Options

**Decided direction:** 

| Concern | Owner | Why |
|---------|-------|-----|
| **Inventory** (what exists) | Python inspector | Django is authoritative on what tags/filters are registered, which libraries exist, app ordering effects, dynamic registration |
| **Rules/validation** (how to validate) | Rust + `ruff_python_parser` | Fast, safe, testable; mines TemplateSyntaxError paths, block structure, expression grammar |

The Python inspector provides:
- `engine.libraries` mapping (load-name → module path) with keys preserved
- Ordered builtin module paths
- Tag/filter inventory per library (names, which library they belong to)

Rust extraction enriches the inventory with validation semantics by parsing source files.

The remaining question is **where extraction lives within the Rust codebase**.

### Option A: Expand `djls-semantic` (No New Crates)

Add extraction and new validation logic to existing crate.

```
djls-semantic/
├── src/
│   ├── extraction/        # NEW: Python AST → rules
│   │   ├── mod.rs
│   │   ├── tags.rs
│   │   ├── filters.rs
│   │   ├── structural.rs
│   │   └── opaque.rs
│   ├── load_resolution.rs # NEW: {% load %} scoping
│   ├── filters.rs         # NEW: filter validation
│   ├── if_expression.rs   # NEW: expression validation
│   ├── arguments.rs       # EXTEND: use extracted rules
│   └── blocks/            # EXTEND: use extracted BlockTagSpec
```

**Pros:**
- No new crate boundaries to manage
- Shared types don't need pub export juggling
- Simpler dependency graph

**Cons:**
- `djls-semantic` grows large
- Extraction depends on `ruff_python_parser`; semantic analysis doesn't
- Harder to test extraction in isolation

### Option B: New `djls-extraction` Crate

Separate crate for Python AST parsing and rule extraction.

```
djls-extraction/           # NEW CRATE
├── Cargo.toml             # Depends on ruff_python_parser
└── src/
    ├── lib.rs
    ├── registry.rs        # Registration discovery
    ├── rules.rs           # TemplateSyntaxError → rules
    ├── structural.rs      # Block specs
    ├── filters.rs         # Filter specs
    └── types.rs           # TagValidation, BlockTagSpec, etc.

djls-semantic/
├── src/
│   ├── load_resolution.rs # Uses extraction output
│   ├── filters.rs
│   └── ...                # Uses extraction output
```

**Pros:**
- Clean separation: extraction (Python AST) vs validation (templates)
- Extraction crate testable in isolation
- `ruff_python_parser` dependency confined to one crate

**Cons:**
- Adds crate boundary
- Types must be pub-exported across crates
- Slightly more complex build graph

### Option C: New `djls-rules` Crate (Types Only)

Separate crate for rule types; extraction stays ad-hoc.

```
djls-rules/                # NEW CRATE: types only
├── src/
│   └── lib.rs             # TagValidation, BlockTagSpec, FilterSpec, etc.

djls-semantic/
├── src/
│   ├── extraction/        # Uses djls-rules types
│   └── ...                # Uses djls-rules types
```

**Pros:**
- Rule types shareable without extraction logic
- Minimal new crate

**Cons:**
- Doesn't solve the extraction isolation problem
- `ruff_python_parser` still in semantic

### Comparison Matrix

| Criterion | A (semantic) | B (crate) | C (types) |
|-----------|--------------|-----------|-----------|
| Dependency isolation | ❌ | ✅ | ❌ |
| Test in isolation | ❌ | ✅ | ❌ |
| Conceptual clarity | ⚠️ | ✅ | ⚠️ |
| Build complexity | ✅ | ⚠️ | ✅ |
| Future flexibility | ⚠️ | ✅ | ⚠️ |

### Recommendation: Option B (`djls-extraction` Crate)

**Rationale:**
1. **Dependency isolation**: `ruff_python_parser` (git dep) confined to one crate
2. **Testability**: Extraction testable against golden fixtures without template parsing
3. **Conceptual clarity**: "Python source analysis" vs "template semantic analysis" are distinct concerns
4. **Safety**: Changes to extraction don't risk breaking template validation

The crate boundary cost is low; the isolation benefit is high given the git-dep nature of `ruff_python_parser`.

---

## 6. Risk Register

### R1: Tokenization Drift

| Aspect | Detail |
|--------|--------|
| **Risk** | Rust template tokenizer produces different token boundaries than Django's lexer |
| **Impact** | Validation misses errors or produces false positives |
| **Likelihood** | Medium (djls already has a tokenizer; may have subtle differences) |
| **Mitigation** | Tokenization parity fixtures from template_linter; regression tests on Django admin templates |
| **Monitoring** | Corpus test: any new diagnostics vs baseline = investigation |

### R2: AST Pattern Ambiguity

| Aspect | Detail |
|--------|--------|
| **Risk** | Python AST patterns for extraction are more varied than template_linter handles |
| **Impact** | Missing rules for some tags; over-extraction (false rules) |
| **Likelihood** | Medium (template_linter corpus proves patterns, but Rust port may interpret differently) |
| **Mitigation** | Golden tests: extracted rules match template_linter output; conservative extraction (skip ambiguous) |
| **Monitoring** | Per-library inventory test: tag/filter count matches expected |

### R3: ruff_python_parser Instability

| Aspect | Detail |
|--------|--------|
| **Risk** | Breaking changes in ruff_python_parser AST types |
| **Impact** | Extraction code breaks; requires port updates |
| **Likelihood** | Low-Medium (internal crate, but Astral is careful) |
| **Mitigation** | Pin to SHA; don't update without testing; wrapper types if needed; monitor Ruff releases for breaking changes |
| **Monitoring** | CI build failure on parser update = review changes; watch Ruff changelog |

### R4: Load Scoping Edge Cases

| Aspect | Detail |
|--------|--------|
| **Risk** | Complex `{% load %}` scenarios (extends/include, conditional loads) break scoping |
| **Impact** | Incorrect diagnostics; user confusion |
| **Likelihood** | Medium (extends/include are common) |
| **Mitigation** | Document limitations; conservative default (assume unknown tags valid in complex cases) |
| **Monitoring** | Real-world project testing; user feedback |

### R5: Performance Regression

| Aspect | Detail |
|--------|--------|
| **Risk** | Extracted rules are larger/slower than handcoded specs |
| **Impact** | Slow LSP response; poor UX |
| **Likelihood** | Low (Salsa caching; rules loaded once) |
| **Mitigation** | Profile before/after; optimize rule representation if needed |
| **Monitoring** | Benchmark: diagnostics latency on large template |

### R6: Collision Without Registry

| Aspect | Detail |
|--------|--------|
| **Risk** | Projects with multiple templatetags dirs have collisions; static analysis can't resolve |
| **Impact** | Missing validation for colliding tags; user must provide registry |
| **Likelihood** | High (common in Django projects) |
| **Mitigation** | Clear diagnostic: "Cannot resolve `foo`: multiple libraries define it"; document registry setup |
| **Monitoring** | Track user reports of collision issues |

### R7: Filter Argument Parsing Complexity

| Aspect | Detail |
|--------|--------|
| **Risk** | Django's filter argument syntax (`|filter:"arg"`) has quote handling edge cases |
| **Impact** | Filter validation false positives/negatives |
| **Likelihood** | Medium (template_linter has `_parse_filter_chain`; port may diverge) |
| **Mitigation** | Port filter parsing logic carefully; dedicated test suite for quoting |
| **Monitoring** | Filter-specific corpus tests |

### R8: Parser Filter Representation Change (M4 Breakpoint)

| Aspect | Detail |
|--------|--------|
| **Risk** | Changing `filters: Vec<String>` → structured representation touches many layers |
| **Impact** | Wide-ranging PR; potential for subtle regressions; merge conflicts |
| **Likelihood** | High (this is a known "touch many layers" change) |
| **Mitigation** | Plan as explicit breakpoint: update all callers in one PR, or add adapter layer; do after invalidation (M2) is solid to avoid amplifying caching bugs |
| **Monitoring** | Comprehensive snapshot tests before/after; no intermediate broken states |

---

## 7. Success Criteria

The port is "complete" when:

1. **Inventory parity**: Extracted tag/filter count for Django 5.2 matches template_linter
2. **Validation parity**: Running against Django admin templates produces equivalent diagnostics (same error codes, same locations ±1 line)
3. **No regressions**: All existing djls tests pass
4. **Load scoping works**: Documented test cases for `{% load %}` behavior all pass
5. **Filter validation exists**: `{{ x|unknown }}` produces diagnostic
6. **Salsa invalidation works**: Config change triggers revalidation (tested)
7. **Corpus clean**: Django admin templates validate with zero false positives under strict mode

---

## 8. Open Questions (Captured for Resolution)

| # | Question | Proposed Owner | Depends On |
|---|----------|----------------|------------|
| Q1 | What SHA of ruff_python_parser to pin initially? | Port lead | M5 kickoff |
| Q2 | How to handle `{% extends %}`/`{% include %}` for load scoping? | Port lead | M3 design |
| Q3 | What's the performance budget for rule loading at startup? | Project maintainer | Profiling |
| Q4 | Should third-party library rules be bundled or user-provided? | Project maintainer | M5 design |
| Q5 | What's the compatibility strategy for parser filter representation change (M4)? | Port lead | M4 planning |

---

## Appendix A: Key File References

### template_linter (Python Prototype)

| Module | Purpose |
|--------|---------|
| `extraction/api.py` | Entry points for extraction |
| `extraction/rules.py` | AST → validation rules |
| `extraction/structural.py` | Block spec extraction |
| `extraction/filters.py` | Filter spec extraction |
| `resolution/load.py` | `{% load %}` resolution |
| `resolution/runtime_registry.py` | Django Engine state |
| `validation/tags.py` | Tag rule validation |
| `validation/filters.py` | Filter validation |
| `validation/if_expression.py` | Expression parsing |
| `types.py` | `TagValidation`, `BlockTagSpec`, etc. |

### django-language-server (Rust)

| Location | Purpose |
|----------|---------|
| `crates/djls-semantic/src/templatetags/builtins.rs` | Handcoded TagSpecs (to replace) |
| `crates/djls-semantic/src/templatetags/specs.rs` | `TagSpec`, `TagSpecs` types |
| `crates/djls-semantic/src/blocks/` | Block tree building |
| `crates/djls-semantic/src/arguments.rs` | Argument validation |
| `crates/djls-project/inspector/queries.py` | Python introspection |
| `crates/djls-project/src/django.rs` | `TemplateTag` types |
| `crates/djls-ide/src/completions.rs` | LSP completions |
| `crates/djls-server/src/db.rs` | Salsa database, settings |

---

## Appendix B: Mapping template_linter Types → djls

| template_linter | djls Current | djls Target |
|-----------------|--------------|-------------|
| `TagValidation` | `TagSpec` (partial) | `TagSpec` + validation rules |
| `ExtractedRule` | N/A | Validation logic in `arguments.rs` |
| `ContextualRule` | N/A | Preconditioned validation (new) |
| `ParseBitsSpec` | `TagArg` | `TagArg` (extended) |
| `BlockTagSpec` | `TagSpec.end_tag` + `intermediate_tags` | Same (populated from extraction) |
| `OpaqueBlockSpec` | N/A | New type for verbatim/comment |
| `FilterSpec` | N/A | New type in `djls-semantic` |
| `LibraryIndex` | N/A | New in `load_resolution.rs` |
| `RuntimeRegistry` | N/A | New in `djls-project` |
