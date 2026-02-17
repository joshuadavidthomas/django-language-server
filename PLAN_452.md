# Plan: Reduce Allocations in Semantic Validation Hot Path (#452)

## Investigation Summary

Callgrind profiling of `just dev profile semantic validate_all_templates` revealed
several allocation-heavy patterns accounting for ~15-20% of total validation
instruction count:

- `String::clone` — 661K Ir (2.98%), 4,500+ string clones per validation batch
- `hashbrown::RawTable::reserve_rehash` — 631K Ir (2.84%)
- `core::hash::BuildHasher::hash_one` — 613K Ir (2.76%)
- `hashbrown::HashMap::insert` — 429K Ir (1.93%)
- `drop_in_place<TemplateLibraries>` — 492K Ir (2.22%)

## Root Cause Analysis

### The Core Problem: Cloning at the Trait Boundary

The `SemanticDb` trait returns owned values for large data structures:

```rust
// Current: returns owned values — every call clones
fn template_libraries(&self) -> TemplateLibraries;
fn tag_specs(&self) -> TagSpecs;
fn filter_arity_specs(&self) -> FilterAritySpecs;
```

This creates a cascade of unnecessary allocations:

1. **`TemplateValidator::new`** calls `db.template_libraries()`, `db.tag_specs()`,
   and `db.filter_arity_specs()` — three clones of large data structures.

2. **`check_tag_scoping_rule`** and **`check_filter_scoping_rule`** each call
   `db.template_libraries()` *again* — cloning the entire `TemplateLibraries` on
   **every tag and every filter** just to check `inspector_knowledge` (a boolean
   guard). For a template with N tags and M filter uses, that's N+M additional
   full clones.

3. **`AvailableSymbols::at_position`** is called per-node. Each call:
   - Runs `LoadedLibraries::available_at(position)` which clones strings from
     `LoadStatement` into owned `HashSet<String>` / `HashMap<String, HashSet<String>>`
   - Runs `AvailableSymbols::from_load_state` which builds 4 hash collections
     from scratch with zero initial capacity

4. **`LoadState`** is cloned into `AvailableSymbols` for `is_library_loaded` /
   `is_symbol_imported` queries used by completions — not needed during validation.

### Full Call Stack (Annotated with Allocation Sites)

```
validate_nodelist(db, nodelist)                    [#[salsa::tracked]]
 └─ TemplateValidator::new(db, nodelist, opaque_regions)
      ├─ db.template_libraries()                   ← CLONE: entire TemplateLibraries
      ├─ db.tag_specs()                            ← CLONE: entire FxHashMap<String, TagSpec>
      ├─ db.filter_arity_specs()                   ← CLONE: entire FilterAritySpecs
      ├─ compute_loaded_libraries(db, nodelist)    [#[salsa::tracked], cached]
      ├─ discovered_symbol_candidates_by_name(Tag)
      └─ discovered_symbol_candidates_by_name(Filter)

 └─ validator.validate(nodes)
      └─ walk_nodelist(&mut validator, nodes)
           ├─ visit_tag(name, bits, span)          [per tag node]
           │    ├─ AvailableSymbols::at_position(...)
           │    │    ├─ available_at(position)      ← CLONE: String per load stmt
           │    │    └─ from_load_state(...)        ← ALLOC: 4 hash collections from zero
           │    │         └─ load_state.clone()     ← CLONE: HashSet + HashMap of Strings
           │    ├─ check_tag_scoping_rule(db, ...)
           │    │    └─ db.template_libraries()     ← CLONE: entire TemplateLibraries (AGAIN!)
           │    └─ ...
           │
           └─ visit_variable(var, filters, span)   [per variable node]
                ├─ AvailableSymbols::at_position(...)  [same allocation path]
                └─ per filter:
                     └─ check_filter_scoping_rule(db, ...)
                          └─ db.template_libraries()   ← CLONE: entire TemplateLibraries (AGAIN!)
```

