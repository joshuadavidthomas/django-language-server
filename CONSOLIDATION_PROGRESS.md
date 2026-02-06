# Consolidation Progress

## Phases

- [x] **Phase 1**: Corpus Crate — copy `djls-corpus`, add workspace deps, port corpus extraction tests, adapt `extract_rules` calls
- [x] **Phase 2**: Module Resolution — copy `resolve.rs` to `djls-project`, move `build_search_paths`/`find_site_packages`, export types
- [ ] **Phase 3**: Workspace/External Partitioning — update `Project` salsa input, add `collect_workspace_extraction_results`, update compute queries and `refresh_inspector`
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
