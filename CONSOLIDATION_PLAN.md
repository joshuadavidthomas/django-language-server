# Consolidation Plan: Intent-Opus as Base, Cherry-Pick from Detailed-Opus

## Context

This plan is executed inside the `intent-opus-4.6` worktree. It brings valuable infrastructure from a sibling worktree (`detailed-opus-4.6`) into this codebase. Both were produced by the same model (Opus 4.6) with different prompting strategies. Intent-opus has better architecture and more complete user-facing features; detailed-opus has better infrastructure (corpus testing, module resolution, workspace/external partitioning).

**Source worktree (read-only reference)**: `../detailed-opus-4.6/`
**Target worktree (where changes are made)**: `.` (current directory, `intent-opus-4.6`)

## Decision

Use **`intent-opus-4.6`** as the base and bring in the valuable pieces from **`detailed-opus-4.6`**. The reasoning:

- Intent-opus has more complete end-to-end functionality (argument completions, snippets, filter arity validation, if-expression validation, opaque regions — all wired up)
- Detailed-opus has better infrastructure pieces (corpus crate, module resolution, workspace/external partitioning) but stubbed out the user-facing features
- Pulling infrastructure *into* a working system is easier than replacing stubs *with* working features

## What Intent-Opus Has (Keep As-Is)

### Fully wired features
- **Argument completions** from extracted args — `generate_argument_completions()` in `crates/djls-ide/src/completions.rs` uses `ExtractedArg` to offer position-aware completions (literals, choices, variables)
- **Snippet generation** — `crates/djls-ide/src/snippets.rs` generates LSP snippets from extracted args (e.g., `{% for ${1:item} in ${2:items} %}`)
- **If-expression Pratt parser** — `crates/djls-semantic/src/if_expression.rs` (651 lines), validates `{% if %}`/`{% elif %}` expression syntax, emits S114
- **Filter arity validation** — `crates/djls-semantic/src/filter_validation.rs` (441 lines), validates filter arguments against extracted arity, emits S115/S116
- **Opaque regions** — `crates/djls-semantic/src/opaque.rs` (325 lines), computed once in `validate_nodelist` and threaded through all passes
- **Modular load resolution** — split across 4 files under `crates/djls-semantic/src/load_resolution/`
- **163 extraction tests** with 42 golden fixture snapshots in `crates/djls-extraction/`
- **All 14 error variants** (S101–S117) in `crates/djls-semantic/src/errors.rs`
- **589 total passing tests**

### Data model
- `ExtractionResult` uses `FxHashMap<SymbolKey, TagRule>` + `FxHashMap<SymbolKey, FilterArity>` + `FxHashMap<SymbolKey, BlockTagSpec>` — in `crates/djls-extraction/src/types.rs`
- `TagRule` has `arg_constraints: Vec<ArgumentCountConstraint>` + `required_keywords: Vec<RequiredKeyword>` + `known_options: Option<KnownOptions>` + `extracted_args: Vec<ExtractedArg>`
- `extract_rules(source: &str, module_path: &str) -> ExtractionResult` — takes module path upfront, returns `Default` on parse failure
- `ExtractionResult::merge(&mut self, other: Self)` exists for combining results
- `TagSpec.extracted_rules` is `Option<TagRule>` in `crates/djls-semantic/src/templatetags/specs.rs`

### Current `Project` salsa input (`crates/djls-project/src/project.rs`)
```rust
pub struct Project {
    pub root: Utf8PathBuf,
    pub interpreter: Interpreter,
    pub django_settings_module: Option<String>,
    pub pythonpath: Vec<String>,
    pub inspector_inventory: Option<TemplateTags>,
    pub extracted_external_rules: Option<ExtractionResult>,  // ← will change
    pub diagnostics: DiagnosticsConfig,
}
```