## Reference Architecture: How ruff/ty Solves This

Analysis of the ruff/ty codebase (cloned locally in `./reference/ruff/`) reveals a
clean pattern for avoiding clones at Salsa DB trait boundaries.

### DB Trait Methods Return References

Every large type crosses the trait boundary as `&T`:

```rust
// ruff_db::Db
fn vendored(&self) -> &VendoredFileSystem;
fn system(&self) -> &dyn System;
fn files(&self) -> &Files;
fn python_version(&self) -> PythonVersion;  // only Copy types by value

// ty_module_resolver::Db
fn search_paths(&self) -> &SearchPaths;

// ty_python_semantic::Db
fn rule_selection(&self, file: File) -> &RuleSelection;
fn lint_registry(&self) -> &LintRegistry;
fn analysis_settings(&self, file: File) -> &AnalysisSettings;
```

### Three Backing Storage Patterns

Each `&T` return is backed by one of three storage locations:

| Storage | Example | Mechanism |
|---------|---------|-----------|
| DB struct field | `&self.files` | Plain field borrow |
| Salsa input field | `Program::get(self).search_paths(self)` | `#[returns(ref)]` on input field |
| Tracked function memo | `file_settings(self, file).rules(self)` | `#[salsa::tracked(returns(ref))]` |

### Config Singleton Pattern

`Program` is a `#[salsa::input(singleton)]` with `#[returns(ref)]` on all fields
and `Durability::HIGH`. DB trait methods delegate directly:

```rust
fn search_paths(&self) -> &SearchPaths {
    Program::get(self).search_paths(self)  // &T from Salsa storage, zero clones
}
```

### No Wrappers, No Name Interning

- `ModuleName` and `Name` are plain `CompactString` newtypes — NOT Salsa interned
- No tracked struct wrappers around config data
- Interning is reserved for type representations (`Type<'db>`, unions, etc.)
  where deduplication and Copy semantics matter for millions of comparisons
- `'db` lifetime stays in the type system layer, not in config/project data

### Key Takeaway

The ruff/ty approach avoids clones through **references at trait boundaries**,
not through Salsa wrappers or interning. The data stays where it naturally lives
(Salsa inputs, struct fields) and is borrowed through the entire call stack.

## Approaches Considered and Rejected

### Interning `LibraryName` / `TemplateSymbolName` in Salsa

- Would make names Copy with O(1) equality
- `'db` would infect `TemplateLibraries`, `LoadState`, `AvailableSymbols`, and
  everything downstream
- Can't store `'db` types on `Project` (Salsa input)
- Breaks `Serialize`/`Deserialize` on `TemplateLibraries`
- Massive refactor touching 16+ files across every crate
- **Rejected**: ruff/ty doesn't intern names either. The overhead is in cloning
  containers, not in string comparisons.

### Tracked Struct Wrapper for `TemplateLibraries`

- Wrap `TemplateLibraries` in a `#[salsa::tracked]` struct (like `TagIndex`)
- Returns Copy ID, access via `#[returns(ref)]`
- **Rejected**: Unnecessary complexity. ruff/ty achieves the same result by
  returning `&T` from trait methods. No wrapper needed.

### Interning Names at the Semantic Boundary Only

- Define `InternedName<'db>` in `djls-semantic`
- Use interned IDs in `LoadState` and `AvailableSymbols`
- `'db` contained to 4-5 files in djls-semantic
- **Deferred**: Could be a future enhancement if profiling shows string hashing
  is still hot after the reference-based approach. For now, follow ruff/ty's lead.

## Implementation Plan

### Phase 1: Return References from `SemanticDb` Trait

**Goal**: Eliminate all cloning at the trait boundary. This is the highest-impact
change and addresses the root cause.

#### Step 1.1: Change `SemanticDb` trait signatures

