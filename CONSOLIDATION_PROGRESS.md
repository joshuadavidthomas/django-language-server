# Consolidation Progress

## Phases

- [x] **Phase 1**: Corpus Crate — copy `djls-corpus`, add workspace deps, port corpus extraction tests, adapt `extract_rules` calls
- [x] **Phase 2**: Module Resolution — copy `resolve.rs` to `djls-project`, move `build_search_paths`/`find_site_packages`, export types
- [x] **Phase 3**: Workspace/External Partitioning — update `Project` salsa input, add `collect_workspace_extraction_results`, update compute queries and `refresh_inspector`
- [ ] **Phase 4**: Corpus Template Validation Tests — port integration tests from `djls-server/tests/corpus_templates.rs`
- [ ] **Phase 5**: AGENTS.md Refresh — update with new file locations, updated field docs, operational notes

## Notes

### Phase 1
- `djls-corpus` crate copied as-is (manifest, sync, enumerate modules + CLI main)
- Added `flate2`, `tar`, `reqwest` as direct deps in the crate (not workspace deps — they're only used by the corpus binary)
- Integration tests in `tests/corpus.rs` and `tests/golden.rs` adapted to this codebase's API:
  - `extract_rules(source, module_path)` (two args) instead of `extract_rules(source)` (one arg)
  - `ExtractionResult.tag_rules` / `filter_arities` / `block_specs` (FxHashMaps) instead of `Vec<ExtractedTag>` / `Vec<ExtractedFilter>`
  - `ArgumentCountConstraint::Min(4)` instead of `RuleCondition::MaxArgCount { max: 3 }`
  - No `ExtractionError` type — `extract_rules` returns empty result on parse failure
- Integration tests gated via `required-features = ["parser"]` in Cargo.toml
- All tests skip gracefully when corpus not synced
- `.corpus/` added to `.gitignore`

### Phase 2
- Created `crates/djls-project/src/resolve.rs` with:
  - `ModuleLocation` enum (Workspace/External) for classifying resolved modules
  - `ResolvedModule` struct (module_path, file_path, location)
  - `resolve_module()` — single module resolution with location classification
  - `resolve_modules()` — batch resolution returning (workspace, external) partitioned tuple
  - `build_search_paths()` — moved from `djls-server/src/db.rs`, builds sys_path from interpreter/root/pythonpath
  - `find_site_packages()` — moved from `djls-server/src/db.rs`, locates venv site-packages
  - 6 unit tests covering workspace/external resolution, sys_path ordering, package __init__.py, batch partitioning
- Updated `crates/djls-project/src/lib.rs` — added `mod resolve` and 6 public re-exports
- Updated `crates/djls-server/src/db.rs`:
  - `extract_external_rules` now uses `djls_project::resolve_module` instead of local `resolve_module_to_file`
  - Removed 4 local functions: `build_search_paths`, `find_site_packages`, `find_site_packages_in_venv`, `resolve_module_to_file`
  - Removed 3 tests that tested the now-removed local functions (equivalent tests live in `djls-project::resolve`)

### Phase 3
- **Project salsa input changed**: `extracted_external_rules: Option<ExtractionResult>` → `FxHashMap<String, ExtractionResult>` (per-module keying)
  - `Project::bootstrap` initializes with `FxHashMap::default()` instead of `None`
  - All tests updated: use `FxHashMap` with `insert()` instead of `Some(extraction)`
- **`collect_workspace_extraction_results` added** as `#[salsa::tracked]` function in `db.rs`:
  - Reads `inspector_inventory`, `interpreter`, `root`, `pythonpath` from Project (establishes Salsa deps)
  - Builds search paths via `build_search_paths()` (derived, not stored)
  - Uses `resolve_modules()` to partition workspace vs external
  - For workspace modules: `get_or_create_file()` → `extract_module_rules()` (tracked per-file)
  - Returns `Vec<(String, ExtractionResult)>` of non-empty results
- **`extract_module_rules`** — removed `#[allow(dead_code)]`, now called by `collect_workspace_extraction_results`
- **`compute_tag_specs` updated** — merges workspace results (from tracked query) + external results (from Project field)
- **`compute_filter_arity_specs` updated** — same dual-source pattern
- **`extract_external_rules` updated** — now returns `FxHashMap<String, ExtractionResult>` (per-module), only extracts external modules (workspace skipped)
- **`refresh_inspector` updated** — `unwrap_or_default()` for the extraction result instead of wrapping in `Option`
- No `sys_path` field added to Project — search paths derived at call sites from `interpreter`/`root`/`pythonpath` via `build_search_paths()`
- Key difference from detailed-opus: no `FileKind` check needed (intent-opus `File` doesn't have kinds), and `extract_rules()` takes 2 args (source, module_path)