### Current extraction flow (`crates/djls-server/src/db.rs`)
- `refresh_inspector()` (line 271) queries Python inspector, then calls `extract_external_rules()` on ALL registration modules (no workspace/external distinction)
- `extract_external_rules()` (line 332) collects module paths from inventory, resolves via `resolve_module_to_file()` (inline, line 463) + `build_search_paths()` (inline, line 381), reads source, calls `extract_rules()`, merges all into one `ExtractionResult`
- `compute_tag_specs()` (line 41) merges `django_builtin_specs()` + `project.extracted_external_rules(db)`
- `compute_filter_arity_specs()` (line 70) reads from `project.extracted_external_rules(db)`
- `extract_module_rules()` (line 96) — `#[salsa::tracked]` per-file extraction query, exists but is **DEAD CODE** (only referenced in tests)

## Functional Differences Between Implementations

A thorough comparison revealed functional gaps that the bake-off review missed. These need to be addressed during consolidation.

### Closer Argument Validation — CRITICAL gap in intent-opus

**What**: detailed-opus validates that closer arguments match opener arguments. For example, `{% block content %}...{% endblock wrong_name %}` emits `UnmatchedBlockName`. intent-opus gutted `validate_close()` to always return `Valid` with a misleading comment ("extraction handles validation" — extraction can't handle template-instance-level matching).

**Where in detailed-opus**:
- `crates/djls-semantic/src/blocks/grammar.rs` — `MatchArgSpec` struct, `EndMeta.match_args` field, full `validate_close()` with 5-variant `CloseValidation` enum
- `crates/djls-semantic/src/blocks/builder.rs` — match arms for `ArgumentMismatch`, `MissingRequiredArg`, `UnexpectedArg`

**What to port**: `MatchArgSpec`, `EndMeta.match_args`, the `validate_close()` logic, `CloseValidation` variants, and the builder match arms. This is ~80 lines of focused code. The `_opener_bits` and `_closer_bits` parameters on intent's `validate_close` already accept the right data — the logic just needs to be restored.

**Why extraction can't replace this**: Extraction works on library definitions (compile functions). Closer argument matching is template-instance-level — comparing what the user wrote in `{% endblock X %}` against what they wrote in `{% block Y %}`. No amount of Python AST analysis of Django's source code can tell you what a specific template author typed.

### Rule Evaluation Model — architectural difference, both work

**intent-opus**: Structured constraints (`ArgumentCountConstraint::Min/Max/Exact/OneOf`, `RequiredKeyword`, `KnownOptions`). Evaluator generates error messages.

**detailed-opus**: Condition-based with negation semantics (`RuleCondition` enum with `negated: bool`). Has an `Opaque` variant for unrecognized conditions (silently skipped). Error messages extracted from Python source.

**Decision**: Keep intent's model — it's more structured and generates better messages. The `Opaque` variant is nice-to-have but not critical; unrecognized patterns simply don't produce rules.

### Extraction Data Model — architectural difference

**intent-opus**: `FxHashMap<SymbolKey, TagRule>` — keyed by module + name, scales for multi-module.

**detailed-opus**: `Vec<ExtractedTag>` — flat list, needs post-processing for module keying.

**Decision**: Keep intent's model — HashMap keying is better for the Salsa integration where we merge multiple extraction results.

### Snippet Generation — CRITICAL gap in detailed-opus

**intent-opus** has full LSP snippet support: argument placeholders (`${1:item}`), choice dropdowns (`${1|on,off|}`), block name mirroring, partial completion during typing.

**detailed-opus** has only basic block name mirroring — no argument-based snippets.

**Decision**: Keep intent's snippets as-is (already working).

### Opaque Region Computation — minor efficiency difference

**intent-opus**: Computed once in `validate_nodelist`, passed to all validators.

**detailed-opus**: Recomputed inside each validator.

**Decision**: Keep intent's approach (more efficient, clearer data flow).

---

## What to Bring from Detailed-Opus

### 0. Closer Argument Validation — RESTORE from detailed-opus

**Source**: `../detailed-opus-4.6/crates/djls-semantic/src/blocks/grammar.rs` (lines 13-102)

**What to port**:
- `MatchArgSpec` struct (name, required, position)
- `EndMeta.match_args: Vec<MatchArgSpec>` field
- Full `validate_close()` logic comparing opener/closer argument values
- `CloseValidation` variants: `ArgumentMismatch`, `MissingRequiredArg`, `UnexpectedArg`
- `extract_arg_value()` helper
- Builder match arms in `crates/djls-semantic/src/blocks/builder.rs` for the new variants

**Action**: Port the validation logic, restore the `CloseValidation` variants (which were removed during cleanup), add the builder match arms back. Remove the unused `_opener_bits`/`_closer_bits` parameter prefixes. Wire `MatchArgSpec` population into `TagIndex::from_tag_specs` — detailed-opus derives match args from `TagSpec.args` which no longer exists in intent-opus, so this needs adaptation to work with extracted args or block spec data.

### 1. Module Resolution — NEW FILE: `crates/djls-project/src/resolve.rs`

**Source**: `../detailed-opus-4.6/crates/djls-project/src/resolve.rs` (236 lines)

**What it provides**:
```rust
pub enum ModuleLocation { Workspace, External }

pub struct ResolvedModule {
    pub module_path: String,
    pub file_path: Utf8PathBuf,
    pub location: ModuleLocation,
}

pub fn resolve_module(module_path: &str, search_paths: &[Utf8PathBuf], project_root: &Utf8Path) -> Option<ResolvedModule>
pub fn resolve_modules(module_paths: impl IntoIterator<Item = &str>, search_paths: &[Utf8PathBuf], project_root: &Utf8Path) -> (Vec<ResolvedModule>, Vec<ResolvedModule>)
pub fn build_search_paths(interpreter: &Interpreter, root: &Utf8Path, pythonpath: &[String]) -> Vec<Utf8PathBuf>  // moved from db.rs
```

The `build_search_paths` function (currently inline in `db.rs` at line 381) derives search paths from existing `Project` fields — no need to store `sys_path` as a Salsa input. It builds: project root + PYTHONPATH entries + site-packages from venv. Moving it to `resolve.rs` keeps all path resolution logic together.

Classifies resolved modules as Workspace (under `project_root`) or External (site-packages, stdlib). Uses `tempfile` for tests (already a workspace dep).

**Action**: Copy the file, add `pub mod resolve;` to `crates/djls-project/src/lib.rs`, export the public types. The existing inline `build_search_paths()` function in `db.rs` should be moved into `resolve.rs` as well (or called from there) since it derives search paths from `Interpreter` + `root` + `pythonpath`. The inline `resolve_module_to_file()` is replaced by `resolve_module()`.

### 2. Workspace/External Extraction Partitioning

**Why this matters**: Currently editing `myapp/templatetags/custom.py` does NOT trigger re-extraction — it only updates on `refresh_inspector()`. With workspace partitioning, workspace Python files get per-file Salsa tracking via `extract_module_rules`, so edits auto-invalidate.

**Changes needed in `crates/djls-project/src/project.rs`**:
- Change field: `pub extracted_external_rules: Option<ExtractionResult>` → `pub extracted_external_rules: FxHashMap<String, ExtractionResult>` (per-module keying for external only)
- Do NOT add a `sys_path` field — search paths are derived at call sites from existing `Project` fields (`interpreter`, `root`, `pythonpath`) via `build_search_paths()`. This avoids an unnecessary Salsa input and the question of how/when to populate it. Salsa already tracks the inputs it depends on.

**Changes needed in `crates/djls-server/src/db.rs`**:
- Move `build_search_paths()` and `find_site_packages()` to `crates/djls-project/src/resolve.rs` (or keep them in `db.rs` and call from there — either way, they stay as pure functions deriving paths from `Interpreter` + root + pythonpath)
- Remove inline `resolve_module_to_file()` — replaced by `resolve_module()` from `resolve.rs`
- Add `collect_workspace_extraction_results` tracked function (reference: `../detailed-opus-4.6/crates/djls-server/src/db.rs` line ~72):
  ```rust
  #[salsa::tracked]
  fn collect_workspace_extraction_results(db: &dyn SemanticDb, project: Project) -> Vec<(String, ExtractionResult)> {
      // 1. Get module paths from inspector_inventory
      // 2. Build search paths from project.interpreter(db), project.root(db), project.pythonpath(db)
      // 3. Resolve via resolve_modules() → partition workspace/external
      // 4. For each workspace module: db.get_or_create_file() then extract_module_rules(db, file)
      // 5. Return vec of (module_path, result) for non-empty results
  }
  ```
  Note: This reads `interpreter`, `root`, and `pythonpath` from `Project` via `db`, which establishes Salsa dependencies on those inputs. If any of them change, this query auto-invalidates. No separate `sys_path` input needed.
- Un-dead-code `extract_module_rules` — remove `#[allow(dead_code)]`, wire into `collect_workspace_extraction_results`
- Update `compute_tag_specs`:
  ```rust
  fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
      let mut specs = django_builtin_specs();
      // Merge workspace results (tracked, auto-invalidating)
      let workspace_results = collect_workspace_extraction_results(db, project);
      for (module_path, extraction) in &workspace_results {
          specs.merge_extraction_results(extraction);  // already exists on TagSpecs
      }
      // Merge external results (from Project field)
      for (_module_path, extraction) in project.extracted_external_rules(db) {
          specs.merge_extraction_results(extraction);
      }
      specs
  }
  ```
- Update `compute_filter_arity_specs` similarly — iterate both workspace and external results
- Update `refresh_inspector`:
  - Build search paths from existing `Project` fields (interpreter, root, pythonpath)
  - Use `resolve_modules()` to partition inventory modules into workspace/external
  - Only run extraction on external modules, store per-module in `FxHashMap`
  - Update `project.extracted_external_rules` with the `FxHashMap`
  - Workspace modules are NOT extracted here — `collect_workspace_extraction_results` handles them via Salsa tracking
- Update `Project::bootstrap` — change `extracted_external_rules` initializer to `FxHashMap::default()`
- Update all tests that construct `Project` or mock `extracted_external_rules`

### 3. Corpus Crate — NEW CRATE: `crates/djls-corpus/`

**Source**: Copy entire `../detailed-opus-4.6/crates/djls-corpus/` directory

**Contents**:
- `Cargo.toml` — needs `anyhow`, `camino`, `flate2` (new), `reqwest` (new), `serde_json`, `serde`, `tar` (new), `toml`, `walkdir`
- `src/lib.rs`, `src/main.rs`, `src/manifest.rs`, `src/sync.rs`, `src/enumerate.rs`
- `manifest.toml` — package/repo definitions
- `.corpus/` — downloaded data (gitignored)

**Workspace `Cargo.toml` changes needed**:
- Add to `[workspace.dependencies]`: `djls-corpus = { path = "crates/djls-corpus" }`
- Add new deps: `flate2 = "1.0"`, `reqwest = { version = "0.12", features = ["blocking", "json"] }`, `tar = "0.4"`

**`crates/djls-extraction/Cargo.toml` changes**:
- Add to `[dev-dependencies]`: `djls-corpus = { workspace = true }`, `walkdir = { workspace = true }`

**Test adaptation**: Corpus tests in the extraction crate call `extract_rules(source)` (one arg) in detailed-opus. In intent-opus the signature is `extract_rules(source, module_path)` — all calls need the second argument. The `module_path_from_corpus_file()` helper derives it from the file path.

### 4. Corpus Template Validation Tests

**Source**: `../detailed-opus-4.6/crates/djls-server/tests/corpus_templates.rs`

**What**: Integration tests parsing real templates through the full validation pipeline.

**Depends on**: Corpus crate (item 3) and workspace/external partitioning (item 2).

**Adaptation**: The test builds a `DjangoDatabase` and populates it with extracted rules. The `Db` trait and `TagSpecs::merge_extraction_results` are the same, but the `ExtractionResult` shape differs, so the test helper that builds specs needs to use intent-opus's HashMap-keyed `ExtractionResult`.

## What NOT to Bring

- ❌ **`RuleCondition` / `ExtractedRule` data model** — intent-opus's `TagRule` with `ArgumentCountConstraint` is better
- ❌ **Per-pass opaque region computation** — intent-opus computes once and threads through
- ❌ **Stubbed argument completions** — intent-opus already has working completions
- ❌ **Monolithic `load_resolution.rs`** — intent-opus's 4-file split is better
- ❌ **`compute_opaque_tag_map` tracked query** — intent-opus handles opaque regions correctly without it
- ❌ **Anything from kimi** — incomplete and unreliable

## Execution Order

### Phase 0: Restore Closer Argument Validation (functional fix)

1. Add `MatchArgSpec` struct to `crates/djls-semantic/src/blocks/grammar.rs`:
   ```rust
   #[derive(Clone, Debug, PartialEq, Eq)]
   struct MatchArgSpec {
       name: String,
       required: bool,
       position: usize,
   }
   ```
2. Add `match_args: Vec<MatchArgSpec>` to `EndMeta`
3. Restore `CloseValidation` variants: `ArgumentMismatch`, `MissingRequiredArg`, `UnexpectedArg`
4. Port `validate_close()` logic and `extract_arg_value()` helper from `../detailed-opus-4.6/crates/djls-semantic/src/blocks/grammar.rs`
5. Remove underscore prefixes from `_opener_bits` and `_closer_bits` parameters
6. Restore match arms in `crates/djls-semantic/src/blocks/builder.rs` for the new variants
7. Wire `MatchArgSpec` population in `TagIndex::from_tag_specs` — detailed-opus derives from `TagSpec.args` (removed in M9). Need to either:
   - Derive from `TagSpec.extracted_rules.extracted_args` (look for positional args on closer tags)
   - Or hardcode for the known Django case (`{% endblock %}` mirrors `{% block %}`'s first arg)
   - Note: `{% block %}` is the only Django tag where closer argument matching matters in practice
8. Add tests: `{% endblock wrong_name %}` → error, `{% endblock %}` → valid, `{% endblock content %}` matching `{% block content %}` → valid
9. Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`
10. Commit: "Restore closer argument validation from detailed-opus"

### Phase 1: Corpus Crate (no behavior change, additive only)

1. Copy `../detailed-opus-4.6/crates/djls-corpus/` to `crates/djls-corpus/`
2. Add to workspace `Cargo.toml`:
   - `djls-corpus = { path = "crates/djls-corpus" }` under `[workspace.dependencies]`
   - `flate2 = "1.0"`, `reqwest = { version = "0.12", features = ["blocking", "json"] }`, `tar = "0.4"` under `[workspace.dependencies]`
3. Add `djls-corpus = { workspace = true }` and `walkdir = { workspace = true }` to `crates/djls-extraction/Cargo.toml` `[dev-dependencies]`
4. Port corpus tests from `../detailed-opus-4.6/crates/djls-extraction/tests/corpus.rs` — adapt `extract_rules(source)` → `extract_rules(source, module_path)` calls
5. Add `.corpus/` to root `.gitignore` if not already present
6. Verify: `cargo build -q`, `cargo test -p djls-corpus -q`, `cargo test -p djls-extraction -q`
7. Commit: "Add djls-corpus crate and corpus extraction tests"

### Phase 2: Module Resolution (no behavior change, additive only)

1. Copy `../detailed-opus-4.6/crates/djls-project/src/resolve.rs` to `crates/djls-project/src/resolve.rs`
2. Move `build_search_paths()` and `find_site_packages()` from `crates/djls-server/src/db.rs` into `resolve.rs` — these are pure functions that derive search paths from `Interpreter` + root + pythonpath. Keep the originals in `db.rs` temporarily (call the new ones) or replace all call sites in one go.
3. Add `pub mod resolve;` to `crates/djls-project/src/lib.rs`
4. Export: `pub use resolve::{ModuleLocation, ResolvedModule, resolve_module, resolve_modules, build_search_paths};`
5. Note: `resolve.rs` needs `use crate::Interpreter;` for `build_search_paths` — the `Interpreter` type is already in `djls-project`
6. Check that `tempfile` is already in `[dev-dependencies]` of `djls-project` (it is)
7. Verify: `cargo build -q`, `cargo test -p djls-project -q`
8. Commit: "Add module path resolution with workspace/external classification"

### Phase 3: Workspace/External Partitioning (behavior improvement)

This is the biggest phase. Changes touch `djls-project` and `djls-server`.

**Step 3a: Update Project salsa input**
1. In `crates/djls-project/src/project.rs`:
   - Change: `pub extracted_external_rules: Option<ExtractionResult>` → `#[returns(ref)] pub extracted_external_rules: FxHashMap<String, ExtractionResult>`
   - Add `use rustc_hash::FxHashMap;` import if not present
2. Update `Project::bootstrap` to initialize: `extracted_external_rules: FxHashMap::default()`
3. Fix all compilation errors from the type change (grep for `extracted_external_rules`)
4. Verify: `cargo build -q`

**Step 3b: Add workspace extraction collection**
1. In `crates/djls-server/src/db.rs`:
   - Add import: `use djls_project::resolve::{resolve_modules, build_search_paths};`
   - Remove `#[allow(dead_code)]` from `extract_module_rules`
   - Add `collect_workspace_extraction_results` as a `#[salsa::tracked]` function that:
     - Reads `project.inspector_inventory(db)`, `project.interpreter(db)`, `project.root(db)`, `project.pythonpath(db)` — these reads establish Salsa dependencies
     - Builds search paths via `build_search_paths(interpreter, root, pythonpath)`
     - Collects registration module paths from inventory tags and filters
     - Calls `resolve_modules()` to partition workspace vs external
     - For each workspace module: `db.get_or_create_file(&path)` → `extract_module_rules(db, file)`
     - Returns `Vec<(String, ExtractionResult)>` of non-empty results

**Step 3c: Update compute queries**
1. Update `compute_tag_specs` to merge workspace results + external results (iterate `FxHashMap`)
2. Update `compute_filter_arity_specs` to iterate both sources
3. Verify: `cargo build -q`

**Step 3d: Update refresh_inspector**
1. Remove inline `resolve_module_to_file()` function (replaced by `resolve.rs`)
2. In `refresh_inspector()`:
   - Build search paths from `project.interpreter(db)`, `project.root(db)`, `project.pythonpath(db)`
   - Use `resolve_modules()` to partition inventory modules into workspace/external
   - Only run extraction on external modules, store per-module in `FxHashMap`
   - Update `project.extracted_external_rules` with the `FxHashMap`
   - Do NOT extract workspace modules here — `collect_workspace_extraction_results` handles those via Salsa file tracking
3. Verify: `cargo build -q`, `cargo clippy -q --all-targets --all-features -- -D warnings`, `cargo test -q`
4. Commit: "Wire workspace extraction through Salsa tracked queries"

### Phase 4: Corpus Template Validation Tests (testing improvement)

1. Copy `../detailed-opus-4.6/crates/djls-server/tests/corpus_templates.rs` to `crates/djls-server/tests/corpus_templates.rs`
2. Adapt test helpers:
   - `build_specs_for_entry()` — use intent-opus's `TagSpecs::merge_extraction_results(&ExtractionResult)`
   - Construct `ExtractionResult` matching intent-opus's HashMap-based shape
   - Update `Db` trait methods to match intent-opus's `SemanticDb` trait
3. Tests should skip gracefully when corpus not synced (check dir existence, return early)
4. Verify: `cargo test -q`
5. Commit: "Add corpus template validation integration tests"

### Phase 5: AGENTS.md Refresh

1. Update `AGENTS.md` with:
   - New file locations (`resolve.rs`, corpus crate)
   - Updated `Project` field documentation (changed `extracted_external_rules` type)
   - `collect_workspace_extraction_results` tracked query documentation
   - Note that search paths are derived (not stored) — `build_search_paths(interpreter, root, pythonpath)` in `resolve.rs`
   - Universal operational notes from detailed-opus's AGENTS.md (clippy patterns, Salsa patterns, trait impl locations)
2. Commit: "Update AGENTS.md with consolidation changes"

## Risk Assessment

| Risk | Likelihood | Mitigation |
|---|---|---|
| `ExtractionResult` type change on `Project` breaks callers | **High** | Grep for `extracted_external_rules` — every access point needs updating. Main locations: `compute_tag_specs`, `compute_filter_arity_specs`, `refresh_inspector`, tests |
| `merge_extraction_results` doesn't handle per-module iteration | Medium | It currently takes a single `&ExtractionResult`. For the FxHashMap approach, iterate the map values and call merge for each |
| Salsa invalidation regressions in Phase 3 | Low | Existing tests cover tracked query caching; add a test that workspace file change triggers re-extraction |
| Corpus crate new deps (`reqwest`/`flate2`/`tar`) conflict | Low | Pin to same versions as detailed-opus; they're only dev/build deps |
| `build_search_paths` heuristic misses paths | Low | The heuristic covers venv + PYTHONPATH + project root — handles the common case. Can upgrade to a `sys.path` inspector query later without architecture changes |
| `get_or_create_file` not available on `&dyn SemanticDb` | Medium | This method is on `DjangoDatabase` directly. In tracked functions you have `&dyn SemanticDb`. Check if `WorkspaceDb` trait provides it, or if `collect_workspace_extraction_results` needs a different approach |

## Reference: Key Differences Between the Two Implementations

| Aspect | intent-opus (this repo) | detailed-opus (reference) |
|---|---|---|
| `ExtractionResult` | `FxHashMap<SymbolKey, TagRule>` + 2 more maps | `Vec<ExtractedTag>` + `Vec<ExtractedFilter>` |
| `extract_rules()` | `fn(source, module_path) -> ExtractionResult` | `fn(source) -> Result<ExtractionResult>` |
| `TagSpec.extracted_rules` | `Option<TagRule>` | `Vec<ExtractedRule>` |
| `Project.extracted_external_rules` | `Option<ExtractionResult>` (single blob) | `FxHashMap<String, ExtractionResult>` (per-module) |
| `Project.sys_path` | does not exist (derived from interpreter/root/pythonpath) | `Vec<Utf8PathBuf>` (stored as Salsa input — unnecessary) |
| Module resolution | inline in `db.rs` (~100 lines) | `djls-project/src/resolve.rs` (236 lines) |
| Workspace tracking | dead code (`extract_module_rules` unused) | live (`collect_workspace_extraction_results` calls it) |
| Argument completions | **working** (uses `ExtractedArg`) | **stubbed** (returns `Vec::new()`) |
| Snippets | **working** (`snippets.rs`) | does not exist |
| If-expression validation | **working** (S114) | **working** (S114) |
| Filter arity validation | **working** (S115/S116) | **working** (S115/S116) |
| Opaque regions | computed once, threaded through | recomputed per validation pass |
| Corpus crate | does not exist | `crates/djls-corpus/` with NetBox templates |

## Notes

- The feature-gated `parser` on `djls-extraction` is present in both and should be preserved — it keeps the Ruff parser dependency out of crates that only need the types.
- Both implementations converge on the same macro-architecture, which validates the plan's high-level design.
- When reading files from detailed-opus, always adapt to intent-opus's conventions — don't blindly copy. The data models differ significantly.