```rust
// crates/djls-semantic/src/db.rs
#[salsa::db]
pub trait Db: TemplateDb {
    fn tag_specs(&self) -> &TagSpecs;
    fn tag_index(&self) -> TagIndex<'_>;        // already returns Salsa type
    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>>;  // small, keep owned
    fn diagnostics_config(&self) -> DiagnosticsConfig;    // small, keep owned
    fn template_libraries(&self) -> &TemplateLibraries;
    fn filter_arity_specs(&self) -> &FilterAritySpecs;
    fn model_graph(&self) -> &ModelGraph;
}
```

Files: `crates/djls-semantic/src/db.rs`

#### Step 1.2: Update production DB (`DjangoDatabase`)

For `template_libraries`: `project.template_libraries(self)` already returns
`&TemplateLibraries` from the Salsa input's `#[returns(ref)]` field. Just pass
it through. Use a static default for the no-project fallback.

For `tag_specs` and `filter_arity_specs`: Change the tracked functions to
`#[salsa::tracked(returns(ref))]` so they store the result in Salsa's memo and
return `&T`. Use static defaults for fallbacks.

For `model_graph`: Same pattern — `returns(ref)` on the tracked function.

```rust
// crates/djls-db/src/db.rs
use std::sync::LazyLock;

static DEFAULT_TEMPLATE_LIBRARIES: LazyLock<TemplateLibraries> =
    LazyLock::new(TemplateLibraries::default);
static DEFAULT_TAG_SPECS: LazyLock<TagSpecs> =
    LazyLock::new(djls_semantic::builtin_tag_specs);
static DEFAULT_FILTER_ARITY_SPECS: LazyLock<FilterAritySpecs> =
    LazyLock::new(FilterAritySpecs::new);
static DEFAULT_MODEL_GRAPH: LazyLock<ModelGraph> =
    LazyLock::new(ModelGraph::new);

impl SemanticDb for DjangoDatabase {
    fn template_libraries(&self) -> &TemplateLibraries {
        self.project()
            .map(|project| project.template_libraries(self))
            .unwrap_or(&DEFAULT_TEMPLATE_LIBRARIES)
    }

    fn tag_specs(&self) -> &TagSpecs {
        if let Some(project) = self.project() {
            compute_tag_specs(self, project)  // now returns &TagSpecs
        } else {
            &DEFAULT_TAG_SPECS
        }
    }

    fn filter_arity_specs(&self) -> &FilterAritySpecs {
        if let Some(project) = self.project() {
            compute_filter_arity_specs(self, project)  // now returns &FilterAritySpecs
        } else {
            &DEFAULT_FILTER_ARITY_SPECS
        }
    }

    fn model_graph(&self) -> &ModelGraph {
        if let Some(project) = self.project() {
            compute_model_graph(self, project)  // now returns &ModelGraph
        } else {
            &DEFAULT_MODEL_GRAPH
        }
    }
}
```

```rust
// crates/djls-db/src/queries.rs — add returns(ref)
#[salsa::tracked(returns(ref))]
fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs { ... }

#[salsa::tracked(returns(ref))]
fn compute_filter_arity_specs(db: &dyn SemanticDb, project: Project) -> FilterAritySpecs { ... }

#[salsa::tracked(returns(ref))]
fn compute_model_graph(db: &dyn SemanticDb, project: Project) -> ModelGraph { ... }
```

Files: `crates/djls-db/src/db.rs`, `crates/djls-db/src/queries.rs`

#### Step 1.3: Update bench DB

```rust
// crates/djls-bench/src/db.rs
impl SemanticDb for Db {
    fn tag_specs(&self) -> &TagSpecs {
        &self.tag_specs  // Arc<TagSpecs> auto-derefs
    }
    fn template_libraries(&self) -> &TemplateLibraries {
        &self.template_libraries  // Arc<TemplateLibraries> auto-derefs
    }
    fn filter_arity_specs(&self) -> &FilterAritySpecs {
        &self.filter_arity_specs  // Arc<FilterAritySpecs> auto-derefs
    }
    fn model_graph(&self) -> &ModelGraph {
        static DEFAULT: LazyLock<ModelGraph> = LazyLock::new(ModelGraph::new);
        &DEFAULT
    }
}
```

Files: `crates/djls-bench/src/db.rs`

#### Step 1.4: Update test DB

```rust
// crates/djls-semantic/src/testing.rs
impl crate::Db for TestDatabase {
    fn tag_specs(&self) -> &TagSpecs { &self.tag_specs }
    fn template_libraries(&self) -> &TemplateLibraries { &self.template_libraries }
    fn filter_arity_specs(&self) -> &FilterAritySpecs { &self.filter_arity_specs }
    fn model_graph(&self) -> &ModelGraph {
        static DEFAULT: LazyLock<ModelGraph> = LazyLock::new(ModelGraph::new);
        &DEFAULT
    }
}
```

Files: `crates/djls-semantic/src/testing.rs`

#### Step 1.5: Update all callers

- `TemplateValidator::new` — store `&'a TemplateLibraries` (borrow) instead of
  owned `TemplateLibraries`. The `'a` lifetime is already on the struct.
- `check_tag_scoping_rule` / `check_filter_scoping_rule` — access
  `db.template_libraries()` which now returns `&TemplateLibraries` (zero-cost).
  Or better: pass `&TemplateLibraries` from the validator to avoid even the
  trait method call.
- `AvailableSymbols::at_position` — already takes `&'a TemplateLibraries`.
  No change needed in the signature; the `'a` now borrows from Salsa storage
  instead of from a clone.
- `validation/filters.rs` — update to borrow `&FilterAritySpecs`.
- `djls-ide/src/completions.rs` — update to work with `&TemplateLibraries`.

Files: `crates/djls-semantic/src/validation.rs`, `crates/djls-semantic/src/validation/scoping.rs`,
`crates/djls-semantic/src/validation/filters.rs`, `crates/djls-ide/src/completions.rs`

### Phase 2: Fix Redundant `db.template_libraries()` Calls in Scoping

Even with Phase 1 (no clones), `check_tag_scoping_rule` and
`check_filter_scoping_rule` call `db.template_libraries()` just to read
`inspector_knowledge`. Pass the knowledge status from the validator instead.

```rust
// Before:
pub(crate) fn check_tag_scoping_rule(db: &dyn Db, name: &str, ...) {
    let template_libraries = db.template_libraries();  // redundant call
    if template_libraries.inspector_knowledge != Knowledge::Known { return; }
    ...
}

// After:
pub(crate) fn check_tag_scoping_rule(
    db: &dyn Db,
    name: &str,
    span: Span,
    symbols: &AvailableSymbols,
    env_tags: Option<&HashMap<...>>,
    inspector_knowledge: Knowledge,
) {
    if inspector_knowledge != Knowledge::Known { return; }
    ...
}
```

Files: `crates/djls-semantic/src/validation/scoping.rs`, `crates/djls-semantic/src/validation.rs`

### Phase 3: Cache `AvailableSymbols` per `LoadState` in Validator

(Original subtask `1ulb1f53`)

Between two `{% load %}` tags, every node gets the identical `AvailableSymbols`.
Cache by `LoadState` in the `TemplateValidator`:

```rust
pub struct TemplateValidator<'a> {
    // ...existing fields...
    cached_symbols: Option<(LoadState, AvailableSymbols<'a>)>,
}
```

In `visit_tag` / `visit_variable`:

```rust
let load_state = self.loaded_libraries.available_at(position);
let symbols = match &self.cached_symbols {
    Some((cached_state, cached)) if *cached_state == load_state => cached,
    _ => {
        let symbols = AvailableSymbols::from_load_state(
            &load_state, self.template_libraries
        );
        self.cached_symbols = Some((load_state, symbols));
        &self.cached_symbols.as_ref().unwrap().1
    }
};
```

Reduces `from_load_state` calls from O(nodes) to O(load_statements + 1).

Files: `crates/djls-semantic/src/validation.rs`

### Phase 4: Capacity Hints in `from_load_state`

(Original subtask `klfzzq1x`)

Pre-compute sizes and use `with_capacity_and_hasher`:

```rust
let builtin_count = template_libraries.builtin_libraries()
    .flat_map(|lib| &lib.symbols).count();
let loadable_count = template_libraries.enabled_loadable_libraries()
    .flat_map(|(_, lib)| &lib.symbols).count();

let mut available = FxHashSet::with_capacity_and_hasher(
    builtin_count, Default::default()
);
// ... same for candidates, available_filters, filter_candidates
```

Also eliminate double-hashing where `.contains()` is followed by `.entry()`.

Files: `crates/djls-semantic/src/scoping/symbols.rs`

### Phase 5: Borrow in `LoadState` Instead of Cloning Strings

(Original subtask `nx66i1if`)

Change `LoadState` to borrow from `LoadedLibraries` statements:

```rust
pub struct LoadState<'a> {
    fully_loaded: HashSet<&'a str>,
    selective: HashMap<&'a str, HashSet<&'a str>>,
}
```

`available_at` borrows from `&self.statements` which are owned by
`LoadedLibraries` (returned from a Salsa tracked function, lives long enough).

Files: `crates/djls-semantic/src/scoping/loads.rs`,
`crates/djls-semantic/src/scoping/symbols.rs`

### Phase 6: Consolidate `TagIndex::classify` to Single Lookup

(Original subtask `nm1ofs61`)

Merge three `FxHashMap` fields into one with a `TagRole` enum:

```rust
enum TagRole {
    Opener(EndMeta),
    Closer { opener: String },
    Intermediate { possible_openers: Vec<String> },
}

#[salsa::tracked(debug)]
pub struct TagIndex<'db> {
    #[tracked]
    #[returns(ref)]
    roles: FxHashMap<String, TagRole>,
}
```

`classify` becomes a single hash lookup instead of up to 3.

Files: `crates/djls-semantic/src/structure/grammar.rs`

### Subtask `yt62q7jx` (Avoid deep-cloning Arc-wrapped specs in bench DB)

**Resolved by Phase 1.** Once trait methods return `&T`, the bench DB returns
`&*self.tag_specs` (Arc auto-deref). No deep clones.

## Expected Impact

| Phase | What it eliminates | Est. Ir reduction |
|-------|-------------------|-------------------|
| Phase 1 | All `TemplateLibraries`/`TagSpecs`/`FilterAritySpecs` cloning | ~500K (drop_in_place + clone) |
| Phase 2 | N+M redundant `template_libraries()` calls per template | ~200K (at scale) |
| Phase 3 | O(nodes) → O(load_stmts) for `from_load_state` | ~600K (hashbrown rehash/insert) |
| Phase 4 | Hash table rehashing in `from_load_state` | ~300K (reserve_rehash) |
| Phase 5 | String cloning in `available_at` | ~250K (String::clone) |
| Phase 6 | 2 extra hash lookups per `classify` call | ~100K |

Combined: roughly 15-20% of total validation instruction count, matching the
original profiling assessment.

## Ordering and Dependencies

```
Phase 1 (trait boundary refs)
  ├─ Phase 2 (scoping function cleanup) — depends on Phase 1
  ├─ Phase 5 (LoadState borrowing) — independent but easier after Phase 1
  └─ Phase 6 (TagIndex consolidation) — independent

Phase 3 (AvailableSymbols caching) — independent of Phase 1
Phase 4 (capacity hints) — independent, can be done with Phase 3
```

Phase 1 is the foundation — do it first. Phases 2-6 can be done in any order
after that, potentially as separate PRs.

## Verification

After each phase:
- `cargo test -q` — all tests pass
- `just clippy` — no warnings
- `just dev profile semantic validate_all_templates` — measure instruction count reduction
- Compare specific cost centers (`String::clone`, `reserve_rehash`, `drop_in_place`)
  against the baseline numbers above
