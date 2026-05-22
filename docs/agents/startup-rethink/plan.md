# Plan: startup-rethink

## Overview
Rework DJLS startup into a rust-analyzer-style loading model: the LSP handshake becomes protocol-only, cheap file and Project Facts load in background tasks, static Django Discovery becomes the normal source of startup Project Facts, and runtime Project Introspection becomes optional enrichment.

This plan is the authoritative implementation sequence for startup-rethink. The outline is high-level background; when the outline and this plan disagree about phase mechanics, this plan wins.

The whole plan is the reviewable PR-sized change that will land. Each phase or subphase is a commit-sized implementation slice inside that PR, not an independently shippable pull request. Each slice should be self-contained enough to compile, test, and hand off to the next slice, but the project is not shipping the slices separately; temporary feature gaps or rough intermediate behavior are acceptable when they are named and deleted by a later phase. Do not contort the design to make every intermediate phase a polished product state. If a phase check fails, halt and surface the failure; do not silently redesign the next phase around it.

## Implementation Status

Keep this section current while implementing the plan.

- **Implementation bookmark**: `startup-rethink` points to the latest verified implementation slice.
- **Implementation change**: `tqtwupmz` contains the completed static template inventory slice.
- **Current slice**: Phase 6C completed; Phase 6D is next.

### Implementation Notes

Add one entry at the end of each completed implementation slice, after validation passes and before starting the next `jj` change. Use a short human-readable slice name rather than a phase number as the heading. Include the bookmark, current change ID, scope, validation commands, and follow-ups/blockers. Keep entries newest last so this section reads as an implementation log.

Do not keep placeholder slice headings in this live log. If an example is needed, keep it outside this section so future readers do not mistake it for an implemented slice.

### Protocol-ready startup
- Bookmark: `startup-rethink` still points to planning baseline `sqoqvvrn`; move it to `nyntuxws` after describing this verified slice.
- Current change: `nyntuxws`.
- Scope: removed implicit Project bootstrap from `DjangoDatabase::new`; added explicit legacy project bootstrap for project-aware callers; made `Session::new` capture roots and use client settings only; made `initialized` log and return; kept configuration reload settings storage working without a Project; added temporary no-Project availability adapter; added pytest-lsp startup smoke tests.
- Validation:
  - `cargo test -q` baseline passed before edits.
  - `cargo test -p djls-server session::tests::session_new` passed.
  - `cargo test -p djls-server degraded_no_project` passed.
  - `uv run pytest tests/lsp/test_startup.py -k "initialize_returns_capabilities or server_stays_responsive_after_initialized"` passed.
  - `uv run ruff check tests/lsp/test_startup.py` passed.
  - `cargo test -p djls-server` passed.
  - `cargo test -p djls --test check` passed with custom project tagspec coverage.
  - `cargo build -q` passed.
  - `just fmt --check` passed.
- Follow-ups/blockers: Phase 3C must move/delete `crates/djls-semantic/src/availability.rs` into `djls-project::availability` or narrow it to a semantic-only adapter.

### Neutral source/workspace primitives
- Bookmark: `startup-rethink` points to `xsnutlnv`.
- Current change: `xsnutlnv`.
- Scope: added neutral `SourceFileSet` data types in `djls-source`; added source-root identity and discovered/loaded source-file types; added neutral workspace file loading over existing `walk_files`; preserved traversal mechanics without Django policy or readiness state.
- Validation:
  - `cargo test -p djls-source file_set` passed: 6 tests.
  - `cargo test -p djls-workspace file_loader` passed: 7 tests.
  - `cargo build -q` passed.
- Follow-ups/blockers: Phase 3 owns root construction, project-loading readiness, partition patches, and database materialization.

### Source-file node through CLI
- Bookmark: `startup-rethink` points to `snvkzvko` after the review follow-up.
- Current change: `snvkzvko`.
- Scope: added the Phase 3 one-node loading plan, `source-file-set` `NODE_SPECS` manifest row, readiness-to-terminal projection, neutral runner, source-file-specific effects contract, observer event sink, and CLI effect adapter; wired no-explicit-path `djls check` through `run_loading_plan` while keeping targeted path checks from paying the project-wide loading walk until source-file facts feed check behavior.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project loading` passed: 23 tests after review follow-up.
  - `cargo test -p djls --test check` passed: 7 tests.
  - `cargo build -q` passed.
- Follow-ups/blockers: Phase 3A4 adds the LSP generation guard, guarded reset/apply, LSP source-file effect adapter, progress lifecycle, and configuration restart.

### LSP generation guard and guarded apply
- Bookmark: `startup-rethink` points to `toyvwmzs`.
- Current change: `toyvwmzs`.
- Scope: added server-local startup generation primitives, immutable `StartupRunInputs` / `ProjectLoadingSnapshot` capture, versioned open-document snapshots, typed stale-document apply rejection with file/path/captured/current evidence, guarded apply/observe outcomes, and guarded reset coverage.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-server startup_generation` passed: 10 tests after review follow-up.
  - `cargo build -q` passed.
- Follow-ups/blockers: Phase 3A4b wires the LSP source-file executor through the neutral loading runner; current generation primitives are intentionally not connected to `initialized` yet.

### LSP source-file executor
- Bookmark: `startup-rethink` will move to `kttmzkwn` after describing this verified slice.
- Current change: `kttmzkwn`.
- Scope: added the server-local `LspLoadingExecutor` for the `source-file-set` node, running `LoadingPlan::phase3()` through `run_loading_plan`, applying source-file reset/update through `GenerationGuard`, rejecting stale captured open-document state before project facts apply, and keeping blocked source-file activity outside the shared `Session` lock.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-server startup_source_files` passed: 1 test.
  - `cargo test -p djls-server startup_request_while_loading` passed: 1 test.
  - `cargo build -q` passed.
- Follow-ups/blockers: Phase 3A4c adds work-done progress capability parsing and progress/log reporting over the existing loading observer events; the LSP executor remains intentionally not connected to `initialized` until the progress lifecycle slice.

### LSP source-file executor review follow-up
- Bookmark: `startup-rethink` points to `luktmluq`.
- Current change: `luktmluq`.
- Scope: made loading-run control explicit in the neutral runner/effects seam, stopped superseded LSP resets before source-file activity starts, removed synthetic project-readiness fallback laundering for LSP execution outcomes, and made stale-document rejection write terminal query-visible source-file availability before returning a rejected apply outcome.
- Validation:
  - `cargo test -p djls-project loading` passed: 23 tests.
  - `cargo test -p djls-server startup_source_files` passed: 3 tests.
  - `cargo test -p djls-server startup_request_while_loading` passed: 1 test.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Follow-ups/blockers: Phase 3A4c can build progress lifecycle on `LoadingRunResult::execution_outcome` instead of a server-side outcome side channel.

### LSP startup progress lifecycle
- Bookmark: `startup-rethink` points to `vwkwqxpn`.
- Current change: `vwkwqxpn`.
- Scope: parsed `window.workDoneProgress` into `ClientInfo`, added `StartupProgress` as the LSP observer/log fallback boundary, reported begin/node/finish events from the existing loading runner, and centralized startup progress finish through the typed `StartupRunOutcome` returned by the inner source-file runner.
- Validation:
  - `cargo test -p djls-server client::tests::work_done_progress` passed: 3 tests.
  - `cargo test -p djls-server startup_progress` passed: 2 tests.
  - `cargo test -p djls-server startup_source_files` passed: 3 tests.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Follow-ups/blockers: Phase 3A4d wires startup/configuration restart entrypoints to capture `StartupRunInputs` with the real client progress adapter.

### LSP startup progress lifecycle review follow-up
- Bookmark: `startup-rethink` will move to `zzwtomox` after describing this verified slice.
- Current change: `zzwtomox`.
- Scope: made work-done progress tokens generation-scoped, moved LSP progress create/notify work onto a nonblocking dispatcher so progress IO cannot gate loading execution, added explicit work-done progress state that only emits begin/report/end after successful token creation, and removed the unused recording-only log event.
- Validation:
  - `cargo test -p djls-server work_done_progress` passed: 5 tests.
  - `cargo test -p djls-server startup_progress` passed: 3 tests.
  - `cargo test -p djls-server startup_source_files` passed: 3 tests.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Follow-ups/blockers: Phase 3A4d can pass the active `StartupGeneration` into `StartupProgress::for_client(...)` when wiring real startup/configuration restarts.

### Architecture correction planning
- Bookmark: `startup-rethink` points to `rqpkyvqm`.
- Current change: `rqpkyvqm`.
- Scope: recorded rust-analyzer/Ruff-ty evidence, assessed the current implementation and future phases, accepted a stable `djls_project::Project` root input as the semantic Project Facts model, inserted a required pre-3B architecture correction gate, marked completed `ProjectLoadingState` slices as superseded history, and rewrote current/future phase prose to target stable `Project` facts plus server/CLI orchestration.
- Validation: docs/planning only; implementation validation is defined in the Architecture correction gate.
- Follow-ups/blockers: do not implement Phase 3A4d/3B feature work until the stable Project root cleanup gate passes.

### Stable Project root implementation
- Bookmark: `startup-rethink` still points to `rqpkyvqm`; move it to `zzvlwosx` after describing this verified slice.
- Current change: `zzvlwosx`.
- Scope: replaced the Salsa-visible `ProjectLoadingState` readiness singleton with stable `djls_project::Project`, moved source-file facts to `Project.source_inventory`, initialized the Project handle once in production/bench/test databases, removed run-start `Loading`/`Stale` Project Fact writes, and changed stale-document rejection to leave Project Facts unchanged.
- Validation:
  - `cargo test -p djls-db --no-run` passed.
  - `cargo test -p djls-bench --no-run` passed.
  - `cargo test -p djls-semantic --no-run` passed.
  - `cargo test -p djls-db source_file_set` passed: 5 tests.
  - `cargo test -p djls-project loading` passed: 21 tests.
  - `cargo test -p djls-server startup_source_files` passed: 3 tests.
  - `cargo test -p djls-server startup_request_while_loading` passed: 1 test.
  - `cargo test -p djls --test check` passed: 7 tests.
  - `rg "project_loading_state|ProjectLoadingState" crates -g '*.rs'` returned no matches.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Lamport review and Rust specialist review both requested stronger LSP preservation coverage; added prior-`Ready` assertions for stale-document rejection and superseded runs.
  - Rust specialist advisory cleanup removed now-dead `ProjectSourceFilesIssue::StaleDocument` and dead `TerminalSourceFilesAvailability::Deferred`.
  - Librarian found no major divergence from rust-analyzer and Ruff/ty: ty uses a stable Salsa `Project` input updated in place, and rust-analyzer keeps loading/progress/supersession in orchestration while lowering durable facts into incremental inputs.
- Follow-ups/blockers: none for the architecture correction gate; Phase 3A4d may resume next.

### Configuration restart through startup controller
- Bookmark: `startup-rethink` still points to `zzvlwosx`; move it to `uorlmwwk` after describing this verified slice.
- Current change: `uorlmwwk`.
- Scope: wired `initialized` to start the source-file loading graph with client progress, routed env-changing `didChangeConfiguration` through the same `StartupController` generation path, removed production use of the old runtime-refresh queue, tightened generation supersession so newer generations become active before apply linearization waits, and republished diagnostics after configuration-triggered loading completes.
- Validation:
  - `cargo test -p djls-server configuration_restart` passed: 1 test.
  - `cargo test -p djls-server startup` passed: 20 tests.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Lamport review found a race where configuration changes could wait behind an older apply; fixed by marking the new generation active before waiting for apply linearization and rechecking generation after acquiring the session lock.
  - Rust specialist requested post-restart diagnostic republish ordering; fixed by awaiting configuration-triggered loading before republishing diagnostics.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty restart patterns. It noted mature servers avoid holding the main/session lock while waiting and prefer pull diagnostic refresh when available; this slice waits only outside the session lock and keeps existing push/pull diagnostic behavior.
- Follow-ups/blockers: Phase 3B discovery/enrichment Project-root scaffolding is next.

### Discovery/enrichment Project-root scaffolding
- Bookmark: `startup-rethink` still points to `uorlmwwk`; move it to `lvnkyyyr` after describing this verified slice.
- Current change: `lvnkyyyr`.
- Scope: added `ProjectDiscovery`, `ProjectDiscoverySet`, root-scoped `RootDiscoveryInput`, project-owned Django environment/settings seeds, canonical `ProjectEnvVars`, non-empty discovery/enrichment issue wrappers, and optional `ProjectEnrichment` facts under the stable `Project` root. Kept loading/progress/generation state out of discovery/enrichment facts and did not wire config loading or discovery apply yet.
- Validation:
  - `cargo test -p djls-project discovery` passed: 10 tests.
  - `cargo test -p djls-db --no-run` passed.
  - `cargo test -p djls-bench --no-run` passed.
  - `cargo test -p djls-semantic --no-run` passed.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Hickey review and Rust specialist review both rejected implicit duplicate env-var resolution; `ProjectEnvVars` now accepts only already-resolved unique entries and canonicalizes after duplicate detection.
  - Rust specialist requested `#[returns(ref)]` for owned Salsa fields and stronger invariants; added ref-returning discovery/enrichment fields and non-empty constructors for ready/unavailable discovery/enrichment states.
  - Rust specialist flagged the option-matrix environment seed; replaced it with a named settings-module seed variant.
  - Librarian found no major reversal needed. Phase 3C must keep interpreter/module-search facts and resolved settings as core root-scoped Project Facts once semantics depend on them, apply env precedence before constructing `ProjectEnvVars`, and distinguish missing config fallback from invalid config.
- Follow-ups/blockers: Phase 3C should preserve the reference-check constraints above while wiring structured settings load and discovery apply.

### Structured root settings load
- Bookmark: `startup-rethink` still points to `lvnkyyyr`; move it to `urrpmlnp` after describing this verified slice.
- Current change: `urrpmlnp`.
- Scope: added `djls_conf::load_root_settings` with root-scoped `RootSettingsLoadOutcome`, effective source path, typed issue categories, fallback-after-error marker, and tests for missing config, unrelated `pyproject.toml`, invalid TOML, source provenance, and client override behavior.
- Validation:
  - `cargo test -p djls-conf root_settings_load` passed: 6 tests.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Hickey review and Rust specialist review rejected guessed source provenance and lossy parse/schema classification; root settings loading now parses candidate files directly enough to distinguish unrelated `pyproject.toml`, syntax parse failures, schema/deserialization failures, unsupported shapes, and effective project config source.
  - Rust specialist clarified that client/default settings can override a successful root config without being recorded as fallback-after-error; this behavior is covered by test.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It noted ty preserves per-value provenance; this slice keeps root/effective-source provenance only, with per-value provenance deferred until a future diagnostic actually needs it.
- Follow-ups/blockers: Phase 3C2 should lower this structured outcome into project-owned discovery facts without reverse-engineering `ConfigError` strings.

### Discovery data and Project apply
- Bookmark: `startup-rethink` still points to `urrpmlnp`; move it to `ynmqxous` after describing this verified slice.
- Current change: `ynmqxous`.
- Scope: added `ProjectDiscoveryLoadRequest`, `ProjectDiscoverySetData`, `RootDiscoveryData`, shared `build_project_discovery_data`, structured env-file load outcomes, typed config/env provenance lowering, project-owned environment seed lowering, canonical env-var construction after duplicate resolution, and `DjangoDatabase::apply_project_discovery_data` that mutates stable `Project.discovery` through setters.
- Validation:
  - `cargo test -p djls-project loading_settings` passed: 5 tests.
  - `cargo test -p djls-project discovery_invalidation` passed: 1 test.
  - `cargo test -p djls-db project_discovery` passed: 2 tests.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Hickey review and Rust specialist review found that failed empty discovery data could overwrite old facts; fixed by rejecting empty data without changing `Project.discovery`.
  - Reviews found env-file failures and duplicate env vars were logging-only/implicit; added structured env-file outcomes, `EnvFileLoadFailed` / `DuplicateEnvVar` discovery issues, and deterministic last-wins duplicate resolution before constructing `ProjectEnvVars`.
  - Rust specialist found repeated identical discovery data would allocate fresh `RootDiscoveryInput` handles and invalidate Salsa; apply now compares plain data against existing `RootDiscoveryInput` fields before allocating/setter calls.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. Carry-forward caution: future provenance should not participate in semantic equality when it would cause avoidable invalidation; keep resolved semantic values distinct from diagnostic provenance when that matters.
- Follow-ups/blockers: Phase 3C3 should add the `project-discovery-set` loading node and wire both CLI/LSP adapters using the shared discovery data/apply seams.

### Project-discovery loading node
- Bookmark: `startup-rethink` still points to `ynmqxous`; move it to `ynlpuktv` after describing this verified slice.
- Current change: `ynlpuktv`.
- Scope: added the `project-discovery-set` node to the active Phase 3 loading plan after `source-file-set`, extended the loading effects/driver contract for discovery data load/apply, wired CLI and LSP executors through the shared discovery activity and stable `Project.discovery` apply method, derived discovery roots from the same canonical source-root plan, and projected clean/degraded/unavailable discovery outcomes into terminal node status.
- Validation:
  - `cargo test -p djls-project loading` passed: 27 tests.
  - `cargo test -p djls-db project_discovery` passed: 2 tests.
  - `cargo test -p djls-server startup` passed: 20 tests.
  - `cargo test -p djls --test check` passed: 7 tests.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Lamport review found empty discovery input could preserve stale ready discovery facts; empty discovery apply now writes an unavailable no-workspace-roots discovery fact instead.
  - Rust specialist requested canonical root consistency, degraded status for recoverable discovery issues, and explicit projection tests; discovery loading now derives roots from `build_source_roots`, ready-with-issues maps to `Degraded`, and plan tests cover clean/degraded/deferred/unavailable outcomes.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed that mature tools keep loading/supersession outside Salsa, derive loading from canonical roots, model partial discovery as usable/degraded state, and mutate a stable project handle through targeted setters.
- Follow-ups/blockers: Phase 3C4 should move pure Project Facts availability projection into `djls-project::availability` and extend degraded request behavior for absent/unavailable discovery facts.

### Availability/request matrix
- Bookmark: `startup-rethink` still points to `ynlpuktv`; move it to `sxrlwqyu` after describing this verified slice.
- Current change: `sxrlwqyu`.
- Scope: moved pure Project Facts availability classification into `djls-project::availability`, deleted the temporary semantic availability module, exposed a narrow `Session::project_facts_availability` request boundary, preserved non-empty discovery issues in the availability API, logged availability from template request handlers, and added degraded absent/unavailable discovery request tests for diagnostics, completions, hover, definition, and references.
- Validation:
  - `cargo test -p djls-project availability` passed: 3 tests.
  - `cargo test -p djls-server degraded` passed: 2 tests.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
  - `rg "ProjectFactsAvailability|degraded_no_project|availability" crates/djls-semantic crates/djls-ide crates/djls-server -g '*.rs'` shows only the server session/request boundary and unrelated scoping-symbol availability comments/tests; no semantic availability bridge remains.
  - `rg "availability|ProjectSourceFiles|FileSetPartition" crates/djls-source crates/djls-db/src/db.rs -g '*.rs'` shows no readiness availability in `djls-source`; `djls-db` references project source-file apply types without Django partition names.
- Review/reference follow-up:
  - Ousterhout review found no must-fix issues and accepted the project-owned availability boundary and degraded request tests.
  - Rust specialist required removing `djls-semantic` passthrough re-exports, preserving the non-empty discovery-issue invariant, and making availability visible in the production request path; fixed by deleting the semantic re-export, carrying `ProjectDiscoveryIssues` through `ProjectDiscoveryUnavailableReason::Failed`, and adding the session helper used by request handlers.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed that project facts belong in the project layer, request handlers should degrade to empty/`None` results, and loading/Salsa internals should stay behind project/session APIs.
- Follow-ups/blockers: Phase 3D should add layout/concrete provenance and continue cleanup of legacy queue/dependency wiring.

### Layout index and queue cleanup
- Bookmark: `startup-rethink` still points to `sxrlwqyu`; move it to `yktvoszl` after describing this verified slice.
- Current change: `yktvoszl`.
- Scope: added `ProjectLayoutIndex` and `project_layout_index` over stable `Project.source_inventory`, explicit absent/unavailable layout outcomes, path/file/name/extension/directory/package lookup APIs, `settings_module_candidates` as the first layout consumer that preserves unavailable layout instead of returning empty candidates, and tests proving layout invalidates on source-inventory changes but not enrichment-only changes. Deleted the obsolete server `Queue` module after confirming startup/discovery no longer use it. Did not introduce generic provenance because Phase 3D has no concrete provenance consumer.
- Validation:
  - `cargo test -p djls-project layout` passed: 4 tests.
  - `cargo test -p djls-server queue` passed: 0 tests after deleting the module.
  - `cargo test -p djls-server startup` passed: 20 tests.
  - `cargo test -p djls-bench --no-run` passed.
  - `cargo build -q` passed.
  - `just fmt --check` passed.
  - `rg "Queue|enqueue|refresh_external_data|load_template_library_cache" crates/djls-server -g '*.rs'` returned no matches.
  - `rg "ProjectLoadingSnapshot|Arc<Mutex<Session>>" crates/djls-project/src -g '*.rs'` returned no matches.
- Review/reference follow-up:
  - Ousterhout review found no must-fix issues and agreed the layout boundary hides `SourceFileSetData` while preserving absent/unavailable source inventory.
  - Rust specialist found no must-fix issues and confirmed the enrichment-only invalidation test covers the core `project_layout_index` dependency claim.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed stable source-root/file-set-backed lookup APIs, explicit unavailable-vs-empty outcomes, Salsa invalidation scoped to source membership, delayed provenance, and deletion of unused queue abstractions all match mature tooling patterns.
- Follow-ups/blockers: Phase 4 should add Python source models on top of the layout/source inventory boundary.

### Name/type move
- Bookmark: `startup-rethink` still points to `yktvoszl`; move it to `qylmxnpq` after describing this verified slice.
- Current change: `qylmxnpq`.
- Scope: moved project-domain name newtypes and `InvalidName` into `djls-project`, added path-like `djls_project::TemplateName`, kept temporary semantic re-exports for existing callers, renamed the semantic Salsa identity to `InternedTemplateName`, and changed legacy `ProjectTemplateFile` to store the domain `TemplateName` while interning only at tracked-query boundaries.
- Validation:
  - `cargo test -p djls-project names` passed: 6 tests.
  - `cargo test -p djls-semantic --no-run` passed.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
  - `rg "TemplateName|LibraryName|PyModuleName|TemplateSymbolName" crates/djls-semantic crates/djls-project -g '*.rs'` shows the new project-owned definitions/exports, temporary semantic re-exports/old semantic callers, and the renamed `InternedTemplateName` identity.
- Review/reference follow-up:
  - Beck review required the semantic compatibility shim to include `InvalidName`; added the temporary re-export while keeping the new `TemplateName` out of semantic re-exports to avoid identity confusion.
  - Rust specialist required `TemplateName` to use template-specific path-like validation and to replace raw template-name strings in the legacy semantic template file model; `TemplateName` now rejects empty/absolute/parent-component names while allowing path-like names with spaces, and `ProjectTemplateFile` stores the new domain type.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed that mature tools keep validated domain newtypes in project/domain crates and separate stable domain values from Salsa/interned identities.
- Follow-ups/blockers: Phase 4B should add the Ruff AST anti-corruption layer and tracked Python source-model queries in `djls-project`.

### Python source model extraction
- Bookmark: `startup-rethink` still points to `wknnmsuv`; move it to `mwwsvlop` after describing this verified slice.
- Current change: `mwwsvlop`.
- Scope: added Ruff parser/AST dependencies to `djls-project`, introduced DJLS-native `PythonSourceModel` / `PythonSourceIndex` types, added tracked `python_source_model(db, file)` and `python_source_index(db, project)` queries, kept Ruff AST nodes private to the extraction boundary, modeled parse failures explicitly, derived indexed module names from `ProjectLayoutIndex` source roots, and represented static literal extraction with typed unknown issues.
- Validation:
  - `cargo test -p djls-project python_source_model` passed: 2 tests.
  - `cargo test -p djls-project python_source_index` passed: 1 test.
  - `cargo test -p djls-project python_source_model --no-run` passed.
  - `just fmt --check` passed.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Hickey review required module-name resolution to use layout/source-root context and parse errors to remain visible; fixed by resolving modules in `python_source_index` through `ProjectLayoutIndex` and adding `PythonSourceModelStatus::ParseError`.
  - Rust specialist required broader recursive AST traversal and usable public accessors; added recursive handling for common statement/expression child shapes plus accessors for call arguments/keywords, class bases, function async-ness, and static value segments. The loading-node concern is intentionally deferred to Phase 4C.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed the AST anti-corruption layer, Salsa query boundaries, explicit parse/readiness outcomes, source-root-derived module names, and typed static unknowns match mature tooling patterns.
- Follow-ups/blockers: Phase 4C should observe `python_source_index(db, project)` through the loading graph and project terminal status from the live query outcome.

### Python source-model readiness observation
- Bookmark: `startup-rethink` still points to `mwwsvlop`; move it to `vorzrswp` after describing this verified slice.
- Current change: `vorzrswp`.
- Scope: added the `python-source-models` loading node after source files and project discovery, introduced generic readiness projection for source files, discovery, and Python source index outcomes, wired neutral loading observation through CLI and LSP adapters, observed `python_source_index(db, project)` without holding the `Session` mutex across tracked-query execution, balanced superseded observation progress events, and added nonblocking request plus query-reuse coverage.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project loading_python_source_models` passed: 3 tests.
  - `cargo test -p djls-project python_source_index_reuse` passed: 1 test.
  - `cargo test -p djls-server python_source_models` passed: 2 tests.
  - `cargo test -p djls --test check` passed: 7 tests.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Lamport review found no must-fix concurrency or state-machine issues.
  - Rust specialist required unique test paths, balanced node lifecycle events on supersession, reusable/public readiness projection, discovery participation in the generic readiness projection, and a loading-path reuse test; addressed all in this slice.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed that incremental query databases, typed task/readiness outcomes, snapshot/background observation, avoiding long global locks, and cancellation/supersession checks match mature tooling patterns.
- Follow-ups/blockers: Phase 4D should reuse the nonblocking observation seam for settings-candidate discovery, or stop and revise if candidate derivation needs a different access pattern.

### Settings candidate discovery
- Bookmark: `startup-rethink` still points to `vorzrswp`; move it to `smwxvrqu` after describing this verified slice.
- Current change: `smwxvrqu`.
- Scope: added project-owned settings candidate types and query, provenance/origin tracking, partial issue reporting, explicit/configured-environment/env-var/manage.py/conventional settings sources, `src/` layout conventional-module handling, invalid module reporting, and test-only fixture helpers for source inventories and discovery sets.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project settings_candidates` passed: 5 settings-related tests.
  - `cargo test -p djls-project testing` passed: 1 helper test.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Hickey review required settings discovery candidates to be independent of layout availability, configured Django environments to be collected, provenance not to be discarded by deduplication, and invalid module values to remain visible; addressed by returning partial issues, adding the configured-environment source, preserving duplicate provenance-bearing candidates, and reporting invalid module issues.
  - Rust specialist required the same partial issue model, explicit source ranking without provenance-dropping deduplication, better `src/` conventional module handling, and private façade module boundaries; addressed in this slice.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed preserving multiple config/project candidates, origin/provenance, partial diagnostics, and avoiding silent default selection align with mature tooling patterns.
- Follow-ups/blockers: Phase 5A should replace the temporary conventional module heuristic with the planned import-root/module resolver and route settings candidates through that resolver when available.

### Module resolution roots
- Bookmark: `startup-rethink` still points to `smwxvrqu`; move it to `xuputsyz` after describing this verified slice.
- Current change: `xuputsyz`.
- Scope: added project-owned import roots and module resolution over loaded source inventories, including source-root, `src/` convention, configured `pythonpath`, and interpreter site-packages hint roots; resolved `module.py` and `package/__init__.py`; represented not-found, ambiguous, and deferred unavailable-root outcomes; and routed settings conventional-module derivation through the resolver import-root mapping.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project resolver` passed: 5 tests.
  - `cargo test -p djls-project settings_candidates` passed: 5 tests.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Ousterhout review required `NotFound` to mean authoritative absence rather than known-but-unloaded import roots; addressed by deferring unresolved modules when any relevant known root is outside the loaded source inventory.
  - Rust specialist required the same unloaded-root deferral and required settings candidates to stop using their temporary conventional-module heuristic; addressed by adding resolver-backed `module_name_for_path` and using it from settings candidates.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed source/import-root mapping, loaded-file-scoped resolution, conventional module-file/package lookup, and explicit deferred state for incomplete inventories align with mature tooling patterns.
- Follow-ups/blockers: Phase 5B should use settings candidates as input to Django Environment candidates without selecting a global settings module.

### Django Environment candidates
- Bookmark: `startup-rethink` still points to `xuputsyz`; move it to `mrznmops` after describing this verified slice.
- Current change: `mrznmops`.
- Scope: added Django Environment candidate IDs, sources, readiness outcomes, candidate issue preservation, and file-scoped environment selection by longest root prefix. Every settings candidate can become an environment candidate, multiple candidates remain normal ready state, and no global settings module is selected.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project environments` passed: 4 tests.
  - `cargo test -p djls-project multisite` passed: 1 test.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Hickey review required upstream settings-candidate issues to survive promotion to environment candidates; addressed by carrying mapped settings issues alongside ready candidates.
  - Rust specialist required file-based candidate roots to use owning project roots, stable candidate IDs, multiple project candidates to remain ready instead of globally ambiguous, and upstream issues to survive; addressed all in this slice.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed multiple project/config/environment records, per-file/root selection, stable provenance, and partial issue preservation match mature tooling patterns.
- Follow-ups/blockers: Phase 5C should observe `django_environment_candidates(db, project)` through the loading graph without holding the session lock across candidate derivation.

### Environment-discovery loading observation
- Bookmark: `startup-rethink` still points to `mrznmops`; move it to `sqppsxrp` after describing this verified slice.
- Current change: `sqppsxrp`.
- Scope: added the `environment-discovery` loading node after source files, project discovery, and Python source models; projected terminal status from `DjangoEnvironmentCandidatesOutcome`; wired CLI and LSP effect adapters through the shared loading driver; reused the LSP snapshot observation seam without holding the `Session` lock across candidate derivation; guarded progress/final outcome against supersession; and added nonblocking request plus query-reuse coverage.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project loading_environment_discovery` passed: 2 tests.
  - `cargo test -p djls-project environment_candidates_reuse` passed: 1 test.
  - `cargo test -p djls-server startup` passed: 22 tests.
  - `cargo test -p djls --test check` passed: 7 tests.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Lamport review required stale/superseded runs not to emit successful environment-discovery progress or finish as succeeded after supersession; addressed by guarding node progress and final outcome emission against the active generation.
  - Rust specialist found no must-fix issues and confirmed the node uses the neutral loading path, nonblocking snapshot seam, readiness projection, reuse coverage, and request-while-running coverage.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed snapshot/background work, short session locks, progress, cancellation/supersession guards, and query/cache reuse align with mature tooling patterns.
- Follow-ups/blockers: Phase 5D should register `workspace-ready` as a loading-plan milestone over source files, Python source models, and environment discovery.

### Workspace-ready milestone
- Bookmark: `startup-rethink` still points to `sqppsxrp`; move it to `yzympxnx` after describing this verified slice.
- Current change: `yzympxnx`.
- Scope: added `workspace-ready` milestone policy to the neutral loading plan, recorded milestone results with full vs degraded terminal status, emitted milestone observer events, guarded LSP milestone progress by startup generation, and moved semantic trait inheritance toward `djls_project::Db` while retaining legacy `ProjectDb` during migration.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project loading_plan` passed: 4 tests.
  - `cargo test -p djls-semantic --no-run` passed.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Lamport review required milestone results/events to distinguish full readiness from degraded readiness; addressed with `MilestoneTerminalStatus::{Succeeded, Degraded}` and tests for full, degraded, and non-advancing cases.
  - Rust specialist required the same degraded milestone status, LSP milestone reporting, and stronger acceptance/rejection matrix tests; addressed all in this slice.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed composite readiness, degraded health/status preservation, guarded stale progress, and best-effort service align with mature tooling patterns, with DJLS intentionally making the milestone a first-class neutral loading result.
- Follow-ups/blockers: Phase 6A should build static effective settings and installed-app projection on top of the environment candidate and module-resolution seams.

### Effective settings and installed-app projection
- Bookmark: `startup-rethink` still points to `yzympxnx`; move it to `yortktwk` after describing this verified slice.
- Current change: `yortktwk`.
- Scope: added static `effective_settings` and `installed_apps` tracked queries in `djls-project`; introduced ordered top-level Python source operations for supported settings interpretation; preserved partial list gaps and unknown causes; supported direct settings assignments, list concat, `+=`, append/extend, direct imports, and relative star imports; resolved known installed app package/AppConfig entries through static module resolution and already loaded files; and extracted AppConfig `name`, `label`, and `path` from the selected class only.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project effective_settings` passed: 6 tests.
  - `cargo test -p djls-project installed_apps` passed: 5 tests.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Hickey review required settings operations to replay in source order, unknown installed-app segments to preserve their cause, and AppConfig metadata to be class-specific; addressed all in this slice.
  - Rust specialist required source-order semantics, relative settings imports, partial list concat with known tails, and class-scoped AppConfig extraction; addressed all in this slice.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed the ordered, file-local AST-derived projection plus layered semantic interpretation, static module resolution, and deferred/partial states match mature tooling patterns.
- Follow-ups/blockers: Phase 6B should add installed-app and configured-template file loading through the source-inventory partition merge seam.

### Installed-app and template-directory file loading
- Bookmark: `startup-rethink` still points to `yortktwk`; move it to `qxtuwrlp` after describing this verified slice.
- Current change: `qxtuwrlp`.
- Scope: added installed-app and configured-template-directory file loaders in `djls-project`; derived roots from static environment/settings/app projections; introduced partitioned source-file load outcomes and patches; extended the source-inventory merge seam with first-party, configured-template-directory, and installed-app partition precedence; preserved lower-precedence resurrection across partition updates; surfaced deferred/unavailable/degraded app/template gaps as typed node outcomes; and wired the two loading nodes through the shared driver plus CLI/LSP effect adapters without registering `django-apps-ready` yet.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project installed_app_files` passed: 1 test.
  - `cargo test -p djls-project template_directory_files` passed: 1 test.
  - `cargo test -p djls-project loading_template_files` passed: 3 tests.
  - `cargo test -p djls-project loading_plan` passed: 4 tests.
  - `cargo test -p djls-server startup` passed: 22 tests.
  - `cargo test -p djls --test check` passed: 7 tests.
  - `cargo test -p djls-db source_file_set` passed: 5 tests.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Lamport review required first-party partition reloads to preserve lower-precedence partitions so resurrection remains possible; addressed by applying first-party updates through the same replace-one-partition merge invariant and adding regression coverage.
  - Rust specialist required typed deferred/unavailable/degraded outcomes instead of empty-root `Skipped`, preservation of missing/ambiguous/deferred installed-app gaps, `AppConfig.path` root selection, and no Phase 6B `django-apps-ready` API; addressed all in this slice.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed neutral VFS/workspace loading, source-root/file-set partitioning, typed roots/search paths, transactional DB changes, and explicit precedence align with mature tooling patterns.
- Follow-ups/blockers: Phase 6C should build static template directory/file/tag-library inventories over the merged source inventory and distinguish known-but-not-loaded template roots from loaded-empty roots.

### Static template inventory
- Bookmark: `startup-rethink` still points to `qxtuwrlp`; move it to `tqtwupmz` after describing this verified slice.
- Current change: `tqtwupmz`.
- Scope: added static template directory, template file, and template tag library inventories in `djls-project`; preserved unknown settings directory segments; represented loaded, deferred, unavailable, and stale template-directory states from source-inventory partition/root readiness; inventoried templates only from loaded configured or installed-app template directories; and inventoried tag libraries from Django builtins, resolved installed-app `templatetags`, and static `TEMPLATES[*].OPTIONS["libraries"]` aliases resolved through module resolution.
- Validation:
  - `just fmt --check` passed.
  - `cargo test -p djls-project template_inventory` passed: 6 tests.
  - `cargo build -q` passed.
- Review/reference follow-up:
  - Hickey review required installed-app template directories to count as loaded when covered by an app root and tag libraries to be tied to resolved installed apps rather than any `templatetags` path; addressed both.
  - Rust specialist required inventory to read partition/root readiness, preserve unavailable/stale/deferred directory semantics, and avoid global `templatetags` scans; addressed with a root-readiness projection and installed-app-root-scoped tag-library inventory.
  - Librarian found no major divergence from rust-analyzer/Ruff/ty. It confirmed the candidate-to-loaded-inventory layering, explicit source roots/file roots, static settings/search-path derivation, and tracked semantic index shape align with mature tooling patterns.
- Follow-ups/blockers: Phase 6D should migrate the first semantic template consumer and register `django-apps-ready` over the installed-app/template-directory file-loading nodes.

## Current State
- `initialize` constructs a full `Session`, which loads project config, creates `DjangoDatabase`, and bootstraps a single old `Project` input before returning capabilities (`crates/djls-server/src/server.rs:131-200`, `crates/djls-server/src/session.rs:51-75`, `crates/djls-db/src/db.rs:88-115`).
- `Project::bootstrap` chooses one optional Django Settings Module and seeds runtime-backed or refresh-backed project fields such as `TemplateDirs::Unknown`, `TemplateLibraries`, `ProjectTemplateFiles`, and `ProjectPythonIndex` (`crates/djls-semantic/src/project/input.rs:219-325`).
- `initialized` still runs the old inspector/cache path: it loads a template-library cache, queues `refresh_external_data`, and awaits the queued refresh when no cache was loaded (`crates/djls-server/src/server.rs:203-249`).
- The queued refresh does expensive work behind the shared `Session` mutex: runtime introspection, template-directory walking, Python indexing, and external extraction (`crates/djls-server/src/server.rs:40-71`, `crates/djls-semantic/src/project/sync.rs:47-85`).
- DJLS has no work-done progress implementation. It only forwards tracing logs to `window/logMessage`; `ClientInfo` records pull diagnostics and snippets but not `window.workDoneProgress` (`crates/djls-server/src/client.rs:95-122`, `crates/djls-server/src/logging.rs:31-110`).
- The repo has static discovery scaffolding, but it uses a generic `Fact<T>` model that this design intentionally does not carry forward (`crates/djls-semantic/src/project/static_model.rs:21-47`).
- Existing tests cover many lower-level pieces but not the full startup contract, request behavior during background loading, or work-done progress.

## Desired End State
- `initialize` returns after minimal runtime setup. It does not load project config from disk, bootstrap the old `Project`, run runtime introspection, scan template directories, or extract Python semantics.
- `initialized` starts background startup jobs and progress/log reporting, then returns immediately.
- Background work computes outside the shared `Session` lock and applies only short generation-checked input updates.
- An explicit `SourceFileSet` input records loaded files and roots. Project queries enumerate that input instead of walking the filesystem or reading `SourceFiles` side tables.
- A new `djls-project` crate owns static Django Discovery: project layout indexing, Python source models, module resolution, settings candidates, Django Environment candidates, effective settings, installed apps, template inventory, Python module inventory, and enrichment merge policy.
- Existing IDE features consume `djls-project` queries directly. Collection-heavy tracked queries use Salsa-friendly return shapes (`#[salsa::tracked(returns(ref))]` where appropriate, or small handle inputs) so readiness probes and request paths do not clone large vectors/maps unnecessarily. The old fat `Project` fact bag is removed from the semantic API.
- Runtime Project Introspection contributes enrichment hints only. Python/Django failure leaves static readiness intact and visible as degraded enrichment.
- `djls check` and LSP share the same project model and static loading graph. The CLI runs the graph synchronously; LSP runs it through the startup controller with generation guards and progress reporting.

## Design and Outline Carry-Forwards
- **Project config loading must not happen in `Session::new`.** `Session::new` records client/default settings only. Phase 1 does not load or store per-root project config because the root-scoped project model does not exist yet. Phase 3 loads root-scoped project config in `djls-project` loading activity code, outside the session lock, and lowers it into `ProjectDiscoverySetData`. This is preparation for discovery, not a separate readiness milestone.
- **`djls-project` cannot depend on `djls-semantic`.** Follow the outline's crate boundary. If `djls-project` needs an existing helper that currently lives under `djls-semantic::project`, move that helper to the owning crate and leave only temporary semantic re-exports until Phase 10.
- **Use pytest-lsp for real LSP startup.** The outline selected a tiny pytest/pytest-lsp e2e foundation at `tests/lsp/test_startup.py`; keep that decision. Add the Python test dependency and harness instead of replacing it with Rust integration tests.
- **Workspace roots are real inputs, not a primary-root shortcut.** Preserve all workspace folders as source roots. Discovery data is root-scoped, and file-scoped queries choose the relevant Django Environment by path. Do not silently collapse a multi-root workspace into one startup-selected Project root.
- **Root config failures are Project Facts too.** If per-root config loading fails and client/default settings are used as fallback, preserve that as typed discovery data with provenance. Do not silently substitute defaults.
- **Add minimal enrichment facts early.** Phase 3 creates `Project.enrichment` as an absent/disabled/unavailable domain-fact field on the stable Project root. Phase 9 expands the same field with runtime/deep enrichment hints and typed failures.
- **Runtime introspection is an enrichment provider, not semantic analysis.** Phase 9 keeps stable enrichment domain types, drafts, state, issues, and merge policy in `djls-project`. Inspector subprocess invocation, JSON DTOs, zipapp packaging/embedding, cache I/O, freshness policy, and provider fallback behavior remain infrastructure owned by `djls-db`/server-side provider code. Only translated `ProjectEnrichmentDraft` values and typed issues cross into `djls-project`; `djls-semantic` consumes merged facts only.
- **No generic `Fact<T>` in `djls-project`.** Keep domain objects primary and model uncertainty in domain-specific result enums or partial settings values.
- **Lean into Rust for startup invariants.** Prefer ADTs, newtypes, private fields, sealed constructors, and typed apply seams over comments plus validation. `Ready` states should contain only ready/materialized values. Ambiguity states should use `AtLeastTwo<T>` or an equivalent private-constructor type, not raw `Vec<T>`. Unknown/not-found states that drive degradation or reporting should carry a `NonEmpty<T>` issue/evidence set. Stringly inputs such as Python module names, template names, environment names, settings-module seeds, document versions, and partition precedence should be parsed into domain types at the boundary.

## What We're Not Doing
- Not eagerly validating every template at startup.
- Not eagerly building the full Model Graph.
- Not running Python, importing project code, calling `django.setup()`, or emulating app `ready()` hooks during static readiness.
- Not recursively scanning all of `site-packages`.
- Not preserving the old fat `Project` API as compatibility glue.
- Not merging ambiguous Django Environments into a fake union environment.
- Not adding progress cancellation in this implementation slice.
- Not treating caches as authoritative startup state.

## Implementation Approach
1. Run the pre-flight checks below before edits.
2. Implement phases in order. Each phase must compile and pass its targeted tests before the next phase starts, but those checks are implementation-control gates, not release criteria for a separately shipped product.
3. Prefer clean internal breaks over backwards-compatibility shims. Temporary re-exports are allowed only to keep the workspace compiling while a later phase removes the old module.
4. Keep protocol/LSP types in `djls-server`, runtime mutation in `djls-db`, workspace traversal in `djls-workspace`, neutral file identity in `djls-source`, loading readiness/partition policy and Django Discovery in `djls-project`, and template semantics in `djls-semantic`.
5. All new Salsa inputs and tracked return values containing owned data must derive `PartialEq` so Salsa can backdate unchanged results. Use `#[returns(ref)]` for owned vector/map/string fields.
6. Compare before calling Salsa setters. Setter calls always invalidate.
7. Do not hold a `Session` lock across filesystem walking, Python parsing, subprocess execution, cache I/O, or `.await` points inside background task bodies.
8. Use typed issues, result enums, and domain newtypes. Do not introduce `Error(String)`, generic reason strings, bool state fields, or wildcard matches over enums owned by DJLS.
9. Project Facts live under one stable `djls_project::Project` Salsa input. The project handle is created once during database construction and must not be swapped during reload; reloads update tracked fields through setters. Loading/progress/quiescence state is server/CLI orchestration state, not semantic Project Facts. `StartupGeneration` and `GenerationGuard` are the single authority for superseded-result rejection in the LSP executor. Starting a run must not write `Loading` or `Stale` into Salsa. Successful coherent applies update `Project` fields; failed/superseded runs leave existing facts intact and record durable diagnostics only when the diagnostic is itself a Project Fact. Stale open-document rejection is an executor outcome and must not be written as a source-file Project Fact.
10. Remove old `Project` accessors phase-by-phase as replacements land. Do not wait until Phase 10 if a phase has already migrated all consumers of a specific old field/API.

## Workspace Roots Policy
- DJLS models a workspace as multiple source roots. The LSP server and CLI capture raw roots only; they do not canonicalize, deduplicate, assign file ownership, or construct `SourceRootId` values themselves.
- `djls-project` owns root normalization and source-root construction through a small typed seam, for example `djls_project::roots::build_source_roots(raw_roots, options) -> SourceRootsPlan`. This seam normalizes roots, chooses fallback identities for missing roots, deduplicates identical canonical roots, records duplicate/missing-root facts for later project issue classification, and defines overlapping-root file ownership by longest matching prefix.
- `djls-source` owns neutral root identity types such as `SourceRootId` and `SourceRoot`; it does not attach project-loading issues or readiness to them. `djls-workspace` consumes already-normalized `SourceRoot` values and owns traversal mechanics only.
- If workspace roots overlap, retain each root for root-scoped discovery, but assign each file to one owning root by the longest matching root prefix. Identical canonical roots collapse to one `SourceRoot` with project-owned duplicate-root issue data. File-scoped environment selection uses the owning root first, then applies the normal candidate ambiguity rules.
- Static Django Discovery is root-scoped. A root may produce zero, one, or multiple Django Environment candidates, and a workspace may contain candidates from multiple roots.
- There is no global startup-selected Django Settings Module and no global startup-selected Project root. Queries such as template lookup, diagnostics, and module resolution select the relevant environment by file path.
- Settings/config loading is also root-scoped. Client settings provide defaults/overrides; per-root config refines the discovery input for that root.

## Shared Loading Graph
- Maintain one executor-neutral loading graph for the static Project Facts sequence. LSP startup and `djls check` must execute the same graph; they differ only in executor behavior and reporting.
- Do not create an empty graph in Phase 1 and do not add a temporary Phase 2 scheduler. Phase 2 is neutral primitives only. Introduce the shared graph when `djls-project` exists in Phase 3.
- Do not add a dedicated loading crate. The planned `djls-project` crate is the shared owner for project loading sequence and discovery activity.
- Task nodes are introduced by the phase that introduces their real activity service. Do not predeclare future Project Facts task IDs just to make later readiness maps compile.
- `djls-project` owns the neutral loading driver, for example `run_loading_plan(plan, effects, observer)`. The driver owns dependency order, readiness-source-derived terminal policy, abstract effect-outcome propagation, and, starting in Phase 5, milestone advancement. It must not know LSP generation IDs, cancellation policy, or why an adapter returned `Superseded`/`RejectedApply`.
- Split the neutral loading boundary into two small seams:
  - an execution/apply contract for running node activity and applying typed project-loading updates;
  - an observer/event sink for stable, presentation-free node event IDs and structured event data.
- Do not bundle activity execution, database apply, progress, logs, and terminal formatting into one god effect trait. The driver emits stable node events; LSP and CLI adapters translate those events into progress messages, logs, or terminal output separately.
- The LSP effect adapter supplies async task execution, generation guards, cancellation/superseded-result rejection, and guarded apply. Its observer supplies work-done/log progress. Its concrete executor lives in `djls-server`.
- The CLI effect adapter supplies synchronous execution and direct database application. Its observer supplies terminal diagnostics/warnings. Its concrete executor lives in the CLI crate, for example `crates/djls/src/loading.rs` or near `commands/check.rs`, not in `djls-project`. Introduce this executor in Phase 3 with the graph, not as a Phase 10 retrofit.
- Loading graph/task activity functions return domain outputs such as `PartitionedSourceFilePatch`, `ProjectDiscoverySetData`, or `ProjectEnrichmentDraft`. They must not wrap outputs in server-local `StartupUpdate`, advance milestones, emit LSP progress, or know whether the caller is LSP or CLI.
- Hide file-update choreography behind one shared apply seam at the `djls-project`/`djls-db` boundary. CLI and LSP adapters may differ in direct versus guarded invocation and reporting, but neither adapter should know the internal sequence for merging a partition patch, materializing `File` handles, updating `SourceFileSet`, and finalizing `Project.source_inventory`.
- `StartupController` owns LSP adapter volatility only: generation guards, async spawning, progress observation, and guarded application. Once Phase 3 introduces `djls-project::loading::plan`, `StartupController` must delegate node ordering and readiness-source-derived terminal policy to the neutral loading driver rather than encoding dependency policy itself. Phase 5 adds milestone policy when `workspace-ready` exists.
- Phase 3 creates `djls-project::loading::plan`, neutral loading event/outcome types, the neutral driver, the execution/apply contract, the observer/event sink, concrete LSP and CLI effect adapters, and activity modules. In Phase 3, `loading::plan` owns node IDs, dependencies, and terminal policy only; Phase 5 extends it with milestone IDs, prerequisites, and advancement. Server/CLI executors own concrete activity execution, guarded/direct database apply, progress/log emission, and terminal reporting; activity modules such as `files`, `settings`, `apps`, `templates`, and `enrichment` return typed domain outcomes. Keep sequence policy, execution behavior, application, reporting, and discovery activities separate.
- Every phase that adds a real static loading node must update the loading-node table, add executor-neutral driver/plan coverage, and wire the node through both CLI and LSP effect adapters in the same phase. Optional runtime enrichment may be skipped by a CLI policy, but the skip must still be an explicit effect-adapter outcome.
- Once `djls-project::loading::plan` exists, mirror the loading-node table in a static code-backed manifest, for example `NODE_SPECS: &[NodeSpec]`. `NodeSpec` should contain `NodeId`, prerequisites, readiness-source kind, and projection metadata used by `node_status_from_readiness`, observer tests, milestone policy, and CLI/LSP parity tests. Manifest tests must assert `NODE_SPECS` against the table-shaped intent here; later phase prose should reference this table/rule instead of restating full node semantics. This is a checked manifest, not a dynamic plugin registry: no runtime node factories, trait-object schedulers, or extension mechanism.
- Tracked queries are not loading nodes. If `effective_settings`, installed-app projection, template inventory, Python module inventory, or another tracked query becomes scheduled work later, that phase must add a row to the loading-node table and add both adapter paths.
- File-loading nodes must not create competing semantic readiness surfaces. The stable `Project` source inventory is the semantic owner of merged source files and partition metadata. Node terminal status, progress, and milestones are derived from the applied update or domain query outcome; they are not separate Project Facts.
- Source-file partition/root availability must also be query-visible through a narrow projection, not through raw partition internals. Queries such as template inventory and installed-app file projection need to distinguish `LoadedEmpty` from `KnownButNotLoaded`, `Deferred`, or `Unavailable` for the roots/directories they depend on. Absence of files is authoritative only for the partition/root represented by the current Project Facts.

### Loading-node ownership table

This table plus the canonical projection rule below is the plan's source of truth for loading-node ownership, prerequisites, readiness sources, adapter obligations, milestone effects, and status projection. Phase sections may add implementation mechanics and tests, but they must not redefine or restate node semantics as a second authority; if phase prose conflicts with this table and `node_status_from_readiness`, fix the phase prose. When a phase introduces or changes a node, update this table and the `NODE_SPECS` manifest tests first, then keep phase text to the implementation delta. A row is implemented only in the phase named by that row; earlier phases must not predeclare runtime task IDs just to satisfy future readiness maps. `effective_settings(db, project, env)`, installed-app projection, template inventory, and Python module inventory are tracked queries, not loading nodes, unless a later phase explicitly turns them into rows here.

| Node ID | Phase introduced | Graph prerequisites | Activity owner | Output type | Readiness source | CLI effect behavior | LSP effect behavior | Milestone affected |
|---|---:|---|---|---|---|---|---|---|
| `source-file-set` | 3A3 activity; 3A4 LSP adapter; stable-Project cleanup revises apply target | none | `djls-project::loading::source_files` | `PartitionedSourceFilePatch` then project-owned source-inventory update containing an incremental materialization patch; `djls-db` returns `SourceFileSetMaterialized`; project finalization returns `ProjectSourceFilesApplied` | `node_status_from_readiness(ProjectSourceFilesApplied)` using the applied first-party partition/node transition; successful apply updates `Project.source_inventory` for queries | `crates/djls` effect runs activity, applies update directly, reports terminal warnings | `djls-server` effect runs activity, applies update through `GenerationGuard`, reports progress/logs | prerequisite for `workspace-ready` once Phase 5 adds milestones |
| `project-discovery-set` | 3C, after stable-Project cleanup | `source-file-set` | `djls-project::loading::settings` | `ProjectDiscoverySetData` | applied `Project.discovery` field update and/or derived discovery domain outcome | direct database apply of discovery data | guarded database apply of discovery data | none |
| `python-source-models` | 4 | `source-file-set`, `project-discovery-set` | `djls-project::python` readiness-observation activity | typed live-query outcome; tracked queries remain source of truth | `PythonSourceIndexOutcome` from live `python_source_index(db, project)`; no `ProjectLoadingState` field | observe live query and report terminal outcome | observe live query and report progress/log outcome | prerequisite for `workspace-ready` once Phase 5 adds milestones |
| `environment-discovery` | 5 | `source-file-set`, `project-discovery-set`, `python-source-models` | `djls-project::environments` readiness-observation activity | typed live-query outcome; tracked queries remain source of truth | `DjangoEnvironmentCandidatesOutcome` / `EnvironmentSelection` outcome from live `django_environment_candidates(db, project)` / `environment_for_file(db, project, file)` queries; no `ProjectLoadingState` field | observe live query and report ambiguity/degraded outcome | observe live query and report ambiguity/degraded progress/log outcome | `workspace-ready` |
| `installed-app-files` | 6B | `source-file-set`, `project-discovery-set`, `python-source-models`, `environment-discovery` | `djls-project::apps` | `PartitionedSourceFilePatch` then project-owned source-inventory update containing an incremental materialization patch; `djls-db` returns `SourceFileSetMaterialized`; project finalization returns `ProjectSourceFilesApplied` | `node_status_from_readiness(ProjectSourceFilesApplied)` using the applied installed-app partition/node transition; successful apply updates `Project.source_inventory` | apply update directly; report unknown/deferred app gaps | guarded apply; report unknown/deferred app gaps via progress/logs | prerequisite for `django-apps-ready` once Phase 6D registers it |
| `template-directory-files` | 6B | `source-file-set`, `project-discovery-set`, `python-source-models`, `environment-discovery` | `djls-project::templates::loading` | `PartitionedSourceFilePatch` then project-owned source-inventory update containing an incremental materialization patch; `djls-db` returns `SourceFileSetMaterialized`; project finalization returns `ProjectSourceFilesApplied` | `node_status_from_readiness(ProjectSourceFilesApplied)` using the applied template-directory partition/node transition; successful apply updates `Project.source_inventory` | apply update directly; report deferred template roots | guarded apply; report deferred template roots via progress/logs | prerequisite for `django-apps-ready` once Phase 6D registers it |
| `enrichment` | 9 | static milestones as configured by runtime policy; must not block them | `djls-project::enrichment` | `ProjectEnrichmentDraft` | applied `Project.enrichment` field update and/or derived enrichment domain outcome | run or explicitly skip according to CLI policy | run optional runtime/cache work and guarded apply | no static milestone; optional enrichment view only |

### Canonical readiness projection rule

Query-visible Project Facts are the source of readiness. Loading-plan node terminal status, progress messages, and milestone advancement must be projections of the table's readiness source through `node_status_from_readiness`, not independent readiness facts. Phase sections should refer back to this rule instead of restating per-node readiness policy.

- Nodes that mutate Project Facts produce a typed domain outcome. The effect adapter applies that outcome through the relevant project/database apply intent, updating tracked fields on the stable `Project` root. Loading/running/stale orchestration states are not Project Facts.
- File-loading nodes produce a typed file update and an applied partition/node readiness transition. Applying that update also updates the `Project.source_inventory` semantic owner. Node terminal status and milestones use the applied transition or derived domain outcome; they must not maintain a second semantic readiness field.
- Query-only/readiness-observation nodes do not get shadow `ProjectLoadingState` fields. Their terminal status is derived from the typed live tracked-query outcome named in the loading-node table, such as `python_source_index(db, project)` or `django_environment_candidates(db, project)`.
- The neutral driver records node terminal status from the readiness source named in the table through one projection API, for example `node_status_from_readiness(node_id, readiness) -> NodeTerminalStatus`. It must not mark a node `Succeeded`, `Degraded`, or `Skipped` from an un-applied domain value, an unobserved query, or an adapter-owned `Superseded`/`RejectedApply` outcome.
- Observers receive the same `NodeTerminalStatus` and transition- or query-derived node event as stable IDs/data, not user-facing strings. They may format it differently for LSP and CLI, but they must not invent separate readiness semantics.
- Milestone advancement, once introduced in Phase 5, reads the readiness sources named in the loading-node table. For applied nodes, the stable `Project` facts and apply/query outcomes are the semantic source of truth. For readiness-observation/query-only nodes, the live typed query outcome wins. The plan must not maintain a parallel readiness field just for milestone advancement.
- Tracked queries that are not loading nodes expose domain availability through their result types and stable `Project` fields; the loading graph must not create shadow terminal statuses for those queries.
- `node_status_from_readiness` or its exact equivalent is the only place that maps table/`NODE_SPECS` readiness sources to `NodeTerminalStatus`. CLI reporting, LSP progress, logs, and milestones consume that projected status. Adapter-level run outcomes such as `Superseded`/`RejectedApply` are handled at the execution boundary and are not inputs to this readiness projection.

### Readiness-to-terminal-status projection table

`node_status_from_readiness` must implement this contract. Node-specific code may add typed details, but it must not choose a different terminal class for the same readiness class.

| Readiness class | `NodeTerminalStatus` | Notes |
|---|---|---|
| `Ready` / `Fresh` | `Succeeded` | Current facts are available for the node's readiness source. |
| `Ambiguous` | `Degraded` | Facts were computed, but request paths must preserve ambiguity instead of selecting a fake default. `workspace-ready` may advance only as degraded when its `NodeSpec` accepts this status. |
| `Deferred` | `Deferred` | The node has useful partial/previous context but lacks required current inputs. Dependents run only when their `NodeSpec` accepts deferred prerequisites. |
| `Skipped` | `Skipped` | The node intentionally did not run, usually because no relevant inputs exist or policy disabled optional work. Dependents run only when their `NodeSpec` marks the prerequisite optional. |
| `Unavailable` | `Unavailable` | Required facts cannot be produced from current inputs. Mandatory dependents are blocked through the prerequisite policy. |
| `Failed` | `Failed` | Unexpected execution or apply failure. Mandatory dependents are blocked and the run finishes failed unless adapter policy explicitly downgrades to degraded reporting. |
| `CachedStale` | `Degraded` | Cache/enrichment staleness is a durable domain fact, not reload-in-progress state. It may support degraded presentation when the node's policy accepts it. |
| `Superseded` / `RejectedApply` | Not a readiness input | These are executor outcomes. They become `StartupRunOutcome::Superseded` and must not be fed to `node_status_from_readiness`. |

Milestone acceptability is configured in `NODE_SPECS`. By default, milestones advance fully only from `Succeeded` prerequisites. `workspace-ready` may advance as degraded for accepted `Degraded` environment ambiguity or intentional `Skipped` Python-source work with no relevant files; it must not advance on `Deferred`, `Unavailable`, or `Failed`. `django-apps-ready` advances fully only when both app/template file nodes succeed; accepted `Deferred` app/template partitions may advance it only as degraded.

### Project fact surface coherence

Stable `Project` fields are query-visible Project Facts. Server/CLI loading state says whether a reload is in flight, failed, or superseded; it is not read by tracked semantic queries.

- A load/reload start must not erase current Project Facts.
- A successful coherent apply updates the relevant `Project` fields through setters.
- A failed or superseded reload leaves existing Project Facts intact. Durable diagnostics may be updated when they are themselves Project Facts.
- Stale open-document rejection is an executor outcome and must not be written as source/discovery/enrichment facts.
- Multi-surface queries read a coherent `Project` revision. They must not assemble Project Facts from separate current/previous readiness bags.
- Add mixed-surface tests after the stable Project cleanup: diagnostics, completions, navigation, references, and hover must derive degraded/deferred outcomes from Project domain facts and server/session loading state at the request boundary, not from raw loading enums in tracked queries.

### Readiness observation policy

A loading node may advance terminal status or milestones only from the readiness source named in the loading-node table as observed on the live project database after any required apply. Clone-only work is allowed only as report-only validation or speculative measurement. A cloned read-only database must not be the evidence for `Ready`, milestone advancement, or user-facing "complete" progress unless the same result has also been observed through the live readiness projection. Live observation must use a nonblocking database access seam; it must not hold `Arc<Mutex<Session>>` across Python parsing, filesystem traversal, or other long tracked-query work. If a phase intends startup to avoid first-request recomputation, its node must run the tracked query on the live database and include a test or counter proving the first request after the node reports ready does not recompute the same expensive facts.

### Prerequisite terminal policy

`NODE_SPECS` must encode how each prerequisite terminal status affects each dependent node. No successor may remain `Pending` because a prerequisite ended in a non-ready state. Use this default policy unless a node spec names a narrower exception:

| Prerequisite terminal status | Dependent-node transition |
|---|---|
| `Ready` / `Succeeded` | Run the dependent node. |
| `Degraded` | Run only if the dependent `NodeSpec` declares that degraded prerequisite acceptable; otherwise mark the dependent `Skipped { blocked_by }` with a typed prerequisite issue. |
| `Deferred` | Run only if the dependent can produce useful partial facts from deferred input and declares that in `NodeSpec`; otherwise mark the dependent `Skipped { blocked_by }` or `Deferred { blocked_by }` according to the dependent's own readiness type. |
| `Skipped` | Run only when the prerequisite is declared optional for that dependent; otherwise mark the dependent `Skipped { blocked_by }`. |
| `Unavailable` | Do not run mandatory dependents. Mark them `Skipped { blocked_by }` or `Unavailable { blocked_by }` according to the dependent's own readiness type. |
| `Failed` | Do not run mandatory dependents. Mark not-yet-started dependents blocked and finish the run as failed unless an explicit adapter policy downgrades that failure to degraded reporting. |
| `Superseded` / `RejectedApply` | Stop the current run, mark not-yet-started work superseded, and finish progress exactly once as `StartupRunOutcome::Superseded`. Do not start successors. |

Milestone policy reads the same prerequisite mapping. A milestone may advance in a degraded state only when its spec explicitly accepts the prerequisite's projected terminal status; aggregate readiness fields cannot override this table.

### Executor transition policy

LSP executor outcomes are not readiness inputs. They gate whether an observed or applied readiness value is allowed to affect node events, progress, milestones, or Project Facts.

| Transition | Required checks | Stale/current-generation failure outcome | Notes |
|---|---|---|---|
| Guarded run start | Check generation before recording server-side run-start state. Do not write `Loading`/`Stale` Project Facts. | `StartupRunOutcome::Superseded` | No activity starts after a superseded generation. |
| Guarded apply | Check generation before applying; validate captured open-document versions before writing facts derived from captured text. | `ApplyOutcome::Superseded` for generation mismatch; `ApplyOutcome::Rejected { reason: ApplyRejection::StaleDocument { file, path, captured, current } }` for stale text. | `Superseded` is only for generation supersession. Stale document text is its own rejection reason with evidence. |
| Guarded live-query observation | Check generation before live observation, after live observation, and before emitting node events or advancing milestones. | `ObservationOutcome::Superseded` | Query-only nodes such as `python-source-models` and `environment-discovery` must not report gen1 success from gen2 facts. |
| Progress or milestone emission | Check the run is still current immediately before reporting. | Finish the old run once as `StartupRunOutcome::Superseded`; do not emit current-success events. | Reporting is also guarded, not just database mutation. |
| Stale-document rejection | Do not apply facts derived from older open-buffer text. | Restart the affected node once from a fresh snapshot; if it repeats, return an executor/node outcome with typed stale-document evidence and finish the run coherently. | Do not write stale-document rejection as a Project Fact, and do not conflate it with generation supersession. |

### Open-buffer event policy

Background loading must not regress Project Facts or diagnostics to older open-buffer text. `ProjectLoadingSnapshot` captures opened documents with stable document identity and version/epoch data, not live buffer handles. `didOpen`, `didChange`, and `didClose` update the live `File::source(db)` state and advance an open-document epoch before request handling observes the file. Loading activities should avoid carrying source text across async boundaries; when they must use captured source text, guarded apply/reporting must verify that the captured document versions still match the live open-document table. A stale document snapshot produces `ApplyOutcome::Rejected { reason: ApplyRejection::StaleDocument { file, path, captured, current } }` and follows the stale-document rejection policy above instead of applying facts from older text.

## Temporary Bridge Deletion Gates
- Do not add a temporary Phase 2 startup scheduler or server-owned Django file policy.
- Any temporary semantic re-export created while moving helpers/types to `djls-project` must name Phase 10 as its deletion gate.
- The temporary Phase 1 no-project semantic adapter must move to `djls-project::availability` or be deleted/narrowed to a semantic-specific adapter in Phase 3C4. Do not grow it into the durable shared availability API before `djls-project` exists.
- Any remaining `Queue` use must be removed or explicitly bounded before Phase 3 completes; using `Queue` for startup or Django Discovery after Phase 3 is a bug.
- Remove old `Project` accessors phase-by-phase as their replacement query lands; each phase that migrates all consumers of an old field must include a cleanup search for that field.
- Any temporary adapter from old inspector/cache code to enrichment must be deleted in Phase 9 when the project-owned enrichment provider lands.
- If repeated cleanup `rg` checks start drifting or being skipped, add a runnable `just` target or checked script for the startup-rethink cleanup searches instead of relying on prose-only checkboxes.

## Pre-flight

### jj change discipline
1. Use `jj` for implementation workflow commands, not raw `git`. In a colocated repo, Git's detached `HEAD` is normal; `jj st` is the source of truth.
2. Use one new work-item bookmark for the whole rewrite, `startup-rethink`. Do not create one bookmark per slice. The bookmark is the review/push name and a stable way to find the work; the described `jj` changes are the slice history.
3. Keep the planning docs in their own initial described `jj` change and put the `startup-rethink` bookmark there first. This preserves the plan/research baseline as the first change in the stack.
4. Start implementation in a fresh child change on top of `startup-rethink`, for example `jj new startup-rethink -m "startup-rethink implementation"`.
5. Record the bookmark and current change ID in the Implementation Status section above.
6. Make sure the current `jj` change is clean before the first code change. `jj` has no staging area, so this means `jj st` shows no unexpected file changes in `@`.
7. Run the baseline checks before the first code change.
8. Complete and verify one implementation slice at a time.
9. After each slice passes its targeted checks, update the Implementation Status / Implementation Notes section for that slice.
10. At the end of each verified slice, describe the completed change with `jj describe -m "<descriptive message>"`, then move `startup-rethink` to that verified slice with `jj bookmark set startup-rethink -r '@'`.
11. Start the next slice with `jj new` and no message. Leave the new empty working change undescribed until its work is complete and verified.
12. Use a normal descriptive change message for the actual completed change. Do not mention "plan", "phase", or slice numbers in the message unless the domain change itself needs that wording.
13. Before pushing for review, make sure the bookmark points at the latest completed slice: `jj bookmark set startup-rethink -r '@'`.

### Commands
- [x] Describe the planning-docs change: `sqoqvvrn` (`docs: add startup rethink planning docs`)
- [x] Create the work-item bookmark: `startup-rethink` points to `sqoqvvrn`
- [x] Start the implementation change on top of the planning baseline: `otrxksps` (`startup-rethink implementation`)
- [x] Confirm the implementation change is clean before implementation: `jj st` shows no unexpected file changes in `@`
- [x] Capture baseline: `cargo test -q` — passed before edits (workspace tests all green).

### Halt conditions
- If baseline tests fail, halt and report the failing tests before starting the rewrite.

## Phase 1: protocol-ready without Project bootstrap

### Overview
Make the first slice boring: `initialize` constructs a minimal session/database and returns capabilities without project config, old `Project` bootstrap, cache loading, runtime introspection, or filesystem discovery. `initialized` must not await or start any Project Facts loading yet; it should log and return. This phase proves no-project request paths degrade instead of panicking.

Do not add the startup controller, generation guards, progress task APIs, loading executors, source-file loading, or root-scoped config loading in Phase 1. Those require real loading work and land in Phase 3 with the shared executor boundary.

### Changes Required

#### 1. Stop bootstrapping `Project` in database construction
**File**: `crates/djls-db/src/db.rs`

**Edits**:
- Change `DjangoDatabase::new(file_system, settings, project_path)` to `DjangoDatabase::new(file_system, settings)`.
- Remove the `if let Some(path) { db.set_project(...) }` branch from construction.
- Keep `project: Arc<Mutex<Option<Project>>>` temporarily, but it should remain `None` after construction in this phase.
- Keep `set_project` only if tests or old code still need it during the transition; make it `pub(crate)` or remove it once no call sites remain.

**Call sites to update**:
- `crates/djls-server/src/session.rs`
- `crates/djls/src/commands/check.rs` file-system path
- `crates/djls/src/commands/check.rs` stdin path

#### 2. Make `Session::new` minimal
**File**: `crates/djls-server/src/session.rs`

**Edits**:
- Add `workspace_roots: Vec<Utf8PathBuf>` to `Session`.
- Resolve roots from `InitializeParams.workspace_folders`, then `root_uri`, then current directory. Preserve all workspace folders, not only the first.
- Parse client options and build `ClientInfo` as today.
- Use `client_options.settings.clone()` as the initial settings. Do not call `Settings::new` here.
- Create `DjangoDatabase::new(workspace.overlay(), &client_settings)` with no project path.
- Add `Session::workspace_roots(&self) -> &[Utf8PathBuf]`.
- Do not add `Session::startup_context_seed`, startup snapshots, generation state, or any equivalent loading helper in this phase. `Session` should only expose narrow accessors such as `workspace_roots()` and `client_info()`.
- Update `Session::default()` tests: the default session should have `project().is_none()`.

**Tests to add/update**:
- `session_new_does_not_bootstrap_project`
- `session_new_preserves_all_workspace_folders`
- `session_new_uses_client_settings_without_project_config_load`

#### 3. Make `initialized` fire and return
**File**: `crates/djls-server/src/server.rs`

**Edits**:
- Remove imports of `load_template_library_cache` and `refresh_external_data` from server startup.
- In `initialize`, construct/store the minimal session and return capabilities. Do not send requests/notifications before the initialize response.
- In `initialized`, log receipt and return immediately. Do not load root settings/config, do not create a `StartupController`, do not await startup work, do not call `load_template_library_cache`, and do not call `refresh_external_data`.
- Leave the existing `Queue` in place only for legacy non-startup ordered session mutation. Do not add any new startup or background discovery work to it.

#### 4. Update old callers to degrade with no project
**Files**:
- `crates/djls-db/src/db.rs`
- `crates/djls/src/commands/check.rs`
- `crates/djls/src/commands/common.rs`

**Edits**:
- Ensure `SemanticDb` methods still return builtin/default/empty values when `project()` is `None`.
- `djls check` with explicit paths should continue to validate parser/builtin semantics.
- `discover_files` can continue to fall back to walking the project root when `db.template_dirs()` is `None`.

#### 5. Define request behavior while Project Facts are absent
**Files likely affected**:
- `crates/djls-server/src/server.rs`
- `crates/djls-ide/src/diagnostics.rs`
- `crates/djls-ide/src/completions.rs`
- `crates/djls-ide/src/navigation.rs`
- `crates/djls-semantic/src/availability.rs` as the temporary Phase 1 home for the first availability state
- `crates/djls-semantic/src/resolution.rs`

**Edits**:
- Document and test the degraded request contract before removing Project bootstrap. Keep Phase 1 pinned to shared outcomes, not feature-local readiness policy:
  - diagnostics should return parser/builtin diagnostics or a deferred/no-project diagnostic result; they must not panic or depend on old `Project` fields.
  - completions should return builtin/static-only completions or an empty result with a trace log; they must not panic or depend on old `Project` fields.
  - navigation/references/hover should return no target/deferred results; they must not panic or depend on old `Project` fields.
- Add only a minimal temporary no-project semantic adapter for Phase 1 request degradation. Because `djls-project` does not exist yet, this may be a narrow `ProjectFactsAvailability::Absent { reason }`-style semantic type or equivalent helper, but it must not try to design the durable shared availability API. Phase 3C creates the project-owned `djls-project::availability` seam and deletes or narrows this temporary adapter.
- Add a Phase 3C move/delete note beside the temporary Phase 1 type. Phase 3C must move pure readiness classification to `djls-project::availability` and either delete the temporary semantic type/module or leave only a clearly named semantic-specific adapter.
- Add unit or server-level tests for an opened template while `project()` is `None`.
- Do not add feature-specific readiness branching in this phase that Phase 3C's shared projection must rediscover. `djls-ide` must consume the shared availability result and translate it to presentation behavior; it must not own pure Project Facts availability or branch directly on raw readiness enums.

#### 6. Add minimal real-LSP startup smoke tests
**Files**:
- `pyproject.toml`
- `uv.lock`
- `tests/lsp/test_startup.py`

**Edits**:
- Add `pytest` and `pytest-lsp` to the Python dev dependency group and update `uv.lock`.
- Use pytest-lsp to spawn `djls serve` over stdio. Define a stable fixture that launches the workspace binary through the same command everywhere, for example `cargo run -q -p djls -- serve --connection-type stdio` until a packaged `djls` binary fixture exists. Do not hand-roll a Rust LSP harness for this contract.
- Keep this Phase 1 slice black-box and protocol-only:
  - `initialize_returns_capabilities`
  - `server_stays_responsive_after_initialized`
- Do not assert work-done progress in Phase 1. Progress token creation and begin/end semantics land with the loading executor in Phase 3.
- Do not use pytest-lsp timing assertions to prove blocked startup work is nonblocking. Blocked loading work is introduced and tested through injected executor seams in Phase 3.

### Success Criteria

#### Automated Verification
- [x] Session startup tests pass: `cargo test -p djls-server session::tests::session_new` — 3 passed.
- [x] Minimal real-LSP startup smoke tests pass: `uv run pytest tests/lsp/test_startup.py -k "initialize_returns_capabilities or server_stays_responsive_after_initialized"` — 2 passed.
- [x] No-project degraded request tests pass through the temporary no-project semantic adapter and record the Phase 3C move/delete gate to `djls-project::availability`: `cargo test -p djls-server degraded_no_project` — 1 passed; gate documented in `crates/djls-semantic/src/availability.rs`.
- [x] Executable startup assertion proves Phase 1 `initialized` does not call or schedule `load_template_library_cache`, `refresh_external_data`, root config loading, startup controller work, or Queue startup work; if a direct assertion would be more intrusive than the behavior change, keep the manual `rg` check as a temporary Phase 1 gate and replace it when the startup controller seam lands. Evidence: `initialized` only logs/returns; `rg "load_template_library_cache|refresh_external_data|StartupController|root settings|Settings::new" crates/djls-server -g '*.rs'` finds only non-startup configuration-refresh uses.
- [x] Server and CLI compile with the new database constructor: `cargo test -p djls-server` — 32 passed; doc test ignored as before.
- [x] Existing CLI tests still pass with explicit legacy Project bootstrap for `djls check` while LSP startup remains no-project: `cargo test -p djls --test check` — 7 passed, including custom project tagspec coverage.
- [x] Workspace builds: `cargo build -q` — passed.

#### Manual Verification
- [x] Inspect `crates/djls-server/src/server.rs` and confirm `initialized` only logs/returns and no longer calls `load_template_library_cache`, `refresh_external_data`, root settings/config loading, startup controller code, or any awaited startup work. Evidence: inspected `initialized`; body is only `tracing::info!(...)`.
- [x] Run `rg "load_template_library_cache|refresh_external_data|StartupController|root settings|Settings::new" crates/djls-server -g '*.rs'` and confirm any matches are not in `initialize`, `initialized`, `Session::new`, or Phase 1 startup paths. Evidence: matches are only the `refresh_external_data` import/call and `Settings::new` in `did_change_configuration`.
- [x] Inspect `tests/lsp/test_startup.py` and confirm the Phase 1 smoke tests use the stable `djls serve --connection-type stdio` launch fixture and cover only black-box protocol responsiveness, not progress or background-loading timing. Evidence: `SERVER_COMMAND` is `cargo run -q -p djls -- serve --connection-type stdio`; tests assert capabilities and an awaited completion request only.

## Phase 2: neutral source and workspace loading primitives

### Overview
Add the neutral types and traversal helper needed by the later loading graph. This phase does not schedule startup work, does not add readiness/loading state, does not add Django file-selection policy, and does not teach `djls-db` partition precedence. It is a small primitives PR.

### Changes Required

#### 1. Add neutral source file-set types
**File**: `crates/djls-source/src/file_set.rs`

**Edits**:
- Add `SourceFileSet`, `SourceFileSetData`, `SourceRootId`, `SourceRoot`, `SourceRootEntry`, `DiscoveredSourceFile`, `LoadedSourceFile`, and `FileSetSummary`.
- Use existing `FileKind`, `File`, and `FileRootKind`.
- `SourceFileSet` is a Salsa input whose data stores the final handle-bearing source roots/files for the current project-loading view after `djls-db` materializes `File` handles. It does not store partition snapshots, precedence, conflict policy, or readiness state.
- Do not add `SourceFileSetAvailability`, `FileSetPartition`, `FileSetPatch`, source loading issues, or generation IDs to `djls-source`. Those are project-loading concerns introduced in Phase 3.
- `SourceRootId` is the canonical root identity for materialization and deletion. It wraps the normalized canonical root path selected by the Phase 3 `djls-project` root-construction seam. If canonicalization fails because a root is missing, the root-construction seam uses the normalized configured path as the fallback identity and records root facts for Phase 3 project-owned issue classification. `djls-source` does not attach typed missing-root issues. For overlapping roots, each retained canonical root has its own `SourceRootId`; file ownership still uses longest-prefix matching.
- `DiscoveredSourceFile` must not carry a `File` handle. It records the owning `SourceRootId`; `DjangoDatabase` mints/preserves `File` handles when Phase 3 applies the project-owned source-file materialization patch into `SourceFileSetData`.
- Keep fields private unless a caller needs direct construction. Provide accessors or iterators as needed.
- Derive `Clone`, `Debug`, `PartialEq`, and `Eq` for non-Salsa structs.

**Code shape**:
```rust
#[salsa::input]
#[derive(Debug)]
pub struct SourceFileSet {
    #[returns(ref)]
    data: SourceFileSetData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFileSetData {
    roots: Vec<SourceRootEntry>,
    files: Vec<LoadedSourceFile>,
    summary: FileSetSummary,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourceRootId(Utf8PathBuf);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRoot {
    id: SourceRootId,
    path: Utf8PathBuf,
    kind: FileRootKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRootEntry {
    root: SourceRoot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredSourceFile {
    path: Utf8PathBuf,
    root: SourceRootId,
    kind: FileKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedSourceFile {
    path: Utf8PathBuf,
    root: SourceRootId,
    kind: FileKind,
    file: File,
}
```

#### 2. Export file-set APIs
**Files**:
- `crates/djls-source/src/lib.rs`
- `crates/djls-source/src/db.rs`

**Edits**:
- Add `mod file_set;` and public re-exports for the new neutral types.
- Add `fn source_file_set(&self) -> Option<SourceFileSet>` as a low-level storage accessor on `djls_source::Db` if needed by tests or later project queries. This `Option` is not startup readiness; project-facing readiness is exposed by `djls-project::loading` in Phase 3.
- Add helper accessors to filter local/project entries if needed by later phases, but do not add Django concepts to `djls-source`.

#### 3. Add neutral file loading in the workspace crate
**File**: `crates/djls-workspace/src/file_loader.rs`

**Edits**:
- Create a workspace-owned neutral loader module and export it from `crates/djls-workspace/src/lib.rs`.
- Define `FilesForRootsRequest { roots: Vec<SourceRoot>, predicate: FileLoadPredicate, options: WalkOptions }` or an equivalent closure-based API.
- Define `FilesForRootsResult { roots: Vec<SourceRoot>, files: Vec<DiscoveredSourceFile>, summary: FileSetSummary, issues: Vec<WorkspaceWalkIssue> }`; each `DiscoveredSourceFile` contains exactly `{ path: Utf8PathBuf, root: SourceRootId, kind: FileKind }` and no `File` handle. `WorkspaceWalkIssue` is neutral traversal evidence only, such as missing root, walk error, or unreadable path; it carries no Django meaning, readiness status, or project-loading policy.
- Implement `load_files_for_roots(request)` by delegating walking to the existing `walk_files` helper rather than creating a second filesystem walker.
- Convert each returned `Utf8PathBuf` into `DiscoveredSourceFile { path, root, kind }` using the caller-provided root metadata.
- Keep `djls-workspace` neutral: it owns traversal mechanics, ignore/hidden handling, path normalization, sorting, and deduplication only. It must not hard-code Python/template/config selection, high-cost Django discovery excludes, installed-app predicates, or template-directory policy.
- `FileSetSummary` should report included counts only unless `walk_files` is extended to return real traversal statistics. Do not invent excluded/skipped counts from a walker that dropped that information. If Phase 2 extends `walk_files`, return typed neutral traversal issues instead of swallowing walker errors with `filter_map(Result::ok)`; Phase 3 maps those issues into `ProjectSourceFilesIssue` and readiness effects.
- Reuse `walk_files` path normalization, sorting, and deduplication; do not duplicate those rules in `file_loader.rs`.
- Preserve `SourceRootId` through loaded roots and files so later materialization patches can remove roots by stable canonical identity.

**Tests to add**:
- Delegates to `walk_files` and preserves sorting/deduplication.
- Excludes `.gitignore`-ignored files when `WalkOptions::default()` does so.
- Excludes hidden files by default.
- Applies a caller-provided predicate without knowing what the predicate means.
- Produces included counts only unless traversal stats are implemented.
- Handles missing roots without panicking and returns neutral traversal/root evidence without attaching project-loading readiness.
- Preserves root identity for duplicate and overlapping roots; root removal can be expressed with `SourceRootId`.

### Success Criteria

#### Automated Verification
- [x] Source file-set unit tests pass: `cargo test -p djls-source file_set` — 6 passed.
- [x] Workspace neutral file-loader tests pass: `cargo test -p djls-workspace file_loader` — 7 passed.
- [x] Workspace builds: `cargo build -q` — passed.

#### Manual Verification
- [x] Confirm `djls-source` contains no `SourceFileSetAvailability`, `FileSetPatch`, `FileSetPartition`, loading generation, or Django partition policy. Evidence: inspected `crates/djls-source/src/file_set.rs`; it contains only neutral source roots, discovered/loaded files, handle-bearing `SourceFileSetData`, and included-file summary.
- [x] Confirm any `djls_source::Db::source_file_set()` accessor is documented as low-level storage, not startup readiness. Evidence: no `djls_source::Db::source_file_set()` accessor was added in Phase 2.
- [x] Confirm `load_files_for_roots` does not require a `Session` lock and delegates filesystem walking to `walk_files`. Evidence: inspected `crates/djls-workspace/src/file_loader.rs`; `load_files_for_roots` takes a `FilesForRootsRequest`, uses no session/lock types, and calls `walk_files` for traversal.
- [x] Confirm `djls-workspace` contains no Python/template/config/installed-app predicate logic. Evidence: inspected `crates/djls-workspace/src/file_loader.rs`; predicates are caller-provided closures and the module contains no Django policy beyond test file extensions used to prove caller filtering.

## Phase 3: `djls-project` crate, shared loading executor, and project layout tracer

### Overview
Create the `djls-project` crate, make it the first real loading-graph boundary, and introduce both LSP and CLI executor shapes with the first source-file node, then the root discovery node once discovery data exists. Add a project layout index over the loaded `SourceFileSet`. This establishes the new Django Discovery crate boundary without yet performing Python AST extraction or Django role classification.

**Revision status**: Phase 3A1-3A4c below records completed slices, including the now-obsolete `ProjectLoadingState` implementation. Treat those references as implementation history until the Architecture correction gate rewrites them. Do not follow any future-facing `ProjectLoadingState` instruction in Phase 3A4d/3B/3C/3D or later phases; those sections are blocked until rewritten against the stable `djls_project::Project` root.

Implement Phase 3 as hard-stop subphases with separate compile/test gates:
1. **Phase 3A1: project crate and helper move** — completed; keep.
2. **Phase 3A2a: loading-state shell** — completed as scaffolding, but the `ProjectLoadingState` semantic-root direction is superseded by the Architecture correction gate.
3. **Phase 3A2b: source partition merge seam** — completed and retained, but future source facts apply into stable `Project.source_inventory`.
4. **Phase 3A2c: database materialization/finalization and invalidation** — completed and retained, but finalization target changes from `ProjectLoadingState.source_files` to stable `Project.source_inventory`.
5. **Phase 3A3: source-file node through CLI** — completed and retained as orchestration.
6. **Phase 3A4a: LSP generation guard and guarded apply** — completed and retained as server-local orchestration.
7. **Phase 3A4b: LSP source-file executor** — completed and retained, but stale-document rejection must stop writing Project Facts.
8. **Phase 3A4c: progress lifecycle** — completed and retained.
9. **Architecture correction: stable Project root** — current required cleanup before any new Phase 3A4d/3B feature work.
10. **Phase 3A4d: configuration restart** — route `didChangeConfiguration` through the same active loading graph without writing run-start `Loading`/`Stale` Project Facts.
11. **Phase 3B: discovery type scaffolding** — add project-owned discovery/enrichment domain facts under stable `Project` without wiring root discovery activity yet.
12. **Phase 3C: root discovery data** — run four gates: structured `djls-conf` root settings load, discovery data/Project apply, `project-discovery-set` through CLI/LSP plus config restart, and availability/request degraded matrix.
13. **Phase 3D: layout, concrete provenance, and cleanup** — add layout indexing over stable source inventory, add or defer provenance based on concrete use, expand degraded request tests, and remove or bound the legacy queue.

Do not begin the next subphase until the current subphase's targeted checks pass.

Treat Phase 3 as separate execution PR slices, not one workbench batch. These slices are the real implementation units; the Phase 3 label is not a release boundary. This plan is the map; before implementation, create or track per-slice work items for:
- **3-loading-shell**: completed scaffolding; its `ProjectLoadingState` design is superseded and must be removed/quarantined by the architecture correction.
- **3-source-merge**: completed and retained source partition merge seam; future applies target stable source inventory.
- **3-db-apply**: completed and retained materialization invariants; future finalization target changes to stable source inventory.
- **3-driver-cli**: completed and retained CLI orchestration for `source-file-set`.
- **3-lsp-guard**: completed and retained generation guard/orchestration.
- **3-lsp-source**: completed and retained LSP source-file executor, with stale-document fact writes removed by the architecture correction.
- **3-lsp-progress**: completed and retained work-done progress lifecycle.
- **3-stable-project-root**: current cleanup slice replacing `Db::project_loading_state()` with stable `Db::project()` and moving source facts under `Project.source_inventory`.
- **3-config-restart**: future configuration restart through the active graph and stable Project apply semantics.
- **3-discovery**: Phase 3B plus Phase 3C1-3C4 discovery facts, structured config loading, root-scoped discovery data, config-change discovery restart, availability ownership move, and project/semantic availability degraded matrix.
- **3-layout-cleanup**: Phase 3D layout indexing over source inventory, concrete-or-deferred provenance, queue cleanup, and dependency wiring.

### Changes Required

#### Phase 3A1: Project crate and helper move
**Files**:
- `Cargo.toml`
- `crates/djls-project/Cargo.toml`
- `crates/djls-project/src/lib.rs`
- `crates/djls-project/src/interpreter.rs`
- `crates/djls-project/src/system.rs`
- `crates/djls-project/src/env.rs`
- `crates/djls-semantic/src/project/python.rs`
- `crates/djls-semantic/src/project/system.rs`
- `crates/djls-semantic/src/project/input.rs`
- `crates/djls-semantic/src/lib.rs`

**Edits**:
- Depends on **Phase 2 neutral source/workspace primitives**.
- Add `djls-project = { path = "crates/djls-project" }` to root `[workspace.dependencies]` in the internal dependency group, alphabetized with the other `djls-*` crates.
- Create `crates/djls-project/Cargo.toml` with version `0.0.0`, edition `2021`, workspace dependencies, and `[lints] workspace = true`.
- Keep `djls-project` façade modules private by default. Expose specific root exports such as `djls_project::Interpreter` and `djls_project::load_env_file`; do not make helper modules public unless a later phase introduces a real module-level API.
- Initial dependencies should include `djls-conf`, `djls-source`, `djls-workspace` only if needed by public contracts, plus `camino`, `dotenvy`, `rustc-hash`, `salsa`, `serde`, `thiserror`, `tracing`, and `which` as needed by the moved interpreter/env helpers and project inputs.
- Do not depend on `djls-server`, `djls-db`, `djls-ide`, or `djls-semantic`.
- Move `Interpreter` and testable system helpers from `djls-semantic::project` into `djls-project`.
- Move `load_env_file` into `djls-project::env`.
- Update old semantic code to import/re-export these moved items temporarily:
  - `crates/djls-semantic/src/project/python.rs` can become a small re-export module if no other Python-project logic remains there.
  - `crates/djls-semantic/src/lib.rs` should continue re-exporting `Interpreter` and `load_env_file` until Phase 10 removes old semantic project APIs.
- Add a local cleanup gate: run `rg "djls_semantic::project::(python|system)|load_env_file|Interpreter" crates -g '*.rs'` and keep only intentional temporary re-exports/callers needed for later phases.

**Tests and stop gate**:
- Preserve current interpreter tests in the new crate.
- Stop after `cargo test -p djls-project`, `cargo test -p djls-project interpreter`, and `cargo test -p djls-project env` pass.
- Stop after the helper-move cleanup search proves remaining semantic helper references are intentional temporary re-exports/callers: `rg "djls_semantic::project::(python|system)|load_env_file|Interpreter" crates -g '*.rs'`.

#### Phase 3A2a-3A2c: Source loading state, merge seam, and DB materialization

**Superseded note**: This section records completed implementation history. Keep the source/workspace primitives, partition merge seam, materialization invariants, and tests. The `ProjectLoadingState` semantic-root instructions in this section are superseded by the Architecture correction gate and must not guide new work.

**Files**:
- `crates/djls-project/src/db.rs`
- `crates/djls-project/src/input.rs`
- `crates/djls-project/src/roots.rs`
- `crates/djls-project/src/loading/state.rs`
- `crates/djls-project/src/loading/files.rs`
- `crates/djls-project/src/loading/source_files.rs`
- `crates/djls-db/src/db.rs`
- `crates/djls-bench/src/db.rs`
- `crates/djls-semantic/src/testing.rs`

**Edits**:
- Depends on **3A1 project crate/helper move** and **Phase 2 neutral source/workspace primitives**.
- Implement this section as three hard gates:
  - **3A2a loading-state shell**: `Db`, `ProjectLoadingState`, source-file availability enum, minimal concrete discovery/enrichment placeholder availability types, fixture DB impls, and reset intent shape only.
  - **3A2b first-party source apply seam**: project-owned root construction, first-party file policy, `SourceFilesLoadRequest`, a first-party partition snapshot, per-partition readiness, incremental materialization patch data, and opaque update constructors. Defer multi-partition conflict, precedence, and lower-precedence resurrection behavior that cannot be tested with a real second partition until Phase 6B.
  - **3A2c database materialization/finalize**: policy-free `djls-db` materialization, changed-path `File` handle preservation, project-owned finalization into `ProjectSourceFilesAvailability::Ready(ReadyProjectSourceFiles)`, `ProjectSourceFilesApplyResult`/`ProjectSourceFilesApplied` return, terminal failure/deferred availability transitions, and invalidation tests only.
- Define `#[salsa::db] pub trait Db: djls_source::Db` and add `fn project_loading_state(&self) -> ProjectLoadingState` as the single Salsa-visible readiness handle for project loading.
- Define `#[salsa::input] ProjectLoadingState` with `source_files`, `discovery`, and `enrichment` fields. In 3A2, source-files must be real and the discovery/enrichment fields must use concrete minimal placeholder types, not untyped placeholders: `ProjectDiscoveryAvailability::Unavailable { issue }` and `ProjectEnrichmentState::NotStarted` / `Unavailable { issue }`. Phase 3B expands discovery to the full enum, including `Loading`, `Ready(ProjectDiscoverySet)`, and `Stale { previous }`, once `ProjectDiscoverySet` exists. Do not add an independent project loading generation.
- Add convenience accessors such as `project_source_files(db)`, `project_discovery(db)`, and `project_enrichment(db)` only if they read fields from `ProjectLoadingState`. Do not implement tracked queries by reading mutex-backed availability values.
- Add `djls_project::roots::build_source_roots(raw_roots, options) -> SourceRootsPlan` or an equivalent small typed builder. It is the only production path that canonicalizes raw LSP/CLI roots into `SourceRoot`/`SourceRootId` values, deduplicates roots, assigns overlapping-file ownership policy, and records duplicate/missing-root facts for project-owned issue classification.
- Add the first-party discovery predicate and high-cost exclude policy in `djls-project`, not in `djls-server` or `djls-workspace`.
- Define a node-specific `SourceFilesLoadRequest` for first-party project file loading. It should contain source roots and the neutral workspace-loader options/predicate inputs needed by `djls-project`; it must not contain `ProjectLoadingSnapshot`, `Arc<Mutex<Session>>`, LSP types, progress handles, or generation guards.
- Expose a small `first_party_discovery_files_request(roots) -> FilesForRootsRequest` helper or equivalent typed request builder that lowers `SourceFilesLoadRequest` into the neutral `djls-workspace` loader request.
- Define `ProjectSourceFilesAvailability` in `djls-project::loading`, not in `djls-source`. It distinguishes `Loading`, `Ready(ReadyProjectSourceFiles)`, terminal `Deferred`/`Unavailable`/`Failed` states with typed issues and optional previous ready files, and `Stale { previous }` without carrying an independent generation. `Ready` and `Stale.previous` must carry only materialized ready source files; discovered-only state is internal to the merge/materialization seam and must not be query-visible as ready.
- Define one durable query-visible project source-file state object, `ReadyProjectSourceFiles`. It privately owns the current partition state plus the materialized merged `SourceFileSet`, and its public `merged()` accessor returns `SourceFileSet` directly, not `Option<SourceFileSet>`. Implementation note: keep the discovered/materialized ADT internal to merge/update/finalization if useful, but do not expose a discovered-only variant through `ProjectSourceFilesAvailability::Ready`. The partition state remains the canonical discovered/source-ownership representation, and handle-free merged discovered data stays an ephemeral derived value for patch comparison.
- Define internal partition state such as `ProjectFileSetPartitions` inside those project source files. In 3A2 it needs to store the first-party partition snapshot plus its domain readiness and enough structure for later patch merges and file-loading node/partition terminal projection. Multi-partition precedence decisions, conflict checks, and lower-precedence resurrection are introduced with their first real second partition in Phase 6B.
- Keep partition snapshots, partition readiness/status values, full merged snapshot data, incremental materialization patch data, and `ProjectSourceFilesUpdate` constructor-controlled with private fields. Treat these as internal implementation shapes for the merge/apply seam, not public consumer APIs. Tests may use fixture builders, but production code must obtain updates through the project-owned merge seam rather than assembling partition state, merged data, diffs, and readiness snapshots independently.
- Define partition identity in `djls-project::loading::files` with a typed `FileSetPartitionId` enum/newtype, not string IDs. In 3A2 only `FirstParty` is active. Add the documented precedence scale, conflict detection, and lower-precedence resurrection in Phase 6B when configured-template-directory and installed-app partitions exist and can test the behavior against real callers.
- Define a full handle-free merged data shape such as `MergedDiscoveredSourceFileSetData` only for project-owned comparison, full-reset/config-restart cases, and tests. It contains merged roots/files as `DiscoveredSourceFile` values, not `LoadedSourceFile` values and not `File` handles. Do not make this full snapshot the normal database handoff for every partition update.
- Define an incremental materialization patch shape, for example `ProjectSourceFilesMaterializationPatch`, containing changed roots, upserted discovered files, removed paths/files, and the updated summary needed to keep the materialized `SourceFileSet` coherent.
- Define one project-owned merge seam, for example `merge_partition_patch(current, patch) -> ProjectSourceFilesUpdate`. In 3A2 this API updates the first-party partition readiness, diffs the previous merged view against the new merged view, maps neutral traversal/root evidence into `ProjectSourceFilesIssue`, and derives the incremental materialization patch. Phase 6B extends the same seam with multi-partition precedence, conflict detection, and lower-precedence resurrection. Full merged snapshots remain available inside the seam for reset/tests, but the steady-state handoff is the patch.
- Define `ProjectSourceFilesUpdate` as the only project-owned handoff aggregate between project-loading policy and database materialization. It contains the updated private `ProjectFileSetPartitions`, the incremental materialization patch, the applied partition/node transition for the file-loading row that produced the update, and any typed merge issues/warnings needed by executors for reporting. `djls-db` may consume only the materialization patch through public accessors; it must not inspect partitions, precedence, conflicts, or `ReadyProjectSourceFiles` internals.
- Expose only the small source-file loading surface needed outside the merge/apply seam: current ready query snapshot, `ProjectSourceFilesApplied`/applied node transition for `node_status_from_readiness`, a narrow partition/root readiness projection, and typed issues for reporting. A ready snapshot may carry partition state privately so later merges have one durable owner, but public consumers should see the merged `SourceFileSet` plus readiness predicates such as whether a required template directory/root is loaded, deferred, unavailable, or stale. Do not expose `ProjectFileSetPartitions`, partition snapshots, merged discovered data, materialization patch internals, update internals, or discovered-only source-file state as general APIs outside `djls-project::loading::files` and the `djls-db` apply boundary.
- Add a source-file coherence invariant: partition snapshots, the project-owned merged view, the incremental materialization patch, the materialized `SourceFileSet`, preserved `File` handles, and `ProjectLoadingState.source_files = ProjectSourceFilesAvailability::Ready(files)` must describe the same `ReadyProjectSourceFiles` after apply. No executor, fixture, or helper may update only one side of those source files.
- `DjangoDatabase` must not merge patches, construct `ReadyProjectSourceFiles`, or know partition policy. It should expose a policy-free materialization method that accepts only the incremental materialization patch, materializes changed roots/files/deletions where possible, preserves existing `File` handles for unchanged paths, compares before setters, stores/updates the neutral `SourceFileSet` input, and returns a materialization result such as `SourceFileSetMaterialized { source_file_set, handle_changes, issues }`. `DjangoDatabase` must not know Django partition names, precedence values, conflict policy, resurrection rules, or private `ProjectFileSetPartitions` fields.
- Add a project-owned finalize/constructor API, for example `finalize_project_source_files(update, materialized) -> ProjectSourceFilesApplyResult`. It combines the private partition state from `ProjectSourceFilesUpdate` with the materialized `SourceFileSet`, maps any `SourceFileMaterializationIssue` into project-owned typed issues, constructs `ReadyProjectSourceFiles`, sets `ProjectLoadingState.source_files` to `ProjectSourceFilesAvailability::Ready(files)` for success or to a terminal `Deferred`/`Unavailable`/`Failed` state for materialization/apply failure, and returns `ProjectSourceFilesApplied` with the files and applied partition/node transition for the success case. The neutral driver consumes that applied value for `node_status_from_readiness`; callers must not reconstruct status from side state.
- Wrap database materialization plus project finalization in one shared apply operation for executors, for example `apply_project_source_files(db, update) -> ProjectSourceFilesApplyResult`. This is the only public apply seam for source-file updates. The CLI adapter calls it directly; the LSP adapter calls the same operation through `GenerationGuard`. Neither adapter should sequence merge, database materialization, finalization, loading-state mutation, or status projection itself.
- Store aggregate project source readiness in the Salsa-visible `ProjectLoadingState` field exposed by `djls_project::Db`, not in `djls_source::Db` or mutex-only side state. Keep file-loading node readiness as applied partition/node readiness carried in `ProjectFileSetPartitions`/`ProjectSourceFilesUpdate`; do not mirror executor-local task state into `ProjectLoadingState`.
- Define a neutral `ProjectLoadingReset` or `begin_project_loading_run` apply intent that updates `ProjectLoadingState.source_files` to `ProjectSourceFilesAvailability::Loading` or `ProjectSourceFilesAvailability::Stale { previous }` before any task output applies. Do not require `GenerationGuard` in 3A2; the CLI wires this intent directly in 3A3 and the LSP wires it through `GenerationGuard` in 3A4. `begin_project_loading_run` and `apply_project_source_files` are the only production write APIs for `ProjectLoadingState.source_files`; add a cleanup gate that searches for direct generated Salsa setter use and allows it only inside the project-owned reset/apply module and narrowly scoped test fixtures.
- Test/fixture databases that do not model project source files should create a `ProjectLoadingState` initialized with a generation-free `Unavailable` state and typed fixture issue.
- Implement `djls_project::Db` for `DjangoDatabase`, `djls-bench::Db`, and semantic test databases.
- After this subphase, neither `djls-server`, `djls-db`, nor `djls-source` owns Django file-selection policy, partition IDs, partition precedence, or readiness state.

**Code shape**:
These are internal seam shapes unless the bullets above explicitly name them as exposed. Use `pub(crate)`/private fields and narrow module re-exports; `pub` below describes shape, not a broad public API commitment.

```rust
pub enum ProjectSourceFilesAvailability {
    Loading,
    Ready(ReadyProjectSourceFiles),
    Deferred { issue: ProjectSourceFilesIssue, previous: Option<ReadyProjectSourceFiles> },
    Unavailable { issue: ProjectSourceFilesIssue, previous: Option<ReadyProjectSourceFiles> },
    Failed { issue: ProjectSourceFilesIssue, previous: Option<ReadyProjectSourceFiles> },
    Stale { previous: ReadyProjectSourceFiles },
}

pub struct ReadyProjectSourceFiles {
    partitions: ProjectFileSetPartitions,
    merged: SourceFileSet,
}

impl ReadyProjectSourceFiles {
    pub fn merged(&self) -> SourceFileSet;
}

pub(crate) enum ProjectSourceFilesBuildState {
    Discovered(ProjectSourceFilesDiscovered),
    Materialized(ReadyProjectSourceFiles),
}

pub(crate) struct ProjectSourceFilesDiscovered {
    partitions: ProjectFileSetPartitions,
}

pub struct ProjectFileSetPartitions {
    partitions: Vec<ProjectFileSetPartitionSnapshot>,
}

pub struct ProjectFileSetPartitionSnapshot {
    partition: FileSetPartition,
    roots: Vec<SourceRoot>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
    readiness: ProjectFilePartitionReadiness,
}

pub enum ProjectFilePartitionReadiness {
    Loading,
    Ready { summary: FileSetSummary },
    Deferred { issue: ProjectSourceFilesIssue, previous: Option<FileSetSummary> },
    Skipped { issue: ProjectSourceFilesIssue, previous: Option<FileSetSummary> },
    Unavailable { issue: ProjectSourceFilesIssue, previous: Option<FileSetSummary> },
    Stale { previous: Option<FileSetSummary> },
}

pub enum FileSetPartitionId {
    FirstParty,
    // Phase 6B adds configured-template-directory and installed-app variants with their real identity payloads.
}

pub struct PartitionPrecedence(u16);

impl PartitionPrecedence {
    pub const FIRST_PARTY: Self = Self(100);
}

pub struct FileSetPartition {
    id: FileSetPartitionId,
    precedence: PartitionPrecedence,
}

pub enum ProjectSourceFilesIssue {
    MissingRoot { root: SourceRootId, path: Utf8PathBuf },
    DuplicateRoot { root: SourceRootId, duplicate_path: Utf8PathBuf },
    WalkFailed { root: SourceRootId, path: Utf8PathBuf, error_kind: std::io::ErrorKind },
    PartitionConflict { path: Utf8PathBuf, winner: FileSetPartitionId, shadowed: FileSetPartitionId },
    FixtureUnavailable { surface: ProjectSourceFilesFixtureSurface },
    StaleDocument { path: Utf8PathBuf },
    MaterializationFailed { path: Utf8PathBuf, error_kind: std::io::ErrorKind },
}

pub enum ProjectSourceFilesFixtureSurface {
    SourceFiles,
    Partitions,
    Materialization,
}

pub struct PartitionedSourceFilePatch {
    partition: FileSetPartition,
    roots: Vec<SourceRoot>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
}

pub struct MergedDiscoveredSourceFileSetData {
    roots: Vec<SourceRootEntry>,
    files: Vec<DiscoveredSourceFile>,
    summary: FileSetSummary,
}

pub struct ProjectSourceFilesMaterializationPatch {
    changed_roots: Vec<SourceRootEntry>,
    removed_roots: Vec<SourceRootId>,
    upserted_files: Vec<DiscoveredSourceFile>,
    removed_files: Vec<Utf8PathBuf>,
    summary: FileSetSummary,
}

pub struct ProjectFileLoadingTransition {
    partition: FileSetPartition,
    readiness: ProjectFilePartitionReadiness,
}

pub struct ProjectSourceFilesUpdate {
    partitions: ProjectFileSetPartitions,
    materialization: ProjectSourceFilesMaterializationPatch,
    applied_transition: ProjectFileLoadingTransition,
    issues: Vec<ProjectSourceFilesIssue>,
}

pub struct SourceFileSetMaterialized {
    source_file_set: SourceFileSet,
    handle_changes: SourceFileHandleChanges,
    issues: Vec<SourceFileMaterializationIssue>,
}

pub enum SourceFileMaterializationIssue {
    MissingRoot { root: SourceRootId },
    MaterializationFailed { path: Utf8PathBuf, error_kind: std::io::ErrorKind },
}

pub struct SourceFileHandleChanges {
    preserved: usize,
    created: usize,
    removed: usize,
}

pub enum ProjectSourceFilesApplyResult {
    Applied(ProjectSourceFilesApplied),
    Deferred { transition: ProjectFileLoadingTransition, issue: ProjectSourceFilesIssue, previous: Option<ReadyProjectSourceFiles> },
    Unavailable { transition: ProjectFileLoadingTransition, issue: ProjectSourceFilesIssue, previous: Option<ReadyProjectSourceFiles> },
    Failed { transition: ProjectFileLoadingTransition, issue: ProjectSourceFilesIssue, previous: Option<ReadyProjectSourceFiles> },
}

pub struct ProjectSourceFilesApplied {
    files: ReadyProjectSourceFiles,
    transition: ProjectFileLoadingTransition,
    issues: Vec<ProjectSourceFilesIssue>,
}
```

**Tests and stop gate**:
- **3A2a gate**: stop after the `ProjectLoadingState` shell and fixture DB impls compile with generation-free source unavailable states: `cargo test -p djls-project loading_state`.
- **3A2b gate**: stop after root-construction, first-party discovery file-policy, and first-party apply-seam tests pass in the project crate, including private-constructor enforcement, `ProjectSourceFilesIssue` construction for missing/duplicate/walk cases, partition readiness/status construction through the merge seam, overlapping-root longest-prefix ownership/deduplication, and root removal by `SourceRootId`: `cargo test -p djls-project files`. Conflict detection and lower-precedence resurrection tests land in Phase 6B with the first real non-first-party partitions.
- **3A2c gate**: stop after database source-file materialization tests preserve `File` handles for unchanged paths, materialize only changed roots/files/deletions from `ProjectSourceFilesMaterializationPatch` where possible, return `SourceFileSetMaterialized`, and stay partition-policy-free: `cargo test -p djls-db source_file_set`.
- **3A2c gate**: stop after round-trip coherence tests prove partitions -> project-owned merged view -> materialization patch -> `SourceFileSetMaterialized` -> project-owned finalization -> `ProjectLoadingState.source_files = ProjectSourceFilesAvailability::Ready(ReadyProjectSourceFiles)` -> `ProjectSourceFilesApplied.files` stay coherent across an apply and config restart, while preserving unchanged `File` handles and preserving the applied partition/node transition separately from aggregate source-file readiness: `cargo test -p djls-db source_file_set_roundtrip` or the equivalent `source_file_set` test module.
- **3A2c gate**: stop after terminal transition tests prove activity/materialization/apply failures do not leave `ProjectLoadingState.source_files` stuck in `Loading`/`Stale`; they must produce query-visible `Deferred`, `Unavailable`, or `Failed` availability with typed issue and previous ready files when available.
- **3A2c gate**: stop after Salsa invalidation tests prove `ProjectLoadingState.source_files` transitions from `Loading`/`Unavailable` to `Ready` invalidate a minimal tracked probe query: `cargo test -p djls-project loading_state_invalidation`.

#### Phase 3A3: Run the source-file node through the CLI executor

**Superseded note**: This section records completed runner/CLI orchestration work. Keep the neutral runner and CLI adapter shape. Any mention of a semantic reset intent is superseded by the Architecture correction; future run-start state stays in the executor.

**Files**:
- `crates/djls-project/src/loading.rs`
- `crates/djls-project/src/loading/plan.rs`
- `crates/djls-project/src/loading/driver.rs`
- `crates/djls-project/src/loading/effects.rs`
- `crates/djls/src/commands/check.rs`
- `crates/djls/src/loading.rs`

**Edits**:
- Depends on **3A2 source-file readiness and merge handoff**.
- Add `loading/plan.rs` as the shared, non-LSP loading-graph boundary. In Phase 3A3 it should be deliberately concrete: `NodeId::SourceFileSet`, a one-node active plan, the initial static `NODE_SPECS` manifest row for that node, the node-status projection API, and no milestone IDs, milestone prerequisites, or node-to-milestone advancement until Phase 5 introduces `workspace-ready`.
- Add a minimal neutral loading runner in `djls-project`, for example `run_loading_plan(plan, effects, observer)`. In Phase 3A3 it should be a boring one-node runner for `NodeId::SourceFileSet`: run reset, run the source-file activity, call one node-level apply intent for the resulting patch, call `node_status_from_readiness` on the applied value, emit observer events, and stop. It must not introduce registry/plugin machinery, dynamic node factories, generic schedulers, node traits, or milestone APIs for a one-node plan.
- When `project-discovery-set` lands in Phase 3C, add a two-node driver test before expanding any driver abstractions. The second node should prove the existing runner shape and justify any generalization; do not retrofit untested generic machinery.
- The driver must not import CLI, server, LSP, database concrete types, or formatting/reporting policy.
- Add a narrow execution/apply contract in `djls-project::loading` using neutral node IDs, domain outcomes, and apply intents. In Phase 3A3 this contract can be source-file-specific rather than a generic node framework. Expose one file-node apply intent such as `apply_source_file_patch(patch) -> ProjectSourceFilesApplyResult`; the concrete effect adapter may choose direct vs guarded invocation, but the one apply intent must own merge, policy-free DB materialization, project finalization, loading-state mutation, and status evidence. Callers and later file-loading nodes must not reimplement that choreography. `djls-project` defines the contract and event/outcome types only; the server and CLI crates implement concrete activity execution and guarded/direct apply behavior.
- Add a separate neutral observer/event-sink contract for stable node events. LSP and CLI adapters translate those events into progress/logs or terminal diagnostics respectively. Do not put progress/log emission or terminal formatting methods on the execution/apply contract.
- Compose the first active plan with the `source-file-set` node only. Add the `project-discovery-set` node in Phase 3C when the discovery activity exists. Do not predeclare future nodes.
- The `source-file-set` activity takes `SourceFilesLoadRequest` and returns a project-owned `PartitionedSourceFilePatch` for the first-party partition. It does not materialize `File` handles and does not apply database setters.
- The concrete `CliLoadingExecutor` lives in `crates/djls`, implements the neutral effect contract, and is invoked by `djls check`. It runs the direct reset intent, lets the neutral runner execute the `source-file-set` node, implements the node-level `apply_source_file_patch` intent with direct database mutation through the single shared apply seam, returns `ProjectSourceFilesApplyResult` to the neutral runner for terminal projection, and reports terminal diagnostics/warnings. The CLI adapter must not call merge/materialize/finalize helpers directly outside that one apply intent.
- Have `djls check` use the same Phase 3 loading plan through the CLI effect adapter for the nodes that exist in this phase. Phase 10 may broaden CLI feature parity, but it must not be the first time the CLI exercises the graph.
- Every later phase that adds a real static graph node must update the loading-node table, add executor-neutral driver/plan tests, and add both LSP and CLI effect-adapter tests for that node in the same phase.
- Shared loading activities return typed outcomes only; they must not import LSP types, wrap outputs in `StartupUpdate`, emit progress, check startup generations, or advance readiness milestones.

**Tests and stop gate**:
- Stop after neutral runner/plan tests pass for the `source-file-set` node, including the concrete one-node path, the `NODE_SPECS` row, terminal-status projection through `node_status_from_readiness(ProjectSourceFilesApplied)`, projection-table coverage for source-file readiness classes, and observer event emission with in-process fake execution/apply effects and a recording observer: `cargo test -p djls-project loading`.
- Stop after the Phase 3 CLI effect adapter runs the active Phase 3 loading plan through `run_loading_plan`: `cargo test -p djls --test check`.

#### Phase 3A4a-3A4d: LSP guarded startup, progress, and configuration restart

**Superseded note**: Phase 3A4a-3A4c are completed and retained for generation/progress orchestration. Any old semantic reset behavior is superseded. Phase 3A4d remains future work and must use stable Project apply semantics.

**Files**:
- `crates/djls-server/src/client.rs`
- `crates/djls-server/src/server.rs`
- `crates/djls-server/src/session.rs`
- `crates/djls-server/src/startup.rs`

**Edits**:
- Depends on **3A2 source-file merge handoff**, **3A3 loading plan/executor boundary**, and the **Architecture correction** gate for any remaining 3A4d work.
- Implement this section as four hard gates:
  - **3A4a generation guard/apply**: generation creation, run-start control through `GenerationGuard`, `ApplyOutcome`, superseded-result propagation, and immutable input capture only.
  - **3A4b LSP source-file executor**: LSP effect adapter runs `source-file-set` through the neutral runner and guarded apply, without progress lifecycle or configuration restart.
  - **3A4c progress lifecycle**: work-done capability parsing, begin/report/end, and log fallback over existing node events.
  - **3A4d configuration restart**: `didChangeConfiguration` updates settings, captures a fresh input, restarts the active graph, and rejects superseded applies.
- Introduce an immutable background input boundary:
  - `ProjectLoadingSnapshot` is a **server-local** generation-free capture taken under a short session lock. It may contain only immutable captured data: workspace roots, client/default settings, versioned read-only document snapshots for open buffers, and current stable Salsa input handles.
  - `StartupRunInputs` is the LSP-owned wrapper captured under the same short lock. It contains `ProjectLoadingSnapshot`, the `StartupGeneration`/`GenerationGuard`, progress/log adapters, and any runtime policy needed by the LSP effect adapter.
  - Before calling `djls-project`, the LSP effect adapter lowers the server-local snapshot through shared per-node request builders such as `source_files_load_request(...)`, `project_discovery_load_request(...)`, `python_source_probe_request(...)`, or `environment_discovery_probe_request(...)`. The CLI effect adapter calls the same builders from CLI inputs and its database/read context. Adapters may differ in capture, apply, and reporting; they must not duplicate request-construction policy.
  - Those request structs contain only roots, client/default settings, structured config data, immutable source snapshots with document version/epoch data or current `File` source state, and source/file abstractions owned by `djls-source`, `djls-workspace`, `djls-conf`, or `djls-project`. They must not contain `Arc<Mutex<Session>>`, LSP types, live overlay handles, mutable buffer access, open-buffer storage internals, progress handles, or generation guards.
  - `didOpen`, `didChange`, and `didClose` update the live `File::source(db)` state and advance an open-document epoch before request handling observes the file. A loading apply/report path that used captured open-buffer text must compare captured versions against the live document table; stale captures return `ApplyOutcome::Rejected { reason: ApplyRejection::StaleDocument { ... } }` with file/path/captured/current evidence and follow the stale-document rejection policy instead of applying facts from older text.
  - Prefer request structs that carry paths and `File` identities over copied source text. When an activity can read source through `File::source(db)` at query time, it must use that path so open-buffer text participates through Salsa-visible file state rather than live LSP overlay handles.
  - `djls-project` activities receive node-specific request structs or plain domain values only. They must never receive `ProjectLoadingSnapshot` or `Arc<Mutex<Session>>`.
  - A cloned `DjangoDatabase` may be used for report-only validation or speculative measurement when the clone captures the relevant Salsa inputs and `File` source state. Clone-only results must not advance node terminal status, milestones, or user-facing "ready" progress. Readiness-advancing observation nodes must observe the table's readiness source on the live database under the appropriate short-lock/read boundary. All mutation still returns typed apply intents and is applied to the live database under a short lock.
- Introduce `LspLoadingExecutor` in `djls-server`, backed by `StartupController`, `StartupGeneration`, `GenerationGuard`, `StartupRunInputs`, and `StartupProgress`. It implements the neutral loading execution/apply contract and observer/event sink, then calls `djls_project::loading::run_loading_plan`; it must not duplicate graph-order or terminal-policy logic from the driver.
- Move work-done progress capability parsing here: add `work_done_progress: bool` to `ClientCapabilities`, parse `window.workDoneProgress`, expose `ClientInfo::work_done_progress`, and test explicit true/false/missing cases.
- Define typed run outcomes. `GenerationGuard::apply` must return `ApplyOutcome<T>`, not `bool`; `Applied` carries the applied value needed for downstream status projection. Superseded reset/apply intents must propagate to the run outcome. Stale document text must use a rejected apply outcome with evidence, not `Superseded`. Do not call executor rejection Project Facts `Stale`.
- Centralize progress finishing: `run_startup` calls an inner runner that returns `StartupRunOutcome`, then calls exactly one `progress.finish(outcome)` path for `Succeeded`, `Failed`, or `Superseded`. Do not leave success/failure/superseded finish behavior to comments or caller discipline.
- Add `StartupProgress` methods for real node events: create token if supported, begin before the first task event, report task events, and finish exactly once from the typed run outcome. Do not add milestone reporting in Phase 3.
- Before spawning node work, the LSP executor checks/records server-local run-start state through `GenerationGuard`. Run start must not write `Loading` or `Stale` Project Facts. A superseded generation must return `StartupRunOutcome::Superseded` before activity work starts.
- `LspLoadingExecutor` runs the same `source-file-set` node as the CLI executor through the neutral runner and implements the same node-level `apply_source_file_patch` intent through `GenerationGuard::apply`. Superseded guarded applies must surface as `StartupRunOutcome::Superseded`.
- Add an injected executor seam test for the real startup controller path: start a startup run, block the `source-file-set` node before apply, issue one representative template/diagnostic request, and assert it returns a valid degraded response without waiting for the background task or holding the `Session` lock across the blocked node.
- Add a stale-document interleaving test: capture an open buffer, block the loading node, send `didChange` or `didClose`, then unblock the node and assert the old snapshot cannot apply Project Facts or diagnostics for the older document text.
- Have `initialized` call the synchronous `StartupController::start(...)`, which captures `StartupRunInputs`, spawns the LSP executor, and returns immediately.
- Route `didChangeConfiguration` through the same `StartupController::restart(...)` path as startup: update client/default settings under a short lock, capture a new `StartupRunInputs`, record server-local run-start state without mutating Project Facts, and run the active loading graph for the nodes that exist in this phase. Remove any old `project().is_none()` guard from the new path; configuration changes must restart root-scoped loading against the stable `Project` handle.
- A configuration-change run must supersede older startup/configuration runs through `GenerationGuard`, not by mutating generation IDs inside `ProjectLoadingState` and not by writing Project Facts `Stale` for executor-only rejection.

**Code shape**:
```rust
pub enum ApplyOutcome<T> {
    Applied(T),
    Superseded,
    Rejected { reason: ApplyRejection },
}

pub enum ApplyRejection {
    StaleDocument {
        file: File,
        path: Utf8PathBuf,
        captured: DocumentVersion,
        current: DocumentVersion,
    },
}

pub enum ObservationOutcome<T> {
    Observed(T),
    Superseded,
}

pub enum StartupRunOutcome {
    Succeeded,
    Failed(StartupFailure),
    Superseded { generation: StartupGeneration },
}

impl GenerationGuard {
    pub(crate) async fn apply(
        &self,
        session: &Arc<Mutex<Session>>,
        apply: impl FnOnce(&mut Session) -> Result<T, ApplyRejection>,
    ) -> ApplyOutcome<T> {
        // generation mismatch returns ApplyOutcome::Superseded;
        // stale captured document text returns ApplyOutcome::Rejected { reason: ApplyRejection::StaleDocument { ... } }
    }
}

async fn run_startup(...) -> StartupRunOutcome {
    progress.begin().await;
    let outcome = run_startup_inner(...).await;
    progress.finish(&outcome).await;
    outcome
}
```

**Tests and stop gate**:
- **3A4a gate**: stop after generation guard tests cover immutable `StartupRunInputs` capture, versioned captured document snapshots for opened unsaved/changed files, stale-document rejection after `didChange`/`didClose` during blocked loading with file/path/captured/current evidence, guarded reset, `ApplyOutcome::Superseded`, `ApplyOutcome::Rejected { reason: ApplyRejection::StaleDocument { ... } }`, guarded observation/reporting, and superseded propagation before node work starts: `cargo test -p djls-server startup_generation` or equivalent startup tests.
- **3A4b gate**: stop after LSP effect-adapter tests pass for the `source-file-set` node through `run_loading_plan`, with guarded apply and no progress assertions: `cargo test -p djls-server startup_source_files` or equivalent startup tests.
- **3A4b gate**: stop after a deterministic request-while-loading test proves a blocked active `source-file-set` startup run does not block a representative request and returns a valid degraded response: `cargo test -p djls-server startup_request_while_loading` or equivalent startup test.
- [x] **3A4c gate**: stop after client work-done progress capability tests pass: `cargo test -p djls-server client::tests::work_done_progress` — 3 passed. Evidence: `ClientCapabilities` parses `window.workDoneProgress` true/false/missing and `ClientInfo::supports_work_done_progress()` exposes it.
- [x] **3A4c gate**: stop after progress lifecycle tests cover begin/report/finish and log fallback over stable observer events: `cargo test -p djls-server startup_progress` — 3 passed after review follow-up; `cargo test -p djls-server work_done_progress` — 5 passed. Evidence: `StartupProgress` observes the neutral loading runner events, emits begin/node/finish events through a recording reporter in tests, uses tracing as the fallback reporter, and `run_startup_source_files_with_gate` has exactly one finish call after the inner runner returns a typed `StartupRunOutcome`. Work-done progress uses generation-scoped tokens, a nonblocking dispatcher, and an explicit created/active state so create failure suppresses begin/report/end.
- **3A4d gate**: stop after configuration-change tests prove `didChangeConfiguration` restarts the active loading graph with the stable `Project` handle, does not write run-start `Loading`/`Stale` Project Facts, and rejects superseded applies from the older run without mutating Project Facts: `cargo test -p djls-server configuration_restart`.

#### Architecture correction: Stable Project root before Phase 3B

**Status**: Required before resuming Phase 3A4d/3B feature work.

**Decision**:
- Adopt the stable `djls_project::Project` root model from `architecture-decision-project-root.md`.
- Keep server/CLI loading state outside Salsa.
- Remove `ProjectLoadingState` as the semantic readiness root.

**Files**:
- `crates/djls-project/src/project.rs` or equivalent root module
- `crates/djls-project/src/db.rs`
- `crates/djls-project/src/loading/state.rs`
- `crates/djls-project/src/loading/files.rs`
- `crates/djls-db/src/db.rs`
- `crates/djls-bench/src/db.rs`
- `crates/djls-semantic/src/testing.rs`
- `crates/djls-server/src/startup.rs`
- `crates/djls/src/loading.rs`

**Edits**:
- Add `djls_project::Project` as the stable Salsa input for Project Facts. It starts as a virtual project with no discovered facts.
- Replace `djls_project::Db::project_loading_state() -> ProjectLoadingState` with `djls_project::Db::project() -> Project`.
- Initialize the `Project` handle once in every concrete/test/bench database. Do not store it behind `Arc<Mutex<Option<_>>>`, and do not swap the handle during reload.
- Move current source-file facts into a project source-inventory field owned by `Project`. The source inventory may internally preserve partition information, but aggregate files and per-partition/node readiness must not become separate semantic authorities.
- Remove production reset writes that set Salsa state to `Loading` or `Stale` before work starts. Run-start state lives in the CLI/LSP executor.
- Change stale-document rejection so it leaves Project Facts unchanged and reports/restarts through executor outcomes.
- Preserve old facts on failed reload by not writing replacement facts. Record durable diagnostics only when they are Project Facts.
- Keep the neutral loading runner as orchestration. Its successful apply paths mutate `Project` fields through setters; its node terminal statuses feed CLI/LSP progress and milestones only.
- Delete or quarantine `ProjectLoadingState`, `ProjectSourceFilesAvailability`, `ProjectDiscoveryAvailability`, and `ProjectEnrichmentState` as semantic readiness inputs. If a temporary bridge is needed, document the exact deletion gate in this section and prevent new phases from consuming it.
- Keep future Phase 3B/3C/3D prose aligned with stable `Project` facts; those sections have been rewritten in this planning slice and must not reintroduce `ProjectLoadingState` as a dependency.

**Behavior-preservation migration matrix**:

| Current gate/category | Replacement expectation |
|---|---|
| Source-file materialization and round-trip coherence | Same behavior, but coherence ends at `Project.source_inventory` rather than `ProjectLoadingState.source_files`. |
| Terminal source-file failures | Domain failures become durable inventory diagnostics or executor outcomes; failed/superseded reloads do not erase prior facts. |
| Salsa invalidation probe | Probe reads a tracked `Project` field and invalidates when that field changes. |
| CLI source-file loading | CLI still runs the shared graph and applies facts directly to `Project`. |
| LSP guarded source-file apply | LSP still applies through `GenerationGuard`; superseded/stale-document rejection leaves Project Facts unchanged. |
| Request while loading | Requests remain responsive; degraded/loading UX is derived from server orchestration plus current Project facts, not a Salsa loading flag. |
| Progress lifecycle | Unchanged; progress observes runner events and startup outcomes. |

**Tests and stop gate**:
- Stop after the stable `Project` root compiles in production, bench, and semantic test databases: `cargo test -p djls-db --no-run`, `cargo test -p djls-bench --no-run`, and `cargo test -p djls-semantic --no-run`.
- Stop after source-file materialization/round-trip tests pass against the new project source inventory: `cargo test -p djls-db source_file_set`.
- Stop after neutral loading runner tests pass without depending on `ProjectLoadingState`: `cargo test -p djls-project loading`.
- Stop after LSP startup source-file tests pass and stale-document rejection no longer writes failed Project Facts: `cargo test -p djls-server startup_source_files`.
- Stop after request-while-loading behavior remains valid: `cargo test -p djls-server startup_request_while_loading`.
- Stop after CLI check still exercises the shared source-file node: `cargo test -p djls --test check`.
- Stop after a cleanup search proves no production code consumes `Db::project_loading_state()` or `ProjectLoadingState`: `rg "project_loading_state|ProjectLoadingState" crates -g '*.rs'` with only documented temporary bridge/test references, or no matches.
- Stop after formatting and build checks pass: `just fmt --check` and `cargo build -q`.

#### Phase 3B: Discovery and enrichment Project-root scaffolding

**Status**: Future phase, rewritten for the stable `djls_project::Project` root. The earlier `ProjectLoadingState` discovery/enrichment scaffold is superseded and must not be extended.

**Files**:
- `crates/djls-project/src/db.rs`
- `crates/djls-project/src/project.rs` or equivalent root module
- `crates/djls-project/src/discovery.rs`
- `crates/djls-project/src/enrichment.rs`
- `crates/djls-db/src/db.rs`
- `crates/djls-bench/src/db.rs`
- `crates/djls-semantic/src/testing.rs`

**Edits**:
- Depends on the **Architecture correction** gate, not on the old loading-state shell.
- Add or finalize `Project.discovery` and `Project.enrichment` tracked fields on the stable `Project` root. These fields hold domain facts only; they must not contain `Loading`, `Stale`, generation IDs, progress, or queued/running executor state.
- Define `ProjectDiscovery` or an equivalent domain fact shape with initial absent facts, ready discovery facts, and durable unavailable/diagnostic facts. A missing first load is represented as absent or unavailable domain facts plus server/CLI loading state, not as a Salsa loading flag.
- Define `ProjectDiscoverySet` as the root-scoped discovery snapshot for the workspace. It contains one `RootDiscoveryInput` per workspace/source root and must not choose a primary root.
- Define the project-owned configuration seed type `DjangoEnvironmentSeed`. Lower `djls_conf::DjangoEnvironmentConfig` into this type in Phase 3C; do not store `djls-conf` DTOs in Project Facts.
- Define `RootDiscoveryInput` as a root-scoped Salsa input snapshot of config values relevant to discovery: root, interpreter, settings module seed, configured Django environment seeds, `pythonpath`, canonical env vars, and root-specific discovery issues. Do not name this as a selected/global Django Settings Module.
- Define typed discovery issue variants for config load failures/fallbacks, interpreter discovery failures, env-file load failures, no workspace roots, and fixtures that do not model discovery. The config/interpreter/env-file failures must carry root/source/cause fields; do not use bare failure variants or generic strings.
- Define `ProjectEnrichment` or equivalent as optional domain facts with initial absent/disabled/unavailable states only. Phase 9 expands this with runtime/deep enrichment hints and failures. Do not model enrichment as readiness for core startup.
- Do not wire root config loading, interpreter discovery, env-file loading, or discovery apply in this subphase.

**Code shape**:
```rust
#[salsa::input]
pub struct Project {
    workspace_roots: ProjectWorkspaceRoots,
    source_inventory: ProjectSourceInventory,
    discovery: ProjectDiscovery,
    diagnostics: ProjectDiagnostics,
    enrichment: ProjectEnrichment,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectDiscovery {
    Absent,
    Ready(ProjectDiscoverySet),
    Unavailable { issues: Vec<ProjectDiscoveryIssue> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEnrichment {
    Absent,
    Disabled,
    Unavailable { issues: Vec<ProjectEnrichmentIssue> },
}
```

Names may change during implementation, but the architectural constraint is fixed: discovery and enrichment are fields under stable `Project`, not fields under a readiness singleton.

**Tests and stop gate**:
- Stop after discovery/enrichment scaffolding compiles on the stable `Project` root without `ProjectLoadingState`: `cargo test -p djls-project discovery` or the nearest targeted module test.
- Stop after production, bench, and semantic test databases initialize one stable `Project` handle and no longer add new `ProjectLoadingState` consumers: `cargo test -p djls-db --no-run`, `cargo test -p djls-bench --no-run`, and `cargo test -p djls-semantic --no-run`.

#### Phase 3C: Root discovery data through shared activity code

**Status**: Future phase, rewritten for stable `Project` facts.

**Files**:
- `crates/djls-conf/src/settings.rs`
- `crates/djls-conf/src/lib.rs`
- `crates/djls-project/src/discovery.rs`
- `crates/djls-project/src/loading/settings.rs`
- `crates/djls-db/src/db.rs`
- `crates/djls-server/src/startup.rs`
- `crates/djls-project/src/availability.rs` or equivalent pure Project Facts projection
- `crates/djls-semantic/src/availability.rs` only if semantic-feature-specific projection is needed
- `crates/djls/src/loading.rs`
- `crates/djls/src/commands/check.rs`

**Edits**:
- Depends on **3A3/3A4 executor shapes**, the **Architecture correction** gate, and **3B stable Project discovery scaffolding**.
- Implement Phase 3C as four hard gates:
  - **3C1 structured root settings load**: `djls-conf` owns config file/schema loading and returns structured root load outcomes.
  - **3C2 discovery data and Project apply**: `djls-project` owns discovery data/coordinator helpers; `djls-db` materializes discovery inputs and mutates `Project.discovery` through setters.
  - **3C3 project-discovery loading node**: add `project-discovery-set` to the graph, CLI/LSP effect adapters, and configuration restart.
  - **3C4 availability/request matrix**: move pure Project Facts projection to `djls-project::availability` and extend degraded request tests.
- Add the `project-discovery-set` node to the active loading plan and loading-node table in this subphase. Keep it as a normal node with terminal status derived from the applied discovery outcome; do not introduce readiness milestones in Phase 3.
- In `djls-conf`, add a structured root settings load API/outcome, for example `load_root_settings(root, client_settings) -> RootSettingsLoadOutcome`. The outcome must carry the root, loaded `Settings` on success, config source path when known, typed error category on failure, and whether client/default settings were used as fallback. Do not require callers to reverse-engineer provenance from a generic `ConfigError` string.
- Add `RootSettingsLoadIssueKind` or equivalent typed categories for I/O, parse, schema/deserialization, and unsupported config shapes.
- Add a shared project activity such as `build_project_discovery_data(request: ProjectDiscoveryLoadRequest) -> ProjectDiscoverySetData` in `djls-project::loading::settings`.
- Keep `project-discovery-set` as a coordinator, not a hidden mini-pipeline. `djls-conf` owns config file/schema loading. `djls-project` should place interpreter discovery, env-file loading, config DTO lowering, and discovery data construction in typed helper modules with typed issues, then have the loading node assemble `ProjectDiscoverySetData` from those helpers.
- Define `ProjectDiscoverySetData` and `RootDiscoveryData` as plain executor-neutral data. Loading activities return these data structs; database apply methods materialize/update Salsa inputs and set `Project.discovery`.
- For each root from `ProjectDiscoveryLoadRequest`, call the structured `djls-conf` root settings load API outside the session lock. On a per-root config error, record a typed `ConfigLoadFailed` issue with source path and cause category where available; if client/default settings are used as fallback, also record a paired `ConfigFallbackUsed` provenance marker in that root's `RootDiscoveryData`.
- Lower any `djls_conf::DjangoEnvironmentConfig` values in the load outcome into `DjangoEnvironmentSeed` / project-owned seed types before constructing `RootDiscoveryData`.
- Interpreter discovery and env-file loading failures must record structured root/source/cause data using `InterpreterDiscoveryIssueKind` and `EnvFileLoadIssueKind`; do not use bare failure variants or generic strings.
- Store environment variables in a deterministic Salsa-visible shape, for example `ProjectEnvVars` wrapping a `BTreeMap<String, String>` or a canonical sorted vector. Resolve duplicate keys before constructing/applying `RootDiscoveryData` using documented source precedence; if a lower-precedence value is discarded and that matters for diagnostics/provenance, record a typed discovery issue or provenance marker. Do not store unordered `Vec<(String, String)>` in discovery inputs.
- `djls-server` must not perform interpreter discovery, env-file loading, project config loading, or `ProjectDiscoverySet` / `RootDiscoveryInput` construction directly. It lowers `StartupRunInputs` into `ProjectDiscoveryLoadRequest`, invokes the shared activity through the LSP effect adapter, and applies the result through `GenerationGuard`.
- `DjangoDatabase::apply_project_discovery_data` is the only production place that creates or updates `ProjectDiscoverySet` / `RootDiscoveryInput` Salsa inputs from the plain data and then sets `Project.discovery`.
- The CLI effect adapter invokes the same activity through the neutral driver and applies the same plain data directly. This node is not complete until both CLI and LSP effect-adapter tests pass.
- Starting a discovery run must not write `Loading` or `Stale` Project Facts. Failed/superseded discovery runs leave prior `Project.discovery` facts intact; durable config/interpreter/env diagnostics may be updated only when represented as Project Facts.
- Extend the `didChangeConfiguration` restart path so changed client/default settings restart root-scoped discovery through the same loading graph. The new run must reload structured root settings, apply new `ProjectDiscoverySetData` only on coherent success, and reject superseded discovery applies from older runs.
- Extend the shared project/semantic availability API introduced in Phase 1 below IDE presentation. Move pure Project Facts projection to `djls-project::availability`, for example `djls_project::ProjectFactsForFile` / `ProjectFactsAvailability`; if a projection is semantic-feature-specific, put it in `djls-semantic`. `djls-ide` should translate already-classified results to LSP-shaped behavior only.
- In Phase 3C, centralize how request paths interpret absent/unavailable source inventory, absent/unavailable discovery facts, and server/session in-flight loading state. Later phases extend the same projection for `Unknown`/`Ambiguous` environment selection. Diagnostics, completions, navigation, references, and hover should consume project/semantic availability results instead of branching independently on raw loading-state enums in `djls-ide`.
- Extend degraded request tests for absent/unavailable source inventory and absent/unavailable discovery facts: diagnostics, completions, navigation, references, and hover must return parser/builtin-only, empty, no-target, or deferred results without panicking or falling back to the old Project fact bag. Add a shared matrix test for the project/semantic availability projection so new availability states are added in one place.

**Code shape**:
```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectDiscoverySetData {
    entries: Vec<RootDiscoveryData>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootDiscoveryData {
    root: Utf8PathBuf,
    interpreter: Option<Interpreter>,
    settings_module_seed: Option<String>,
    configured_environments: Vec<DjangoEnvironmentSeed>,
    pythonpath: Vec<String>,
    env_vars: ProjectEnvVars,
    issues: Vec<ProjectDiscoveryIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEnvVars {
    // Internally canonical: sorted by key with duplicate resolution already applied.
    vars: Vec<(String, String)>,
}
```

**Tests and stop gate**:
- **3C1 gate**: stop after `djls-conf` structured root settings load outcome tests preserve root, source path, typed error category, and fallback marker: `cargo test -p djls-conf root_settings_load`.
- **3C2 gate**: stop after discovery data/helper tests preserve config-load failures/fallback provenance, lower `djls-conf` DTOs into project-owned environment seeds, and canonicalize `ProjectEnvVars` deterministically: `cargo test -p djls-project loading_settings`.
- **3C2 gate**: stop after discovery apply tests mutate `Project.discovery` through setters, preserve old facts on failed/superseded reload, and invalidate discovery-dependent tracked queries when discovery facts change: `cargo test -p djls-project discovery_invalidation`.
- **3C3 gate**: completed in `ynlpuktv`; two-node runner tests prove `source-file-set -> project-discovery-set` ordering, `NODE_SPECS` coverage, terminal-status projection table behavior, successor execution, and observer events without registry/plugin machinery: `cargo test -p djls-project loading`.
- **3C3 gate**: completed in `ynlpuktv`; startup/executor tests cover applying root-scoped discovery data, CLI applying root-scoped discovery data, configuration restart/supersession preserving Project Facts, and config-load failure preservation in discovery data: `cargo test -p djls-server startup` and `cargo test -p djls --test check`.
- **3C4 gate**: completed in `sxrlwqyu`; pure availability ownership moved to `djls-project::availability`, the temporary Phase 1 semantic availability type/module was deleted, and no-discovery-set degraded request tests plus the shared project availability matrix pass: `cargo test -p djls-project availability` and `cargo test -p djls-server degraded`.

#### Phase 3D: Layout, concrete provenance, legacy queue cleanup, and dependency wiring

**Status**: Future phase, rewritten for stable `Project.source_inventory`.

**Files**:
- `crates/djls-project/src/layout.rs`
- `crates/djls-project/src/provenance.rs`
- `crates/djls-server/src/server.rs`
- `crates/djls-server/src/queue.rs`
- `crates/djls-db/Cargo.toml`
- `crates/djls-semantic/Cargo.toml`
- `crates/djls-bench/Cargo.toml`

**Edits**:
- Depends on stable `Project.source_inventory` from the **Architecture correction** gate and discovery facts from **3C**.
- Add `ProjectLayoutIndex` over the current ready `SourceFileSet` entries in `Project.source_inventory` and a domain outcome type such as `ProjectLayoutIndexOutcome`.
- Provide raw lookup APIs on the ready index: `file_path`, `file_for_path`, `children`, `descendant_files`, `files_by_name`, `files_by_extension`, `dirs_by_name`, and `python_package_dirs`.
- Prefer an explicit-project tracked query: `#[salsa::tracked(returns(ref))] pub fn project_layout_index(db: &dyn Db, project: Project) -> ProjectLayoutIndexOutcome`. Transitional root queries may call `db.project()` at the request boundary.
- Read the tracked `Project.source_inventory` field before branching. If source inventory is absent, unavailable, or known incomplete for the requested inventory, return a typed domain outcome. Never collapse absent/unavailable source inventory into an empty ready index. A ready empty index means source files are current and there are genuinely no indexed files.
- Keep direct `djls_source::Db::source_file_set()` access low-level and internal to concrete materialization/layout helpers unless a caller has a concrete source-layer reason. Project/semantic/IDE consumers should use project-owned source-inventory queries.
- Treat `__init__.py` as a Python package marker.
- Do not classify settings candidates, templates, config files, or Django roles in this phase.
- Settings candidates, module resolution, and environment discovery must branch on `ProjectLayoutIndexOutcome`; they must not treat absent/unavailable layout as "no conventional candidates".
- Add only concrete provenance support needed by Phase 3C discovery issues or immediately consumed Project Facts evidence. If there is no concrete Phase 3D consumer, delay `Provenance`, `OriginSet`, and `ProjectFactIssue` until the phase that first consumes them.
- If provenance is introduced here, keep it concrete: `OriginSet` should be a small bitflag-style value or explicit enum set, evidence should use `File`/`Span` for source evidence and `Utf8PathBuf` for path evidence, and `ProjectFactIssue` must not become a generic `Fact<T>` substitute.
- Remove all remaining `Queue` usage from startup and discovery paths.
- Delete `Queue` if no non-startup callers remain.
- If non-startup callers remain, document the bounded remaining use in `server.rs` and name the later phase that removes it. After Phase 3, using `Queue` for startup or background Django Discovery is a bug.
- Add a mechanical cleanup search: run `rg "Queue|enqueue|refresh_external_data|load_template_library_cache" crates/djls-server -g '*.rs'` and prove remaining matches are either deleted or explicitly bounded non-startup callers.
- Add `djls-project = { workspace = true }` to internal dependency groups where code now implements or consumes `djls_project::Db`.
- Keep dependency groups alphabetized and separated from third-party deps.

**Code shape**:
```rust
pub enum ProjectLayoutIndexOutcome {
    Ready(ProjectLayoutIndex),
    Absent { issue: ProjectLayoutIssue },
    Unavailable { issue: ProjectLayoutIssue },
}

pub enum ProjectLayoutIssue {
    SourceInventoryAbsent,
    SourceInventoryUnavailable { issue: ProjectSourceInventoryIssue },
    RequiredPartitionUnavailable { partition: FileSetPartitionId },
}
```

Names may change, but the outcome must be derived from stable Project facts, not executor loading state.

**Tests and stop gate**:
- Stop after layout index tests pass, including absent/unavailable source-inventory outcomes and invalidation through stable `Project.source_inventory`: `cargo test -p djls-project layout`.
- Stop after settings-candidate/layout integration tests prove absent or unavailable layout does not look like "no conventional candidates": `cargo test -p djls-project settings_candidates` or equivalent layout consumer tests.
- Stop after queue cleanup/removal tests or compile checks pass: `cargo test -p djls-server queue` and `cargo test -p djls-server startup`.
- Stop after the queue/cache cleanup search proves no startup/background Django Discovery bridge remains: `rg "Queue|enqueue|refresh_external_data|load_template_library_cache" crates/djls-server -g '*.rs'`.
- Stop after benchmark database compiles: `cargo test -p djls-bench --no-run`.
- Stop after the workspace builds: `cargo build -q`.

### Success Criteria

#### Automated Verification
**Phase 3A1 gate**
- [x] New crate compiles: `cargo test -p djls-project` — 22 passed.
- [x] Interpreter/env moved tests pass for helpers needed by the first loading node: `cargo test -p djls-project interpreter` — 12 passed; `cargo test -p djls-project env` — 11 passed.
- [x] Helper-move cleanup search passes with only intentional temporary semantic re-exports/callers: `rg "djls_semantic::project::(python|system)|load_env_file|Interpreter" crates -g '*.rs'` — remaining hits are new `djls-project` definitions/tests, temporary semantic re-exports, old semantic callers behind those re-exports, and `djls-db` callers using the temporary `djls_semantic` façade.
- [x] `djls-project` exposes moved helpers through specific root exports, not public helper modules. Evidence: `crates/djls-project/src/lib.rs` declares `mod env; mod interpreter; mod system;` and re-exports `load_env_file` / `Interpreter`.

**Phase 3A2a gate**
- [x] `ProjectLoadingState` shell and fixture DB impls compile with generation-free source unavailable states: `cargo test -p djls-project loading_state` — 2 passed; `cargo test -p djls-project project_source_files_summary` — 1 passed; `cargo test -p djls-db` — 13 passed; `cargo build -q` also passed with `DjangoDatabase`, `djls-bench::Db`, and semantic test database `djls_project::Db` impls. Review follow-up evidence: production `DjangoDatabase` now uses `ProjectLoadingState::not_loaded`, fixture DBs keep `fixture_unavailable`, and `ProjectSourceFiles` derives summary from its merged `SourceFileSet`.

**Phase 3A2b gate**
- [x] Root-construction, first-party discovery file-policy, and first-party apply-seam tests pass in the project crate, including private-constructor enforcement, `ProjectSourceFilesIssue` construction for missing/duplicate/walk cases, partition readiness/status construction through the merge seam, overlapping-root longest-prefix ownership/deduplication, and root removal by `SourceRootId`: `cargo test -p djls-project files` — 13 passed. Conflict detection and lower-precedence resurrection tests land in Phase 6B with the first real non-first-party partitions. Evidence includes constructor-controlled `ProjectSourceFiles`/update internals, canonical root alias deduplication, missing-root fallback identity, first-party request policy tests, issue mapping tests, duplicate root issues flowing through the full first-party update, missing-root readiness projecting as `Unavailable`, incremental `changed_roots` diffing, longest-prefix ownership/deduplication tests, and root-removal patch tests; `cargo test -p djls-workspace file_loader` — 7 passed after removing the production-public workspace test constructor; `cargo test -p djls-project loading_state` — 2 passed; `cargo build -q` also passed. Review follow-up evidence: first-party requests now consume `SourceRootsPlan`, `SourceFilesLoadRequest::new` is not public, `ProjectSourceFiles` is opaque publicly while internally modeled as a discovered/materialized ADT, the public merge seam is explicitly first-party (`FirstPartySourceFilePatch`, `merge_first_party_source_file_patch`), `MergedDiscoveredSourceFileSetData` is ephemeral/internal and not crate-root re-exported, and predicate/options helpers are no longer crate-root API.

**Phase 3A2c gate**
- [x] Database source-file materialization tests preserve `File` handles for unchanged paths, apply `ProjectSourceFilesMaterializationPatch` changed roots/files/deletions where possible, return `SourceFileSetMaterialized`, and stay partition-policy-free: `cargo test -p djls-db source_file_set` — 5 passed after review follow-up. Evidence: `DjangoDatabase::materialize_source_file_set` consumes only the materialization patch, preserves the existing `File` for an unchanged discovered path, returns `SourceFileSetMaterialized` with `SourceFileHandleChanges`, counts removed-root file handles once, and has no access to private partition policy.
- [x] Source-file round-trip coherence tests prove partitions, project-owned merged view, materialization patch, `SourceFileSetMaterialized`, project-owned finalization, preserved unchanged `File` handles, `ProjectLoadingState.source_files = ProjectSourceFilesAvailability::Ready(ReadyProjectSourceFiles)`, and `ProjectSourceFilesApplied.files` remain coherent across an apply and config restart while preserving the applied partition/node transition separately from aggregate source-file readiness: `cargo test -p djls-db source_file_set_roundtrip` — 1 passed as the focused round-trip subset of the `source_file_set` tests. Evidence: `DjangoDatabase::apply_project_source_files` captures one previous ready snapshot, materializes from that snapshot, and calls project-owned finalization with the same previous value; finalization validates the materialized `SourceFileSet` against the update partitions before publishing `Ready`.
- [x] Source-file terminal transition tests prove activity/materialization/apply failures do not leave `ProjectLoadingState.source_files` stuck in `Loading`/`Stale`; they produce query-visible `Deferred`, `Unavailable`, or `Failed` availability with typed issues and previous ready files when available. Evidence: `source_file_set_terminal_issue_updates_query_visible_availability` and `terminal_issue_preserves_previous_ready_source_files` pass under `cargo test -p djls-db source_file_set`, and missing-root apply finalization writes `ProjectSourceFilesAvailability::Unavailable { issue, previous }`.
- [x] Salsa invalidation tests prove `ProjectLoadingState.source_files` transitions from `Loading`/`Unavailable` to `Ready` invalidate a minimal tracked probe query: `cargo test -p djls-project loading_state_invalidation` — 1 passed. Cleanup evidence: `rg "set_source_files\\(" crates -g '*.rs'` has one production hit, the sealed `set_project_source_files_availability` helper in `crates/djls-project/src/db.rs`.

**Phase 3A3 gate**
- [x] Neutral runner/plan tests pass for the `source-file-set` node, including the concrete one-node path, the `NODE_SPECS` row, terminal-status projection through `node_status_from_readiness(ProjectSourceFilesApplied)`, projection-table coverage for source-file readiness classes, and observer event emission with in-process fake execution/apply effects and a recording observer: `cargo test -p djls-project loading` — 23 passed after review follow-up. Evidence: `loading::plan` defines the Phase 3 `source-file-set` `NODE_SPECS` row and projection API; projection-table tests cover source-file readiness classes and applied-result wrapping; `loading::driver` runs reset/activity/apply/status projection and emits recording-observer events in the one-node fake-effects test; loading-state tests cover `begin_project_loading_run` no-previous and ready-to-stale transitions.
- [x] Phase 3 CLI effect adapter in `crates/djls` runs the active Phase 3 loading plan through `run_loading_plan`: `cargo test -p djls --test check` — 7 passed. Evidence: `CliLoadingExecutor` implements reset, first-party source-file activity, and direct apply through `DjangoDatabase::apply_project_source_files`; `djls check` invokes `run_loading_plan(LoadingPlan::phase3(), ...)` for no-explicit-path runs while targeted path checks avoid the extra project-wide loading walk until source-file facts feed check behavior.

**Phase 3A4a gate**
- [x] Generation guard tests cover immutable `StartupRunInputs` capture, versioned captured document snapshots for opened unsaved/changed files, stale-document rejection after `didChange`/`didClose` during blocked loading, guarded reset, `ApplyOutcome::Superseded`, `ApplyOutcome::Rejected { reason: ApplyRejection::StaleDocument { ... } }`, guarded observation/reporting, and superseded propagation before node work starts: `cargo test -p djls-server startup_generation` — 10 passed after review follow-up. Evidence: `crates/djls-server/src/startup.rs` defines server-local `ProjectLoadingSnapshot`, `StartupRunInputs`, `StartupGeneration`, `GenerationGuard`, `ApplyOutcome<T>`, `ApplyRejection::StaleDocument { file, path, captured, current }`, and `ObservationOutcome<T>`; tests cover immutable capture, changed/closed stale-document evidence, close/reopen with the same document version, guarded reset, no active generation before first start, default generation initialization, superseded apply before session locking, superseded observation, and serialized generation supersession vs guarded apply.

**Phase 3A4b gate**
- [x] LSP effect-adapter tests pass for the `source-file-set` node through `run_loading_plan`, with guarded apply and no progress assertions: `cargo test -p djls-server startup_source_files` — 3 passed after review follow-up. Evidence: `LspLoadingExecutor` implements `LoadingEffects`, runs `LoadingPlan::phase3()` through `run_loading_plan`, applies reset/source-file update through `GenerationGuard`, updates query-visible `ProjectLoadingState.source_files` on the live session database, stops before source-file loading when reset is superseded, and writes terminal failed source-file availability for stale-document rejection before returning a rejected apply outcome.
- [x] Request-while-loading test proves a blocked active `source-file-set` startup run does not block a representative request and returns a valid degraded response: `cargo test -p djls-server startup_request_while_loading` — 1 passed. Evidence: `startup_request_while_loading_does_not_wait_for_source_file_node` blocks source-file loading before apply, collects diagnostics through the shared session while the node is blocked, then unblocks the run and observes coherent completion.

**Phase 3A4c gate**
- [x] Client work-done progress capability tests pass: `cargo test -p djls-server client::tests::work_done_progress` — 3 passed in the progress lifecycle slice; `cargo test -p djls-server work_done_progress` — 5 passed after review follow-up.
- [x] Progress lifecycle tests cover begin/report/finish and log fallback over stable observer events: `cargo test -p djls-server startup_progress` — 3 passed after review follow-up.

**Architecture correction gate**
- [x] Current and future phase prose is updated for the stable Project-root decision. Evidence: the current cleanup gate, Phase 3A4d, Phase 3B, Phase 3C, Phase 3D, and later source/enrichment references now target `Project` fields and server/CLI orchestration instead of extending `ProjectLoadingState`; completed 3A1-3A4c references remain as superseded implementation history.
- [x] Stable `djls_project::Project` root compiles in production, bench, and semantic test databases: `cargo test -p djls-db --no-run`, `cargo test -p djls-bench --no-run`, and `cargo test -p djls-semantic --no-run` passed.
- [x] Source-file materialization/round-trip tests pass against the new project source inventory: `cargo test -p djls-db source_file_set` — 5 passed.
- [x] Neutral loading runner tests pass without depending on `ProjectLoadingState`: `cargo test -p djls-project loading` — 21 passed.
- [x] LSP startup source-file tests pass and stale-document rejection leaves Project Facts unchanged: `cargo test -p djls-server startup_source_files` — 3 passed. Evidence: stale-document rejection returns the executor failure path while `Project.source_inventory` remains unchanged.
- [x] Request-while-loading behavior remains valid: `cargo test -p djls-server startup_request_while_loading` — 1 passed.
- [x] CLI check still exercises the shared source-file node: `cargo test -p djls --test check` — 7 passed.
- [x] Cleanup search proves no production code consumes `Db::project_loading_state()` or `ProjectLoadingState`: `rg "project_loading_state|ProjectLoadingState" crates -g '*.rs'` returned no matches.
- [x] Formatting and build checks pass: `just fmt --check` and `cargo build -q` passed.

**Phase 3A4d gate**
- [x] Configuration-change restart tests pass for the active loading graph and superseded apply rejection: `cargo test -p djls-server configuration_restart` — 1 passed. Evidence: `initialized` and env-changing configuration reloads start the source-file loading graph through `StartupController`; superseded applies leave prior Project Facts unchanged, and generation supersession is marked active before waiting behind apply linearization.

**Phase 3B gate**
- [x] Phase 3B has been rewritten against the stable `Project` root before implementation starts. Evidence: the Phase 3B section now depends on the Architecture correction gate, defines `Project.discovery`/`Project.enrichment` domain facts, and forbids extending `ProjectLoadingState`.
- [x] Discovery/enrichment project-root scaffolding compiles without adding a new readiness singleton: `cargo test -p djls-project discovery` — 10 passed. Evidence: `Project.discovery` / `Project.enrichment` are stable Project fields, `ProjectDiscoverySet` is root-scoped and non-empty, discovery/enrichment unavailable states require non-empty typed issues, and `ProjectEnvVars` rejects duplicate keys before canonicalization.

**Phase 3C1 gate**
- [x] `djls-conf` structured root settings load outcome tests preserve root, source path, typed error category, and fallback marker: `cargo test -p djls-conf root_settings_load` — 6 passed. Evidence: tests cover missing config without fallback issue, effective `djls.toml` source path, unrelated `pyproject.toml` not masking `djls.toml`, invalid `pyproject.toml` / `djls.toml` parse issues with source paths, and client overrides on successful root config without fallback-after-error.

**Phase 3C2 gate**
- [x] Discovery data/helper tests preserve config-load failures/fallback provenance, distinguish missing config fallback from invalid config, lower `djls-conf` DTOs into project-owned environment seeds, treat interpreter/module-search facts and resolved settings as core root-scoped Project Facts once semantics depend on them, apply env precedence before constructing canonical `ProjectEnvVars`, and canonicalize `ProjectEnvVars`: `cargo test -p djls-project loading_settings` — 5 passed. Evidence: tests cover config failure/fallback provenance, configured environment seed lowering, pythonpath lowering, env-file failure issues, duplicate env-var issues, and canonical env-var ordering.
- [x] Discovery apply tests update stable `Project.discovery` facts through setters, preserve old facts on failed reload, and invalidate discovery-dependent tracked queries when discovery facts change: `cargo test -p djls-project discovery_invalidation` — 1 passed; `cargo test -p djls-db project_discovery` — 2 passed. Evidence: project-crate tracked probe invalidates on discovery change, database apply materializes `RootDiscoveryInput` handles and sets `Project.discovery`, repeated identical data avoids fresh setter invalidation by comparing plain data first, and empty failed discovery data preserves prior facts.

**Phase 3C3 gate**
- [x] Two-node runner tests prove `source-file-set -> project-discovery-set` ordering, `NODE_SPECS` coverage, domain-outcome-to-terminal projection table behavior, successor execution, and observer events without registry/plugin machinery: `cargo test -p djls-project loading`
- [x] Project-discovery loading-node tests pass through both CLI and LSP effect adapters, including configuration-change restart and superseded apply rejection: `cargo test -p djls-server startup` and `cargo test -p djls --test check`

**Phase 3C4 gate**
- [x] Pure availability ownership has moved to `djls-project::availability`; the temporary Phase 1 semantic availability type/module is deleted.
- [x] An executable cleanup assertion proves the temporary semantic availability bridge is gone or narrowed: `rg "ProjectFactsAvailability|degraded_no_project|availability" crates/djls-semantic crates/djls-ide crates/djls-server -g '*.rs'` shows only the final server/session use of the project-owned seam plus unrelated scoping availability comments/tests.
- [x] No-discovery-set degraded request tests, absent/unavailable source/discovery availability tests, and the shared project availability matrix pass: `cargo test -p djls-project availability` and `cargo test -p djls-server degraded`.

**Phase 3D gate**
- [x] Layout index tests pass, including domain outcome variants for absent/unavailable source inventory, invalidation through stable `Project.source_inventory`, and no recomputation for enrichment-only state changes: `cargo test -p djls-project layout`.
- [x] Layout consumer tests prove absent/unavailable layout does not look like "no conventional candidates": `settings_candidates_do_not_treat_unavailable_layout_as_empty` under `cargo test -p djls-project layout`.
- [x] Queue cleanup/removal tests or compile checks pass: `cargo test -p djls-server queue` and `cargo test -p djls-server startup`.
- [x] Queue/cache startup bridge cleanup search passes with no remaining matches: `rg "Queue|enqueue|refresh_external_data|load_template_library_cache" crates/djls-server -g '*.rs'`.
- [x] Benchmark database compiles: `cargo test -p djls-bench --no-run`.
- [x] Workspace builds: `cargo build -q`.

#### Manual Verification
- [x] Confirm `djls-project` has no dependency on `djls-semantic`, `djls-server`, `djls-db`, or `djls-ide`. Evidence: `rg "djls-semantic|djls-server|djls-db|djls-ide" crates/djls-project/Cargo.toml` returned no matches.
- [x] Confirm `project_layout_index` returns a domain outcome, uses stable `Project.source_inventory` / `SourceFileSet`, and does not call `std::fs`, `walk_files`, settings/environment queries, or legacy semantic `Project` fields. Evidence: `rg "std::fs|walk_files|settings_module_candidates|Project::|djls_semantic|project\(\)" crates/djls-project/src/layout.rs` shows only the intended layout consumer and local test setup/calls.
- [x] Confirm low-level `djls_source::Db::source_file_set()` consumers do not use `None`/`Some` as startup readiness. Evidence: `rg "source_file_set\(" crates -g '*.rs'` shows only loading effect names, the source-file-set wrapper accessor, database materialization tests, and CLI/LSP loading adapters; project-facing layout reads stable Project source-inventory facts with an explicit `Project` handle.
- [x] Confirm no `Fact<T>` appears under `crates/djls-project`. Evidence: `rg "Fact<" crates/djls-project -g '*.rs'` returned no matches.
- [x] Confirm `crates/djls-project/src/loading/plan.rs`, the neutral loading driver, the execution/apply contract, and the observer/event-sink contract do not import LSP/server/CLI/database concrete types or activity modules. Evidence: `rg "tower_lsp_server|ls_types|DjangoDatabase|Session|CliLoadingExecutor|ProjectLoadingSnapshot|StartupRunInputs|Arc<Mutex<Session>>" crates/djls-project/src/loading crates/djls-project/src/layout.rs -g '*.rs'` returned no matches.
- [x] Confirm `ProjectLoadingSnapshot` and `StartupRunInputs` are server-local, and `djls-project` activity code receives only node-specific request structs or plain domain values, never `ProjectLoadingSnapshot` or `Arc<Mutex<Session>>`. Evidence: same concrete-type search returned no matches in `djls-project`.
- [x] Confirm CLI and LSP effect adapters use the same per-node request builders; adapters may differ in capture/apply/reporting but not request-construction policy. Evidence: both `crates/djls/src/loading.rs` and `crates/djls-server/src/startup.rs` use `build_source_roots`, `first_party_source_files_load_request`, `build_project_discovery_data`, and `ProjectDiscoveryLoadRequest`.
- [x] Confirm the concrete `CliLoadingExecutor` lives in `crates/djls`, not `crates/djls-project`.
- [x] Confirm `crates/djls-project/src/loading/*` activity modules return typed outcomes only and do not emit progress, check startup generations, or advance milestones; Phase 3 must not define milestone IDs or milestone advancement. Evidence: `rg "milestone|progress|generation" crates/djls-project/src/loading -g '*.rs'` shows only the generation-free source-inventory fixture test name.
- [x] Confirm `djls-source` still has no readiness availability or partition precedence policy, and `djls-db` apply code does not know Django partition names. Evidence: `rg "availability|FileSetPartition" crates/djls-source crates/djls-db/src/db.rs -g '*.rs'` shows no `djls-source` readiness availability and only project source-file apply/materialization types in `djls-db`.

## Phase 4: Python source model and settings candidates

### Overview
Extract local Python source models through a Ruff AST anti-corruption layer, move name newtypes into `djls-project`, and derive settings candidates from explicit config, environment, `manage.py`, and conventional settings modules.

Future `djls-project` tracked query roots introduced from this point should take an explicit `project: Project` argument when practical. Transitional request-boundary helpers may read `db.project()`, but new deep project queries should not hide project identity.

Implement Phase 4 as hard-stop subphases:
1. **Phase 4A: name/type move** — move project-domain name newtypes and prevent `TemplateName` identity confusion.
2. **Phase 4B: Ruff anti-corruption and tracked source-model queries** — add Ruff dependencies, DJLS-native source-model types, and tracked queries.
3. **Phase 4C: `python-source-models` readiness observation node** — wire the live query observation through the neutral runner plus CLI/LSP adapters.
4. **Phase 4D: settings candidates, provenance, and fixtures** — add settings candidate discovery and supporting test helpers.

Do not begin the next subphase until the current subphase's targeted checks pass.

### Changes Required

#### Phase 4A: Move domain name newtypes to `djls-project`
**Files**:
- `crates/djls-project/src/names.rs`
- `crates/djls-semantic/src/project/names.rs`
- `crates/djls-semantic/src/project.rs`
- `crates/djls-semantic/src/lib.rs`

**Edits**:
- Move `LibraryName`, `PyModuleName`, `TemplateSymbolName`, and `InvalidName` into `djls-project`.
- Add a non-interned `TemplateName` newtype in `djls-project`.
- Keep private fields and parse constructors.
- Keep temporary semantic re-exports so old code compiles until Phase 10.
- Rename the current interned semantic `TemplateName` in `crates/djls-semantic/src/primitives.rs` to `InternedTemplateName` unconditionally in the same `jj` change that introduces `djls_project::TemplateName`. This prevents accidental use of the interned identity as the domain type and avoids a conflict window where both crates export `TemplateName`.
- Add a phase-local cleanup search for migrated names: run `rg "TemplateName|LibraryName|PyModuleName|TemplateSymbolName" crates/djls-semantic crates/djls-project -g '*.rs'` and confirm remaining semantic references are intentional re-exports or interned semantic identities.

#### Phase 4B: Add Ruff dependencies to `djls-project`
**File**: `crates/djls-project/Cargo.toml`

**Edits**:
- Add `ruff_python_ast = { workspace = true }` and `ruff_python_parser = { workspace = true }`.
- Keep third-party dependencies alphabetized.

#### Phase 4B: Add Python source model anti-corruption types
**File**: `crates/djls-project/src/python/source.rs`

**Edits**:
- Add `PythonSourceModel`, `PythonSourceIndex`, `PythonSourceIndexOutcome`, `PythonSourceIndexIssue`, `PyModuleNameResolution`, `ModuleNameIssue`, `QualifiedName`, `ImportStatement`, `AssignmentTarget`, `Assignment`, `CallExpression`, `ClassDef`, `FunctionDef`, `StaticValueSegment<T>`, `StaticValue`, and `StaticValueIssue`.
- Translate Ruff AST into these DJLS-native types at the extraction boundary.
- Do not store Ruff AST nodes in returned project-model structs.
- Use `ruff_python_parser::parse_module(source).into_syntax()` for parsing.
- Unknown/unsupported expressions become `StaticValue::Unknown { issue }` or unknown segments, not generic strings.

**Code shape**:
```rust
pub struct PythonSourceModel {
    file: File,
    module: PyModuleNameResolution,
    imports: Vec<ImportStatement>,
    assignments: Vec<Assignment>,
    calls: Vec<CallExpression>,
    class_defs: Vec<ClassDef>,
    function_defs: Vec<FunctionDef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StaticValue {
    String(String),
    StringList(Vec<StaticValueSegment<String>>),
    Dict(Vec<(String, StaticValue)>),
    Unknown { issue: StaticValueIssue },
}

pub enum PythonSourceIndexOutcome {
    Ready(PythonSourceIndex),
    Skipped { issue: PythonSourceIndexIssue },
    Unavailable { issue: PythonSourceIndexIssue },
    Deferred { issue: PythonSourceIndexIssue },
}
```

#### Phase 4B: Add tracked source-model queries
**Files**:
- `crates/djls-project/src/python.rs`
- `crates/djls-project/src/python/source.rs`

**Edits**:
- Add `#[salsa::tracked(returns(ref))] pub fn python_source_model(db: &dyn Db, file: File) -> PythonSourceModel`.
- Add `#[salsa::tracked(returns(ref))] pub fn python_source_index(db: &dyn Db, project: Project) -> PythonSourceIndexOutcome` over local Python files in `Project.source_inventory`.
- `PythonSourceIndexOutcome` must be readiness-bearing: include variants such as `Ready(PythonSourceIndex)`, `Skipped { issue }`, `Unavailable { issue }`, and `Deferred { issue }`. The `python-source-models` node terminal status must be derived only through `node_status_from_readiness` from this outcome, not from executor-local observation.
- Use `File::source(db)` so captured/open-buffer text participates in source text through Salsa `File` state, not through live LSP buffer handles.
- Module-name resolution in this phase should be local/syntactic: derive from import roots known in the layout where possible, otherwise return `OutsideImportRoots` or `Ambiguous` issues. Full module lookup comes in Phase 5.

#### Phase 4C: Probe Python source-model readiness from startup
**Files**:
- `crates/djls-project/src/loading/plan.rs`
- `crates/djls-project/src/loading/driver.rs`
- `crates/djls-server/src/startup.rs`
- `crates/djls/src/loading.rs`
- `crates/djls/src/commands/check.rs`

**Edits**:
- Start with a hard nonblocking live-query access gate before adding `python-source-models`. Define the concrete API shape, for example `Session::with_live_project_db_for_observation(...)` or a narrow live database read handle, that lets the LSP executor observe `python_source_index(db, project)` on the live project database without holding `Arc<Mutex<Session>>` across Python parsing or long tracked-query execution. This gate must also implement the guarded observation/report transition from the Executor transition policy: check generation before observation, after observation, and before node events/progress/milestone advancement. If the chosen shape cannot avoid request blocking or cannot guard observation reporting, stop and revise the readiness-observation requirement before adding this node.
- Introduce the `python-source-models` loading graph node in this phase. Do not predeclare task nodes outside the phase that introduces their real activity.
- Start the node after `source-file-set` and `ProjectDiscoverySet` are available.
- Implement a readiness-observation activity service that takes `PythonSourceProbeRequest`, calls `python_source_index(db, project)` on the live project database under the named nonblocking observation boundary, and returns the typed outcome; the neutral loading driver runs it through the shared plan, and both LSP and CLI effect adapters record/report the outcome through their executor-specific channels in this phase.
- Mark `python-source-models` as `Running`, evaluate `python_source_index(db, project)` and cheap dependencies from `PythonSourceProbeRequest` on the live database, then project terminal status from `PythonSourceIndexOutcome` through `node_status_from_readiness`. Clone-only evaluation may be used for report-only diagnostics or speculative measurement, but it must not mark this node ready or advance `workspace-ready`.
- If there are no loaded first-party Python files or source text is unavailable, return `PythonSourceIndexOutcome::Skipped` or `Unavailable` with a typed issue and project the node terminal status from that outcome through `node_status_from_readiness`. Do not leave the node `Pending` indefinitely.
- Apply no broad blob; tracked source-model queries remain the source of truth. Because node readiness comes from the live tracked query, add measurement/test coverage that the first request after `python-source-models` reports ready does not recompute the same index work.
- Add a request-while-running test for this node: block live Python-source observation, issue a representative request, and prove it does not wait on the session lock or on Python parsing.
- This node becomes one prerequisite for `workspace-ready`; the milestone itself is registered in the loading graph once environment discovery also exists.

#### Phase 4D: Add settings candidate discovery
**Files**:
- `crates/djls-project/src/settings/candidates.rs`
- `crates/djls-project/src/provenance.rs`

**Edits**:
- Add `SettingsCandidate` and `SettingsCandidateSource`.
- Sources are limited to:
  - `ExplicitConfig`
  - `EnvironmentVariable`
  - `ManagePyDefault`
  - `ConventionalModule`
- Parse `os.environ.setdefault("DJANGO_SETTINGS_MODULE", "...")` from `manage.py` using `PythonSourceModel` calls.
- Conventional candidates should include importable modules such as `settings`, `config.settings`, and `<package>.settings` found through `ProjectLayoutIndexOutcome::Ready`. If layout is deferred, unavailable, or stale, return a typed settings-candidate outcome/issue instead of treating conventional candidates as absent.
- Return multiple candidates. Do not choose a default.
- Add provenance/origin data for every candidate.
- If Phase 3D deferred provenance because there was no concrete consumer, introduce the concrete `OriginSet` / `Provenance` support here as part of the first consumer. If Phase 3D already introduced it, reuse that module. Do not introduce `ProjectFactIssue` unless a concrete settings-candidate issue consumes it.

**Code shape**:
```rust
pub struct SettingsCandidate {
    module: PyModuleName,
    file: Option<File>,
    source: SettingsCandidateSource,
    origin: OriginSet,
}

pub enum SettingsCandidateSource {
    ExplicitConfig,
    EnvironmentVariable,
    ManagePyDefault,
    ConventionalModule,
}
```

#### Phase 4D: Add testing helpers
**File**: `crates/djls-project/src/testing.rs`

**Edits**:
- Add fixture helpers that create in-memory source files, a `SourceFileSet`, and a `ProjectDiscoverySet` without Python or Django installed.
- Include helpers for adding `manage.py`, settings files, packages, templates, and app directories.

### Success Criteria

#### Automated Verification
**Phase 4A gate**
- [x] Name newtype tests pass: `cargo test -p djls-project names`.
- [x] Semantic crate still compiles with moved name re-exports: `cargo test -p djls-semantic --no-run`.
- [x] Name re-export cleanup search passes with only intentional temporary re-exports or interned semantic identities: `rg "TemplateName|LibraryName|PyModuleName|TemplateSymbolName" crates/djls-semantic crates/djls-project -g '*.rs'`.

**Phase 4B gate**
- [x] Ruff dependency boundary compiles in `djls-project`: `cargo test -p djls-project python_source_model --no-run`.
- [x] Python source model tests pass: `cargo test -p djls-project python_source_model` and `cargo test -p djls-project python_source_index`.

**Phase 4C gate**
- [x] Nonblocking live-query access seam is named and tested: observing `python_source_index(db, project)` on the live database does not hold `Arc<Mutex<Session>>` across Python parsing or long tracked-query execution, and generation is checked before observation, after observation, and before node events/progress/milestone advancement. Evidence: `Session::project_db_snapshot_for_observation`, guarded LSP observation in `LspLoadingExecutor::observe_python_source_index`, and `cargo test -p djls-server python_source_models` passed.
- [x] Python source-model loading-node tests pass through the neutral runner/shared plan and both real effect adapters, proving terminal status is projected by `node_status_from_readiness(PythonSourceIndexOutcome)` observed on the live database: `cargo test -p djls-project loading_python_source_models`, `cargo test -p djls-server python_source_models`, and `cargo test -p djls --test check` passed.
- [x] Request-while-running test proves a blocked `python-source-models` observation does not block representative requests: `cargo test -p djls-server python_source_models` passed, including `python_source_models_request_while_running_does_not_wait`.
- [x] Live-readiness reuse test or query counter proves the first post-ready request does not recompute `python_source_index(db, project)` after `python-source-models` reported ready: `cargo test -p djls-project python_source_index_reuse` passed.

**Phase 4D gate**
- [x] Settings candidate tests pass: `cargo test -p djls-project settings_candidates` passed.
- [x] Testing helpers compile with source files, `SourceFileSet`, and `ProjectDiscoverySet` fixtures: `cargo test -p djls-project testing` passed.
- [x] Workspace builds: `cargo build -q` passed.

#### Manual Verification
- [ ] Confirm no Ruff AST types appear in public `djls-project` source-model return structs.
- [ ] Confirm an opened unsaved or changed Python file participates in `python_source_model` through Salsa-visible `File::source(db)` state during loading, not through live LSP overlay handles, and that stale captured document versions cannot report ready facts.
- [ ] Confirm unsupported Python expressions are represented with typed `StaticValueIssue` variants.
- [ ] Confirm a fixture with two settings files yields two `SettingsCandidate` values.

## Phase 5: module resolution and Django Environment candidates

### Overview
Add lightweight module resolution over loaded files, preserve all Django Environment candidates, and expose late file-scoped environment selection.

Implement Phase 5 as hard-stop subphases:
1. **Phase 5A: resolver/import roots** — add import roots and module resolution over loaded files.
2. **Phase 5B: environment candidates and file selection** — add all Django Environment candidates and file-scoped selection.
3. **Phase 5C: `environment-discovery` readiness observation node** — wire the live query observation through the neutral runner plus CLI/LSP adapters.
4. **Phase 5D: `workspace-ready` milestone and semantic trait cleanup** — add milestone policy and update semantic trait inheritance.

Do not begin the next subphase until the current subphase's targeted checks pass.

### Changes Required

#### Phase 5A: Add import roots and module resolution
**File**: `crates/djls-project/src/resolver.rs`

**Edits**:
- Add `ImportRoot`, `ImportRootKind`, `ResolvedModule`, `ModuleLocation`, `ModuleResolution`, `ModuleResolutionOutcome`, and `ModuleResolutionIssue`.
- `import_roots(db, project)` should use `Project.discovery`, source roots, `src/` convention, configured `pythonpath`, and interpreter/site-packages hints.
- `resolve_module(db, project, requested)` should resolve only through loaded file-set entries in `Project.source_inventory`. For known roots not loaded into the source inventory, return `Deferred { RootUnavailable { root } }`.
- Recognize both `pkg/module.py` and `pkg/module/__init__.py`.
- Return `Ambiguous` when multiple loaded roots resolve the same module.

**Code shape**:
```rust
pub enum ModuleResolutionOutcome {
    Resolved(ResolvedModule),
    NotFound { issues: NonEmpty<ModuleResolutionIssue> },
    Ambiguous { candidates: AtLeastTwo<ResolvedModule>, issues: NonEmpty<ModuleResolutionIssue> },
    Deferred { issue: ModuleResolutionIssue },
}

pub enum ModuleResolutionIssue {
    NoImportRoots,
    RootUnavailable { root: Utf8PathBuf },
    NotFound,
    MultipleCandidates,
    UnsupportedModuleName,
}
```

#### Phase 5B: Add Django Environment candidates and selection
**File**: `crates/djls-project/src/environments.rs`

**Edits**:
- Add `DjangoEnvironmentId`, `DjangoEnvironmentCandidate`, `EnvironmentCandidateSource`, `DjangoEnvironmentCandidatesOutcome`, `EnvironmentCandidatesIssue`, `EnvironmentSelection`, and `EnvironmentSelectionIssue`.
- `django_environment_candidates(db, project) -> DjangoEnvironmentCandidatesOutcome` should combine:
  - explicit `[[django_environments]]` config
  - explicit `django_settings_module`
  - environment variable settings candidates
  - `manage.py` defaults
  - conventional settings candidates
- Every settings candidate may become an environment candidate. Do not rank one as globally selected.
- `DjangoEnvironmentCandidatesOutcome` must be readiness-bearing: include variants such as `Ready(Vec<DjangoEnvironmentCandidate>)`, `Ambiguous { candidates, issues }`, `Unavailable { issue }`, and `Deferred { issue }`.
- `environment_for_file(db, project, file)` selects by candidate root prefix from the readiness-bearing candidate outcome.
- If multiple candidates match equally, return `EnvironmentSelection::Ambiguous`.
- If none match, return `EnvironmentSelection::Unknown`.

**Code shape**:
```rust
pub enum DjangoEnvironmentCandidatesOutcome {
    Ready(NonEmpty<DjangoEnvironmentCandidate>),
    Ambiguous {
        candidates: AtLeastTwo<DjangoEnvironmentCandidate>,
        issues: NonEmpty<EnvironmentCandidatesIssue>,
    },
    Unavailable { issue: EnvironmentCandidatesIssue },
    Deferred { issue: EnvironmentCandidatesIssue },
}

pub enum EnvironmentSelection {
    Selected(DjangoEnvironmentId),
    Ambiguous {
        candidates: AtLeastTwo<DjangoEnvironmentCandidate>,
        issues: NonEmpty<EnvironmentSelectionIssue>,
    },
    Unknown {
        issues: NonEmpty<EnvironmentSelectionIssue>,
    },
}
```

#### Phase 5C: Probe environment discovery readiness from startup
**Files**:
- `crates/djls-project/src/loading/plan.rs`
- `crates/djls-project/src/loading/driver.rs`
- `crates/djls-server/src/startup.rs`
- `crates/djls/src/loading.rs`
- `crates/djls/src/commands/check.rs`

**Edits**:
- Reuse the nonblocking live-query access seam from Phase 4C. If environment candidate derivation needs a different access pattern, add a stop gate before this node and prove it does not hold `Arc<Mutex<Session>>` across long tracked-query execution.
- Introduce the `environment-discovery` loading graph node after `source-file-set`, `project-discovery-set`, and `python-source-models` are available.
- Implement environment discovery readiness observation as a task activity service that takes `EnvironmentDiscoveryProbeRequest`, calls `django_environment_candidates(db, project)` on the live project database under the nonblocking live-query access seam, and evaluates selected cheap query dependencies from that request. Clone-only evaluation may be used for report-only diagnostics or speculative measurement, but it must not mark this node ready or advance `workspace-ready`. It must run through the neutral loading driver/shared plan and both CLI/LSP effect adapters in this phase, not a server-only scheduler.
- Apply no broad blob; tracked queries are the source of truth. Because node readiness comes from the live tracked query, add measurement/test coverage that the first request after `environment-discovery` reports ready does not recompute the same candidate derivation work.
- Add a request-while-running test for this node: block live environment discovery observation, issue a representative request, and prove it does not wait on the session lock or on candidate derivation.
- Deduplicate project-level ambiguity warnings by `(generation, candidate set)`.
- Report ambiguity through logs/progress, not per-file diagnostics.
- Mark `environment-discovery` with the appropriate terminal status projected only through `node_status_from_readiness` from live `DjangoEnvironmentCandidatesOutcome` and related `EnvironmentSelection` outcomes, not from executor-local status.

#### Phase 5D: Add `workspace-ready` milestone policy
**Files**:
- `crates/djls-project/src/loading/plan.rs`
- `crates/djls-project/src/loading/driver.rs`
- `crates/djls-server/src/startup.rs`
- `crates/djls/src/loading.rs`

**Edits**:
- Extend `LoadingPlan` in this subphase with milestone IDs, milestone prerequisites, acceptable readiness projections, and node-to-milestone advancement; Phase 3 deliberately did not add milestone APIs.
- Introduce the `workspace-ready` loading milestone ID and register it in the loading graph in this phase with prerequisites on `source-file-set`, `python-source-models`, and `environment-discovery`, using acceptable `NodeTerminalStatus` values produced by the loading-node table and `node_status_from_readiness`. Milestone advancement remains `LoadingPlan` behavior, not inline startup prose, and must not maintain parallel readiness state.

#### Phase 5D: Update semantic trait inheritance for new consumers
**File**: `crates/djls-semantic/src/db.rs`

**Edits**:
- Change `pub trait Db: ProjectDb` toward `pub trait Db: djls_project::Db` for new project-model consumers.
- Keep old `ProjectDb` methods only as long as old consumers still require them.
- Update `DjangoDatabase`, `djls-bench::Db`, and semantic test DB impls as required.

### Success Criteria

#### Automated Verification
**Phase 5A gate**
- [x] Import-root and module-resolution tests pass: `cargo test -p djls-project resolver` passed.

**Phase 5B gate**
- [x] Environment candidate tests pass: `cargo test -p djls-project environments` passed.
- [x] Multisite fixture test passes: `cargo test -p djls-project multisite` passed.

**Phase 5C gate**
- [x] Nonblocking live-query access seam covers environment candidate derivation without holding `Arc<Mutex<Session>>` across long tracked-query execution. Evidence: LSP `observe_django_environment_candidates` clones the project DB under the session lock, runs `django_environment_candidates` after releasing it, and `cargo test -p djls-server startup` passed.
- [x] Environment-discovery node tests pass through the neutral runner/shared plan and both real effect adapters, proving terminal status is projected by `node_status_from_readiness(DjangoEnvironmentCandidatesOutcome)` observed on the live database: `cargo test -p djls-project loading_environment_discovery`, `cargo test -p djls-server startup`, and `cargo test -p djls --test check` passed.
- [x] Request-while-running test proves a blocked `environment-discovery` observation does not block representative requests: `cargo test -p djls-server startup` passed, including `environment_discovery_request_while_running_does_not_wait`.
- [x] Live-readiness reuse test or query counter proves the first post-ready request does not recompute `django_environment_candidates(db, project)` after `environment-discovery` reported ready: `cargo test -p djls-project environment_candidates_reuse` passed.

**Phase 5D gate**
- [x] LoadingPlan milestone policy tests pass for `workspace-ready`: `cargo test -p djls-project loading_plan` passed.
- [x] Semantic trait impls compile: `cargo test -p djls-semantic --no-run` passed.
- [x] Workspace builds: `cargo build -q` passed.

#### Manual Verification
- [ ] Confirm a multisite fixture yields distinct `DjangoEnvironmentCandidate` values.
- [ ] Confirm no code path picks a single global Django Settings Module during startup.
- [ ] Confirm ambiguity warnings are deduped and not emitted as template diagnostics.

## Phase 6: effective settings, installed apps, and static template inventory

### Overview
Follow static settings composition far enough to discover known installed apps, bounded installed-app files, Template Directories, Templates, and Template Tag Libraries without runtime introspection.

Implement Phase 6 as hard-stop subphases:
1. **Phase 6A: effective settings and installed-app projection queries** — add static settings composition and known installed-app projection without new loading nodes.
2. **Phase 6B: installed-app and configured-template file loading** — add the two file-loading nodes and run them through the Phase 3A2 source-file merge seam.
3. **Phase 6C: static template inventory** — add template directory, template file, and Template Tag Library inventories over the loaded file set.
4. **Phase 6D: first semantic consumer and `django-apps-ready` milestone** — migrate one lookup path and register the milestone now that both file-loading nodes exist.

Do not begin the next subphase until the current subphase's targeted checks pass.

### Changes Required

#### Phase 6A: Effective settings and installed-app projection queries
**Files**:
- `crates/djls-project/src/settings/composition.rs`
- `crates/djls-project/src/apps.rs`

**Edits**:
- Depends on **Phase 5 environment candidates and `workspace-ready` milestone support**.
- Add `EffectiveSettings`, `PartialList<T>`, `PartialListSegment<T>`, `SettingsIssue`, `TemplateBackend`, and `TemplateSettingsResolution`.
- Implement first supported settings patterns:
  - direct uppercase assignments, especially `INSTALLED_APPS` and `TEMPLATES`
  - simple list concat, append, extend, and `+=`
  - direct imports and star imports from local settings modules
  - source-order behavior where imports apply before current-file overrides/appends
- Preserve unknown list segments with provenance.
- Add `#[salsa::tracked(returns(ref))] pub fn effective_settings(db: &dyn Db, project: Project, env: DjangoEnvironmentId) -> EffectiveSettings`.
- Add `InstalledApp`, `AppConfig`, `InstalledAppResolution`, and `InstalledAppIssue`.
- Use known `INSTALLED_APPS` entries only. Do not guess app-like packages from the filesystem.
- Resolve app package modules through `resolve_module` and already loaded/configured facts.
- Avoid an installed-app bootstrapping cycle: the first projection pass may derive package roots from config, module resolution, and currently loaded files only. If `apps.py` or `AppConfig` details require not-yet-loaded installed-app files, return a typed deferred `InstalledAppResolution` rather than blocking file loading or guessing.
- Support `apps.py`/AppConfig enough to resolve `name`, `label`, and `path` when statically obvious from already loaded files.
- Preserve missing, ambiguous, and deferred app entries as typed resolutions.
- Do not add `effective-settings` or `installed-app-projection` loading graph nodes in this subphase. These are tracked query reads used by later loading activities.
- Do not evaluate arbitrary expressions.

**Tests and stop gate**:
- Stop after effective settings tests pass: `cargo test -p djls-project effective_settings`.
- Stop after installed-app projection tests pass, including deferred AppConfig details for not-yet-loaded app files: `cargo test -p djls-project installed_apps`.
- Stop after known `INSTALLED_APPS` order is preserved with unknown gaps.

#### Phase 6B: Installed-app and configured-template file loading through the merge seam
**Files**:
- `crates/djls-project/src/apps.rs`
- `crates/djls-project/src/templates/loading.rs`
- `crates/djls-project/src/loading/files.rs`
- `crates/djls-server/src/startup.rs`
- `crates/djls/src/loading.rs`
- `crates/djls/src/commands/check.rs`
- `crates/djls-db/src/db.rs`

**Edits**:
- Depends on **6A effective settings/installed-app projection** and the existing real graph nodes `source-file-set`, `project-discovery-set`, `python-source-models`, and `environment-discovery`.
- Update the loading-node table rows for `installed-app-files` and `template-directory-files` if their concrete owner/output/prerequisite wording changes during implementation.
- Add `InstalledAppFilesLoadRequest(Vec<Utf8PathBuf>)` in `djls-project`.
- Implement `load_installed_app_files` in `djls-project` by building the Django-relevant predicate and delegating walking to the neutral `djls_workspace::load_files_for_roots` API.
- The Django-owned installed-app predicate loads only:
  - `apps.py`
  - `models.py` and `models/`
  - `templates/`
  - `templatetags/`
  - `admin.py`, `urls.py`, `forms.py` as role candidates
- Return a `FilesForRootsResult` using `FileRootKind::LibrarySearchPath` for dependency roots and `FileRootKind::Project` for first-party roots.
- Do not recursively scan unrelated `site-packages` directories.
- Do not put Django-specific installed-app predicates in `djls-workspace`; that crate owns walking mechanics only.
- Add a `TemplateDirectoryFilesLoadRequest` or equivalent helper that takes concrete `TemplateDirectory` roots derived from `TEMPLATES[*].DIRS` and statically known installed-app template directories.
- Build a Django-owned predicate for template-directory loading in `djls-project`; delegate traversal to the neutral `djls_workspace::load_files_for_roots` API.
- Introduce loading graph nodes `installed-app-files` and `template-directory-files`. Their graph prerequisites are only existing real graph nodes such as `source-file-set`, `project-discovery-set`, `python-source-models`, and `environment-discovery`. Do not name `effective-settings` or `installed-app-projection` as graph prerequisites unless this subphase also creates them as real loading nodes with terminal-status tests.
- Keep `effective_settings(db, project, env)` and installed-app projection as tracked query reads owned inside the `installed-app-files` / `template-directory-files` activities. Those activities decide whether query results are ready, degraded, or deferred and return typed terminal outcomes for their own nodes.
- In `djls-project`, extend the Phase 3 first-party apply seam with the first real multi-partition policy. Produce project-owned `PartitionedSourceFilePatch` values with installed-app and configured-template-directory partition IDs and precedence. Installed-app partitions must rank below first-party and configured-template-directory partitions; configured-template-directory partitions should rank below first-party workspace files and above installed-app files, matching Django's configured template search order. Add conflict detection and lower-precedence resurrection here, where the behavior has real non-first-party callers.
- Return project-owned `PartitionedSourceFilePatch` values from both activities. The existing node-level file apply intent handles merge, policy-free DB materialization, project-owned finalization, and stable `Project.source_inventory` updates.
- Do not add a Django-specific database apply method; partition metadata, merge precedence, and lower-precedence resurrection stay in `djls-project`, not `djls-db`. `djls-db` still sees only the policy-free materialization patch and returns `SourceFileSetMaterialized`.
- Run both nodes through the neutral `run_loading_plan` runner and both concrete effect adapters in this subphase. Their node terminal statuses must come from `node_status_from_readiness(ProjectSourceFilesApplied)` using the applied installed-app/template-directory partition transitions, not from a separate aggregate readiness field.
- This step is required because Phase 2's bootstrap file set is intentionally bounded and may not include configured template directories named `emails/`, `partials/`, `ui/`, or other project-specific names.
- Preserve existing `File` handles through the policy-free database materialization path and avoid overwriting first-party entries with the same path.
- Do not register `django-apps-ready` in this subphase; Phase 6D adds the milestone after inventory and one semantic consumer prove the loaded files are usable.

**Tests and stop gate**:
- Stop after installed-app file loader tests pass: `cargo test -p djls-project installed_app_files`.
- Stop after template-directory file loader tests pass: `cargo test -p djls-project template_directory_files`.
- Stop after template-directory and installed-app loading nodes pass through the neutral runner and both real effect adapters, with distinct terminal statuses derived through `node_status_from_readiness(ProjectSourceFilesApplied)`: `cargo test -p djls-project loading_template_files`, `cargo test -p djls-server startup`, and `cargo test -p djls --test check`.
- Stop after multi-partition merge tests cover precedence, conflict detection, lower-precedence resurrection, and a partial file-loading readiness case where first-party files `Ready`, installed-app files `Deferred`, and template-directory files `Ready` produce distinct node terminal statuses while `Project.source_inventory` remains one coherent merged source inventory: `cargo test -p djls-project loading_template_files`.
- Stop after a merge-seam/materialization test proves `djls-db` applies policy-free incremental materialization patches without knowing installed-app or configured-template partition names, while project finalization produces `ProjectSourceFilesApplied`: `cargo test -p djls-db source_file_set`.

#### Phase 6C: Static template inventory
**File**: `crates/djls-project/src/templates/inventory.rs`

**Edits**:
- Depends on **6B installed-app/template-directory file loading**.
- Add concrete domain objects:
  - `TemplateDirectory`
  - `ProjectTemplate`
  - `TemplateTagLibrary`
- Add inventory containers:
  - `TemplateDirectoryInventory`
  - `TemplateDirectoryEntry`
  - `TemplateFileInventory`
  - `TemplateTagLibraryInventory`
- `TemplateDirectoryEntry` must include `Discovered(TemplateDirectory)` and `UnknownSettingsDir { issue: SettingsIssue }` so unknown settings directory segments are preserved.
- Add source enums:
  - `TemplateDirectorySource`
  - `TemplateTagLibrarySource`
- Add `TemplateTagLibraryResolution` and `TemplateTagLibraryIssue` for unresolved/ambiguous library entries.
- Add tracked queries:
  - `template_directories(db, project, env)`
  - `template_files(db, project, env)`
  - `template_tag_libraries(db, project, env)`
- Build directories from `TEMPLATES[*].DIRS` and installed app `templates/` when `APP_DIRS` is statically known.
- Build Template File inventory only from configured template directories and installed-app templates that the current `Project.source_inventory` has merged into the current `SourceFileSet`. If a concrete directory is known but not loaded yet, return a deferred/unavailable inventory entry rather than silently omitting it. Use the query-visible source-inventory partition/root projection to distinguish a loaded-empty directory from a known-but-not-loaded directory.
- Build Template Tag Library inventory from Django builtins, installed app `templatetags/*.py`, and statically known `TEMPLATES[*].OPTIONS["libraries"]`.
- Keep tag/filter definition extraction out of this phase.

**Tests and stop gate**:
- Stop after template inventory tests pass: `cargo test -p djls-project template_inventory`.
- Stop after a fixture with configured template directories named `emails/`, `partials/`, or `ui/` is visible to `template_files(db, project, env)` only after those directories have been loaded.
- Stop after transition-shaped template inventory tests prove: loaded empty directory returns `Ready(empty)`; known but not loaded directory returns `Deferred`; stale previous directory load returns stale/degraded provenance rather than current `Ready(empty)`.
- Stop after static Template Tag Library inventory is available with runtime introspection disabled.

#### Phase 6D: First semantic consumer and `django-apps-ready` milestone
**Files**:
- `crates/djls-semantic/src/resolution.rs`
- `crates/djls-ide` callers only if needed for compilation
- `crates/djls-project/src/loading/plan.rs`
- `crates/djls-server/src/startup.rs`

**Edits**:
- Depends on **6B file-loading nodes** and **6C template inventory**.
- Add the first user-visible static template consumer in the same phase that proves authoritative template inventory is usable.
- Move the narrow `discover_templates(db, project, env)` helper or an equivalent minimal `resolve_template` path to read from `djls_project::template_files` for a selected environment.
- Keep the migration deliberately small: prove one template lookup path can use static inventory without waiting for the broad Phase 7 IDE migration.
- If this replaces an old `ProjectTemplateFiles` accessor for that path, remove that accessor in this subphase or record a phase-local cleanup search proving no consumers remain.
- Introduce the `django-apps-ready` loading milestone ID and register it in the active `LoadingPlan` with prerequisites on both `template-directory-files` and `installed-app-files`, using acceptable `NodeTerminalStatus` values produced by the loading-node table and `node_status_from_readiness`. Do not encode this readiness condition in prose only.
- Mark the two graph nodes complete or degraded from their applied partition/node readiness transitions; `django-apps-ready` advances only through the `LoadingPlan` prerequisite mapping and the readiness projection rule. A coherent merged `Project.source_inventory` alone is not sufficient to advance this milestone without the node outcomes named in the loading plan.
- Add an explicit milestone policy test for first-party files `Ready`, installed-app files `Deferred`, and template-directory files `Ready`: the two file-loading nodes must retain distinct terminal statuses, and `django-apps-ready` must follow the configured degraded-prerequisite policy rather than advancing as fully ready from the aggregate source inventory alone.
- Report unknown `INSTALLED_APPS` gaps as progress/log details, not per-file diagnostics.
- Add a phase-local bridge cleanup search: run `rg "ProjectTemplateFiles|template_dirs\(|template_libraries\(" crates/djls-semantic crates/djls-db crates/djls-ide crates/djls-project -g '*.rs'` and remove migrated accessors/callers or document the exact Phase 7 deletion gate for intentional leftovers.

**Tests and stop gate**:
- Stop after minimal static template resolution consumer tests pass: `cargo test -p djls-semantic static_template_resolution`.
- Stop after LoadingPlan milestone policy tests prove `django-apps-ready` requires both `template-directory-files` and `installed-app-files`, reads their applied partition/node transitions rather than only checking that `Project.source_inventory` has a coherent merged view, handles first-party `Ready` / installed-app `Deferred` / template-directory `Ready` according to the configured degraded-prerequisite policy, and does not reference non-existent `effective-settings` or `installed-app-projection` graph nodes: `cargo test -p djls-project loading_plan`.
- Stop after a fixture with a configured template directory named `emails/`, `partials/`, or `ui/` resolves a template through static inventory.
- Stop after the same lookup returns `Deferred` when the template directory is known but its files are not loaded yet.
- Stop after a configuration-change restart test proves the full active static graph re-runs through `source-file-set`, `project-discovery-set`, `python-source-models`, `environment-discovery`, `installed-app-files`, and `template-directory-files`, and rejects superseded applies from the prior run.
- Stop after the workspace builds: `cargo build -q`.

### Success Criteria

#### Automated Verification
**Phase 6A gate**
- [ ] Effective settings tests pass: `cargo test -p djls-project effective_settings`
- [ ] Installed app projection tests pass, including deferred AppConfig details for not-yet-loaded app files: `cargo test -p djls-project installed_apps`

**Phase 6B gate**
- [ ] Installed-app file loader tests pass: `cargo test -p djls-project installed_app_files`
- [ ] Template-directory file loader tests pass: `cargo test -p djls-project template_directory_files`
- [ ] Template-directory and installed-app loading nodes pass through the neutral runner and both real effect adapters with distinct terminal statuses derived through `node_status_from_readiness(ProjectSourceFilesApplied)`: `cargo test -p djls-project loading_template_files`, `cargo test -p djls-server startup`, and `cargo test -p djls --test check`
- [ ] Multi-partition merge tests cover precedence, conflict detection, lower-precedence resurrection, and a partial file-loading readiness case where first-party files `Ready`, installed-app files `Deferred`, and template-directory files `Ready` produce distinct node terminal statuses while `Project.source_inventory` remains one coherent merged source inventory: `cargo test -p djls-project loading_template_files`
- [ ] Database materialization tests prove `djls-db` applies policy-free incremental materialization patches into handle-bearing `SourceFileSetData` without knowing Django partition names, and project finalization produces `ProjectSourceFilesApplied`: `cargo test -p djls-db source_file_set`

**Phase 6C gate**
- [ ] Template inventory tests pass: `cargo test -p djls-project template_inventory`

**Phase 6D gate**
- [ ] Minimal static template resolution consumer tests pass: `cargo test -p djls-semantic static_template_resolution`
- [ ] LoadingPlan milestone policy tests prove `django-apps-ready` requires both `template-directory-files` and `installed-app-files`, reads their applied partition/node transitions rather than only checking that `Project.source_inventory` has a coherent merged view, handles first-party `Ready` / installed-app `Deferred` / template-directory `Ready` according to the configured degraded-prerequisite policy, and does not reference non-existent `effective-settings` or `installed-app-projection` graph nodes: `cargo test -p djls-project loading_plan`
- [ ] Phase-local bridge cleanup search passes or records exact Phase 7 deletion gates: `rg "ProjectTemplateFiles|template_dirs\(|template_libraries\(" crates/djls-semantic crates/djls-db crates/djls-ide crates/djls-project -g '*.rs'`
- [ ] Configuration-change restart test covers the full active static graph through app/template file-loading nodes and superseded apply rejection.
- [ ] Workspace builds: `cargo build -q`

#### Manual Verification
- [ ] Confirm known `INSTALLED_APPS` order is preserved with unknown gaps.
- [ ] Confirm no code guesses installed apps from arbitrary package-like directories.
- [ ] Confirm configured template directories with non-template-like names such as `emails/`, `partials/`, or `ui/` are loaded before `template_files(db, project, env)` is treated as authoritative.
- [ ] Confirm static Template Tag Library inventory is available with runtime introspection disabled.

## Phase 7: semantic features consume static project queries

### Overview
Finish moving template resolution, load-library completions/diagnostics, navigation, references, hover, and diagnostics degradation to `djls-project` queries instead of old `Project` fields. Phase 6 already moved one narrow static template lookup path; this phase broadens that migration to the remaining IDE/semantic consumers.

### Changes Required

#### 1. Replace template resolution API
**File**: `crates/djls-semantic/src/resolution.rs`

**Edits**:
- Replace old `ResolveResult` with `TemplateLookupResult` and `TemplateLookupIssue`.
- Add the outline's environment-scoped discovery helper: `discover_templates(db: &dyn SemanticDb, project: Project, env: DjangoEnvironmentId) -> Vec<Template<'_>>`, built from `djls_project::template_files`.
- Change `resolve_template` to accept the source `File` and a parsed `TemplateName`, or parse the raw template name at the call boundary before entering the resolver. Do not pass arbitrary `String` names through semantic/project APIs.
- Select the environment with `djls_project::environment_for_file(db, project, source)`.
- On `Selected`, search `discover_templates(db, project, env)` by `TemplateName` and template search order.
- On `Unknown` or `Ambiguous`, return `Deferred` with the matching issue.
- Preserve not-found tried paths when template directory inventory is available.
- Update `find_references_to_template` to search only the selected environment and return empty on environment ambiguity.

**Code shape**:
```rust
pub enum TemplateLookupResult<'db> {
    Found(Template<'db>),
    NotFound { name: TemplateName, tried: NonEmpty<Utf8PathBuf> },
    Ambiguous { name: TemplateName, candidates: AtLeastTwo<Template<'db>> },
    Deferred { name: TemplateName, issue: TemplateLookupIssue },
}

pub enum TemplateLookupIssue {
    Environment(EnvironmentSelectionIssue),
    Inventory(TemplateInventoryIssue),
    InvalidTemplateName(TemplateNameParseError),
}
```

#### 2. Update IDE navigation and hover callers
**Files**:
- `crates/djls-ide/src/navigation.rs`
- `crates/djls-ide/src/hover.rs`

**Edits**:
- Pass the current/source file into `resolve_template` and `find_references_to_template`.
- Treat `Deferred` as no navigation target, with trace-level logging only.
- Hover should use static template/tag-library inventory where available and degrade cleanly on ambiguity.

#### 3. Move Template Tag Library availability to static inventory
**Files**:
- `crates/djls-semantic/src/scoping.rs`
- `crates/djls-ide/src/completions.rs`
- `crates/djls-ide/src/diagnostics.rs`

**Edits**:
- Replace reads of `db.template_libraries()` for library inventory with `djls_project::template_tag_libraries(db, project, env)` plus semantic adapters.
- Keep tag/filter definition extraction and `TagSpecs` as semantic concerns.
- Unknown/ambiguous environment selection should suppress environment-specific load-library diagnostics but leave parser/builtin validation active.
- Extend the project/semantic availability projection from Phase 3C so `Unknown` and `Ambiguous` environment selection are mapped once below IDE presentation for diagnostics, completions, navigation, references, and hover.
- Surface workspace/project ambiguity warnings through startup/logging dedupe keys, not per-template diagnostics.

#### 4. Narrow or remove old semantic DB accessors
**Files**:
- `crates/djls-semantic/src/db.rs`
- `crates/djls-db/src/db.rs`
- `crates/djls-bench/src/db.rs`
- `crates/djls-semantic/src/testing.rs`

**Edits**:
- Remove or narrow `template_dirs()` once all consumers have moved.
- Remove or narrow `template_libraries()` for inventory use. If tag/filter definition code still needs a semantic availability adapter, name it after definitions/availability rather than project inventory.
- Update all `SemanticDb` implementors in production, bench, and tests.
- Add a phase-local cleanup search for migrated template inventory accessors: run `rg "template_dirs\(|template_libraries\(|template_files\(|ProjectTemplateFiles|TemplateDirs" crates/djls-semantic crates/djls-db crates/djls-ide crates/djls-bench -g '*.rs'` and remove stale project-inventory accessors or document exact Phase 8/10 deletion gates for leftovers.

#### 5. Remove migrated old `Project` fields
**File**: `crates/djls-semantic/src/project/input.rs`

**Edits**:
- Remove `template_dirs`, `template_files`, and `template_libraries` fields after template feature consumers have moved.
- Remove related setters and tracked helper queries that are no longer used by this phase.
- For `python_index`, complete the outline's template-feature migration in this phase by ensuring no template feature reads it. The Phase 8 extraction migration then removes the remaining `ProjectPythonIndex` extraction consumers and deletes the field.

### Success Criteria

#### Automated Verification
- [ ] Template resolution tests pass: `cargo test -p djls-semantic resolution`
- [ ] Load scoping/availability tests pass: `cargo test -p djls-semantic scoping`
- [ ] IDE navigation tests pass: `cargo test -p djls-ide navigation`
- [ ] IDE completion tests pass: `cargo test -p djls-ide completions`
- [ ] IDE diagnostics tests pass: `cargo test -p djls-ide diagnostics`
- [ ] Runtime-introspection-disabled static template behavior passes: `cargo test -p djls-semantic static_template_inventory`
- [ ] Project/semantic availability matrix covers unknown/ambiguous environment selection for diagnostics, completions, navigation, references, and hover.
- [ ] Template inventory accessor cleanup search passes or records exact deletion gates: `rg "template_dirs\(|template_libraries\(|template_files\(|ProjectTemplateFiles|TemplateDirs" crates/djls-semantic crates/djls-db crates/djls-ide crates/djls-bench -g '*.rs'`
- [ ] Workspace builds: `cargo build -q`

#### Manual Verification
- [ ] Open a template in a fixture with static Template Directories and confirm `{% include %}` can navigate without runtime introspection.
- [ ] Confirm `{% load %}` completions show statically discovered Template Tag Libraries.
- [ ] Confirm ambiguous environments do not emit per-file environment diagnostics.

## Phase 8: extraction inputs move from Python index to project inventories

### Overview
Move model and templatetag extraction inputs from the old `ProjectPythonIndex` and stored external maps to environment-scoped `djls-project` Python module inventories.

### Changes Required

#### 1. Add Python module inventory to `djls-project`
**File**: `crates/djls-project/src/python/inventory.rs`

**Edits**:
- Add `PythonModuleRole`, `PythonModule`, and `PythonModuleInventory`.
- Derive roles from project layout, installed apps, template inventory, and known conventional module paths.
- Add `#[salsa::tracked(returns(ref))] pub fn python_module_inventory(db: &dyn Db, project: Project, env: DjangoEnvironmentId) -> PythonModuleInventory`.
- Include workspace and loaded installed-app files. Do not include unloaded roots.

**Code shape**:
```rust
pub enum PythonModuleRole {
    Model,
    TemplateTag,
    AppConfig,
    Urls,
    Admin,
    Forms,
}

pub struct PythonModule {
    module: PyModuleName,
    file: File,
    roles: Vec<PythonModuleRole>,
    origin: OriginSet,
}
```

#### 2. Update semantic extraction queries
**File**: `crates/djls-semantic/src/queries.rs`

**Edits**:
- Replace `project_model_modules(db, legacy_project)` with `python_module_inventory(db, project, env)` filtered by `PythonModuleRole::Model`.
- Replace `project_templatetag_modules(db, legacy_project)` with `python_module_inventory(db, project, env)` filtered by `PythonModuleRole::TemplateTag`.
- Workspace and installed-app files should be read through tracked `File` inputs.
- Preserve Salsa incremental behavior: editing one templatetag file invalidates extraction for that file, not the whole inventory.

#### 3. Add database scanning boundary for loaded installed-app files
**File**: `crates/djls-db/src/scanning.rs`

**Edits**:
- Add the file and export it from `crates/djls-db/src/lib.rs` if needed.
- Move imperative apply helpers for installed-app file-set updates here if `db.rs` is getting too large.
- Do not recreate `refresh_external_data` as a monolithic pipeline.

#### 4. Delete or quarantine old Python index/external map refresh paths
**Files**:
- `crates/djls-semantic/src/project/input.rs`
- `crates/djls-semantic/src/project/sync.rs`
- `crates/djls-semantic/src/project/resolve.rs`

**Edits**:
- Remove `ProjectPythonIndex`, `ProjectPythonModule`, and old `project_model_modules`/`project_templatetag_modules` once no consumers remain.
- Quarantine external extraction maps on old `Project` behind the Phase 9 enrichment path only, matching the outline. Delete them in Phase 9 once enrichment inputs replace them.
- Keep only helper functions still needed by Phase 9 enrichment or move them to their new owner.
- Add a phase-local cleanup search: run `rg "ProjectPythonIndex|ProjectPythonModule|project_model_modules|project_templatetag_modules|python_index" crates/djls-semantic crates/djls-db crates/djls-project -g '*.rs'` and remove stale extraction-input bridges or document the exact Phase 9/10 deletion gate for intentional enrichment leftovers.

### Success Criteria

#### Automated Verification
- [ ] Python module inventory tests pass: `cargo test -p djls-project python_module_inventory`
- [ ] Semantic extraction query tests pass: `cargo test -p djls-semantic queries`
- [ ] Incremental extraction tests pass: `cargo test -p djls-db invalidation`
- [ ] Corpus extraction tests still pass if corpus is synced: `cargo test -p djls-semantic --test corpus`
- [ ] Python index cleanup search passes or records exact enrichment deletion gates: `rg "ProjectPythonIndex|ProjectPythonModule|project_model_modules|project_templatetag_modules|python_index" crates/djls-semantic crates/djls-db crates/djls-project -g '*.rs'`
- [ ] Workspace builds: `cargo build -q`

#### Manual Verification
- [ ] Editing a known workspace templatetag module invalidates only that module's tracked extraction.
- [ ] Adding a loaded app templatetag file makes it appear in `python_module_inventory` after file-set update.
- [ ] No code path scans all of `site-packages` to find extraction inputs.

## Phase 9: runtime Project Introspection as enrichment

### Overview
Reintroduce runtime-backed data as optional enrichment hints with typed status, superseded-result guards, and cache-as-hint semantics. Runtime data augments static Project Facts; it does not own startup readiness.

### Changes Required

#### 1. Expand enrichment input/domain types
**File**: `crates/djls-project/src/enrichment.rs`

**Edits**:
- Add `ProjectEnrichmentHints` with runtime/deep hint fields:
  - `runtime_template_dirs`
  - `runtime_template_libraries`
  - `runtime_installed_apps`
  - `deep_extraction_hints`
- Add `ProjectEnrichmentDraft` and extend `ProjectEnrichmentIssue` for runtime/cache/deep enrichment failures.
- Expand the Phase 3 `Project.enrichment` domain facts into the single project-visible enrichment fact shape. Do not reintroduce `ProjectEnrichmentAvailability`, and do not put executor status inside `ProjectEnrichmentHints`.
- Add the `enrichment` scheduler-only loading node from the loading-node table in this phase. It may track only pending/running/superseded execution state and is never a core readiness authority. Terminal progress status comes from the scheduler outcome plus the applied `Project.enrichment` domain result. Runtime success/failure/cache-staleness visible to queries comes from `Project.enrichment`.
- Add merge helpers such as `merge_template_libraries(static_inventory, enrichment)`.
- Preserve provenance/staleness on runtime/cache values.

**Code shape**:
```rust
pub enum ProjectEnrichment {
    Absent,
    Disabled,
    Fresh(ProjectEnrichmentHints),
    CachedStale { hints: ProjectEnrichmentHints, issue: ProjectEnrichmentIssue },
    Failed { issue: ProjectEnrichmentIssue },
    Unavailable { issue: ProjectEnrichmentIssue },
}

pub enum ProjectEnrichmentIssue {
    RuntimeUnavailable { interpreter: Option<Interpreter>, kind: RuntimeUnavailableKind },
    InspectorFailed { kind: InspectorFailureKind },
    CacheStale { key: EnrichmentCacheKey, age: CacheAge },
    CacheReadFailed { kind: CacheIssueKind },
}
```

#### 2. Move inspector provider output translation out of semantic analysis
**Files**:
- `crates/djls-db/src/enrichment_provider.rs` or equivalent infrastructure-owned provider module
- `crates/djls-db/src/enrichment_cache.rs` or equivalent infrastructure-owned cache module
- `crates/djls-semantic/build.rs`
- `crates/djls-semantic/inspector/`
- `crates/djls-semantic/src/project/introspector.rs`
- `crates/djls-project/src/enrichment.rs`

**Edits**:
- Keep stable enrichment domain types (`ProjectEnrichment`, `ProjectEnrichmentDraft`, merge policy, typed issues) in `djls-project`.
- Move inspector subprocess invocation, JSON response DTOs, DTO parsing, zipapp build packaging, embedded inspector asset ownership, cache I/O, cache freshness policy, and provider fallback behavior to infrastructure-owned code, not `djls-project` and not `djls-semantic`.
- Translate successful inspector/cache responses into `djls_project::ProjectEnrichmentDraft` at the infrastructure-to-project seam. Only drafts and typed issues cross into `djls-project`.
- Move `crates/djls-semantic/inspector/` and the `djls_inspector.pyz` packaging responsibility from `crates/djls-semantic/build.rs` to the chosen infrastructure owner.
- Update the `include_bytes!(concat!(env!("OUT_DIR"), "/djls_inspector.pyz"))` location so embedded inspector bytes are owned by the infrastructure provider, not `djls-semantic` or `djls-project`.
- Keep `djls-semantic` as a consumer of merged static/enrichment facts only. It must not own inspector JSON DTOs, cache shape, subprocess policy, zipapp packaging, embedded inspector bytes, or provider fallback behavior after this phase.
- Do not mutate environment candidates, template inventories, or semantic outputs directly from inspector responses.
- On failure, produce a failed enrichment draft/state instead of returning `None` to hide the failure.
- Delete or quarantine the old `djls-semantic::project::introspector` module once no callers remain; if a temporary re-export is needed, mark Phase 10 as its deletion gate.

#### 3. Apply enrichment in the concrete database
**File**: `crates/djls-db/src/db.rs`

**Edits**:
- Add `apply_enrichment(&mut self, draft: ProjectEnrichmentDraft)` that updates `Project.enrichment` to `Fresh`, `CachedStale`, `Failed`, or `Unavailable` as appropriate.
- Compare current enrichment fields before calling setters on the stable `Project` input.
- Keep generation/superseded-result rejection in `startup.rs` via `GenerationGuard`; database methods must not know startup generations.

#### 4. Replace old inspector cache helpers
**Files**:
- `crates/djls-semantic/src/project/sync.rs`
- `crates/djls-project/src/enrichment.rs`
- `crates/djls-db/src/enrichment_cache.rs` or equivalent infrastructure-owned cache module

**Edits**:
- Replace template-library snapshot cache helpers with enrichment cache helpers.
- Cache keys must include discovery-relevant config and enough provenance/staleness metadata to treat cache results as hints.
- A warm cache may seed enrichment status, but the fresh file-set/static pass remains authoritative.
- Delete `load_template_library_cache` and `refresh_external_data` public exports when no longer used.
- Add a phase-local cleanup search: run `rg "load_template_library_cache|refresh_external_data|djls-semantic/inspector|project::introspector|djls_inspector.pyz" crates/djls-semantic crates/djls-project crates/djls-db crates/djls-server -g '*.rs' -g 'build.rs'` and remove stale semantic-owned inspector/cache bridges or document the exact Phase 10 deletion gate for intentional temporary re-exports.

#### 5. Schedule enrichment as optional startup/background work
**File**: `crates/djls-server/src/startup.rs`

**Edits**:
- Run the scheduler-only `enrichment` loading node for pending/running progress, not as a terminal readiness authority.
- Implement runtime/cache acquisition as an enrichment activity service that returns `ProjectEnrichmentDraft`; the neutral driver invokes effect adapters that may run or explicitly skip it according to their runtime policy. The LSP effect adapter reports progress through `StartupController`'s adapter hooks and applies the guarded result.
- Capture immutable runtime config into server-local `StartupRunInputs` / `ProjectLoadingSnapshot` under a short session lock, alongside the `GenerationGuard`.
- Lower the captured data into a project-owned `EnrichmentLoadRequest` before invoking `djls-project` enrichment activity code.
- Run subprocess/cache work from that request outside the lock.
- Apply translated enrichment drafts under a short lock: LSP through `GenerationGuard::apply`; CLI directly if it runs enrichment, or an explicit skipped outcome if it does not.
- After the guarded apply writes `Project.enrichment`, derive the scheduler/progress view from that domain fact instead of writing a second terminal task value.
- If an `Enriched` view is added in this phase, implement it as a projection from `Project.enrichment`, not as a core `LoadingPlan` milestone prerequisite over independent task terminal status.
- Static `workspace-ready` and `django-apps-ready` milestone states must remain successful if enrichment fails.
- Report enriched or degraded enrichment through progress/logging.
- If enrichment can outlive core startup progress, use a separate non-cancellable progress token.

### Success Criteria

#### Automated Verification
- [ ] Enrichment merge tests pass: `cargo test -p djls-project enrichment`
- [ ] Inspector provider/translation tests pass in the infrastructure owner and prove only `ProjectEnrichmentDraft`/typed issues cross the provider-to-domain seam: `cargo test -p djls-db enrichment_provider` or the equivalent provider-module test.
- [ ] Inspector zipapp packaging/build ownership lives in the infrastructure provider, and neither `djls-semantic` nor `djls-project` owns `inspector/`, inspector `build.rs` packaging, or embedded inspector bytes: `cargo test -p djls-db --no-run`, `cargo test -p djls-project --no-run`, and `cargo test -p djls-semantic --no-run`.
- [ ] Database enrichment apply tests pass: `cargo test -p djls-db enrichment`
- [ ] Enrichment loading-node tests cover run/skip behavior through the neutral driver/shared plan, LSP effect adapter, and CLI explicit-skip effect outcome: `cargo test -p djls-project loading_enrichment`, `cargo test -p djls-server startup`, and `cargo test -p djls --test check`
- [ ] Cache-as-hint tests pass: `cargo test -p djls-project cache_as_hint`
- [ ] Inspector/cache bridge cleanup search passes or records exact Phase 10 deletion gates: `rg "load_template_library_cache|refresh_external_data|project::introspector|djls_inspector.pyz" crates/djls-semantic crates/djls-project crates/djls-db crates/djls-server -g '*.rs' -g 'build.rs'`
- [ ] `Project.enrichment` expansion remains compatible with Phase 3–8 assertions: `cargo test -p djls-project enrichment_compat`
- [ ] Workspace builds: `cargo build -q`

#### Manual Verification
- [ ] With a failing Python interpreter, static template inventory remains available and enrichment status records failure.
- [ ] With a warm cache, cached enrichment is marked as a hint and does not skip the fresh static file-set pass.
- [ ] No request waits behind a failed or slow inspector subprocess.
- [ ] Confirm provider/cache/zipapp internals remain behind the enrichment provider seam and stable Project Facts merge code consumes only drafts and typed issues.

## Phase 10: CLI, real-LSP readiness, and old Project removal

### Overview
Audit CLI/LSP parity on the shared project model, add real LSP startup/readiness coverage, remove the old fat `Project` semantic API, and update architecture documentation. Each real static loading node must already have CLI and LSP effect-adapter coverage from the phase that introduced it. Phase 10 verifies that invariant and finishes user-facing CLI check behavior by reading tracked Project Facts queries during checking.

### Changes Required

#### 1. Verify CLI check parity on the shared project model
**Files**:
- `crates/djls/src/commands/check.rs`
- `crates/djls/src/commands/common.rs`
- shared loading modules introduced by earlier phases

**Edits**:
- Do not use Phase 10 to catch up missing CLI graph wiring. If any real static loading node from Phases 4–9 lacks CLI effect-adapter coverage, go back to the phase that introduced the node and add the missing adapter/test there.
- In `djls check`, call the neutral loading driver for real loading-node rows only: `source-file-set`, `project-discovery-set`, `python-source-models`, `environment-discovery`, `installed-app-files`, and `template-directory-files`.
- Do not treat tracked queries as loading activity services. `effective_settings`, installed-app projection, template inventory, and Python module inventory remain environment-scoped tracked queries unless the phase that changes them adds loading-node table rows and both effect adapters.
- After the real loading graph finishes, read tracked Project Facts and semantic APIs during checking through the stable Project handle: `environment_for_file`, `effective_settings`, installed-app projections, `template_files`, `template_tag_libraries`, and `python_module_inventory` as needed by the checked feature.
- The CLI effect adapter applies typed loading updates directly to `DjangoDatabase`; it does not use LSP generation guards or work-done progress.
- Use `environment_for_file` for each template being checked.
- Report ambiguous Django Environment selection as terminal warning/error according to CLI strictness. Do not hide ambiguity by picking a global default.
- Keep explicit path behavior: when paths are provided, validate those paths even if template directories are unknown.
- Add a parity audit over the loading-node table and `NODE_SPECS`: every static loading-node row before Phase 10 must have a matching manifest entry and both CLI and LSP effect-adapter tests named in the phase that introduced it.

#### 2. Extend pytest-lsp startup integration tests
**Files**:
- `tests/lsp/test_startup.py`

**Edits**:
- Keep using the Phase 1 pytest-lsp stdio harness. Do not hand-roll a Rust LSP harness for this contract.
- Add supported and unsupported client capability profiles in this test module or reuse them if a Phase 3 progress test fixture already exposed them.
- Extend message assertions as needed for request behavior while startup tasks are in progress:
  - `window/workDoneProgress/create`
  - `$/progress`
  - `window/logMessage`
  - diagnostics or diagnostic responses for the in-progress request case
- Add or expand tests:
  - `supported_client_receives_startup_progress_begin_report_end`
  - `template_request_works_while_loading_in_progress`
  - degraded request behavior while static tasks are pending
- Keep pytest fixtures local to `tests/lsp/test_startup.py` unless a second LSP test file needs them.

#### 3. Remove old semantic Project fact bag
**Files**:
- `crates/djls-semantic/src/lib.rs`
- `crates/djls-semantic/src/project/input.rs`
- `crates/djls-semantic/src/project/sync.rs`
- `crates/djls-semantic/src/project/static_model.rs`
- `crates/djls-semantic/src/project/static_django_environments.rs`
- `crates/djls-semantic/src/project/static_resolver.rs`
- `crates/djls-semantic/src/project.rs`
- `crates/djls-db/src/db.rs`
- `crates/djls-bench/src/db.rs`

**Edits**:
- Remove public re-exports of old `Project`, `ProjectTemplateFiles`, `ProjectPythonIndex`, `TemplateDirs`, old cache loaders, and `refresh_external_data`.
- Delete old static scaffolding that used `Fact<T>` after equivalent `djls-project` APIs exist.
- Remove `ProjectDb::project()` and old current-project helper methods from semantic traits.
- Remove or move any remaining inspector runtime code from `djls-semantic`; Phase 9 should have moved provider/DTO/cache/zipapp ownership to the infrastructure provider while `djls-project` owns only enrichment drafts, state, typed issues, and merge policy.
- Update all imports to use `djls_project` domain types and query APIs.

#### 4. Update docs
**Files**:
- `ARCHITECTURE.md`
- `CONTEXT.md`

**Edits**:
- Add `djls-project` to crate responsibilities.
- Describe protocol-ready, workspace-ready, django-apps-ready, and enriched readiness phases.
- Describe explicit `SourceFileSet` inputs.
- Describe static Django Discovery as the primary source of Project Facts.
- Describe runtime Project Introspection as enrichment only.
- Remove references to `refresh_external_data` as the startup extension point.
- Keep canonical terminology from `CONTEXT.md`: Project, Workspace, Project Facts, Django Environment, Django Discovery, Static Extraction, Project Introspection, Template Directory, Template Tag Library.

#### 5. Final cleanup
**Files**: workspace-wide

**Edits**:
- Run `rg "refresh_external_data|load_template_library_cache|ProjectPythonIndex|ProjectTemplateFiles|TemplateDirs|Fact<|ProjectDiscoveryIssue|Project Model" crates docs -g '*.rs' -g '*.md'` and remove stale references or explain intentional historical docs.
- Run `rg "django_settings_module" crates/djls-server crates/djls-db crates/djls-semantic crates/djls-project -g '*.rs'` and confirm no startup path globally selects one settings module.
- Run `rg "window.workDoneProgress|create_work_done_progress|Client::progress|WorkDoneProgress" crates/djls-server -g '*.rs'` and confirm progress creation and emission are correctly gated.

### Success Criteria

#### Automated Verification
- [ ] Loading-node parity audit confirms every static node row has both CLI and LSP effect-adapter tests in its introduction phase.
- [ ] CLI check tests pass: `cargo test -p djls --test check`
- [ ] Real LSP startup tests pass: `uv run pytest tests/lsp/test_startup.py`
- [ ] Project-model fixture tests pass: `cargo test -p djls-project`
- [ ] Semantic tests pass: `cargo test -p djls-semantic`
- [ ] IDE tests pass: `cargo test -p djls-ide`
- [ ] Full cargo test suite passes: `cargo test -q`
- [ ] Formatting check passes: `just fmt --check`
- [ ] Clippy passes: `just clippy`
- [ ] Pre-commit/lint passes: `just lint`

#### Manual Verification
- [ ] Run `djls serve` through the LSP harness and observe fast handshake, non-blocking `initialized`, progress/log fallback behavior, and degraded-mode requests while loading is in progress.
- [ ] Run `djls check` against a multisite fixture and confirm ambiguity is reported instead of hidden behind a default global settings module.
- [ ] Confirm `ARCHITECTURE.md` and `CONTEXT.md` describe the new startup model without old fact-bag language.

## Testing Strategy

### Unit Tests
- Startup generation/state/progress transitions in `crates/djls-server/src/startup.rs`.
- Client capability parsing in `crates/djls-server/src/client.rs`.
- Neutral `SourceFileSet` construction in `djls-source`, stable `Project.source_inventory` invalidation in `djls-project`, project-owned source partition merge policy in `djls-project`, and partition-policy-free apply/handle preservation in `djls-db`.
- Neutral workspace file loading in `djls-workspace` and Django-specific installed-app bounded loading in `djls-project`.
- `djls-project` query tests for layout, Python source models, settings candidates, module resolution, environments, effective settings, installed apps, template inventory, Python module inventory, and enrichment merge.
- Semantic adapters for template lookup, load-library availability, and extraction inventories.

### Integration Tests
- CLI `djls check` tests under `crates/djls/tests/check.rs` continue to cover explicit files, directories, stdin, ignore/select behavior, and no-template success.
- Add project-model integration fixtures under `crates/djls-project/tests/` for multisite, settings composition, installed apps, ignored files, template inventory, cache-as-hint, explicit config-load failure/fallback issues, and degraded enrichment.
- Keep corpus tests for extraction and model graph behavior. Run `just corpus sync` if corpus tests fail due missing data.

### Real LSP E2E Tests
- Add `tests/lsp/test_startup.py` using pytest-lsp in Phase 1, then extend it in Phase 10.
- Required scenarios across the Phase 1 and Phase 10 slices:
  1. initialize returns capabilities without observable startup side effects
  2. the server remains responsive after the initialized notification
  3. request during loading receives a degraded but valid response
  4. work-done progress is created before startup `$/progress` and emits begin/report/end when supported
  5. unsupported clients receive log fallback and no `$/progress`
- Do not use pytest-lsp timing assertions to prove blocked startup work is nonblocking. Use Rust tests with injected loading executors for deliberately blocked work.

### Manual Testing
1. Start a minimal Django fixture with no cache and verify fast LSP handshake.
2. Start a fixture with multiple settings modules and verify a project-level ambiguity warning, not per-file diagnostics.
3. Break the Python interpreter path and verify static Template Directory/Template Tag Library inventory remains available while enrichment records failure.
4. Warm an enrichment cache, restart, and verify the cache is a hint while the fresh static file-set pass still runs.

## Performance Considerations
- Filesystem walking must happen outside the `Session` lock.
- Python parsing for source models should be tracked per `File`; edits should invalidate one file's model, not a whole project graph.
- Do not scan all of `site-packages`. Only register library roots and load bounded installed-app files after `INSTALLED_APPS` is statically known.
- Progress percentages should only be sent when there is a real total. Otherwise send task/milestone messages without fabricated percentages.
- Avoid one mega tracked query. Use bounded indexes: layout, Python source, module resolution, environment candidates, settings composition, template inventory, Python module inventory, enrichment.

## Migration Notes
- This is a clean internal rewrite. Do not preserve old `Project` APIs for compatibility unless a phase still needs a temporary compile bridge.
- Temporary semantic re-exports from moved `djls-project` types are acceptable only until Phase 10.
- `Project Introspection` failure becomes a degraded enrichment state, not startup failure.
- Cache hits never replace the fresh file-set/static discovery pass.
- If any phase reveals a missing design decision that changes public API shape, stop and ask one focused question before continuing.

## References
- Ticket: `docs/tickets/startup-rethink.md`
- Questions: `docs/agents/startup-rethink/questions.md`
- Research: `docs/agents/startup-rethink/research.md`
- Progress research: `docs/agents/startup-rethink/progress-lsp-research.md`
- Design: `docs/agents/startup-rethink/design.md`
- Outline: `docs/agents/startup-rethink/outline.md`
- Architecture decision: `docs/agents/startup-rethink/architecture-decision-project-root.md`
- Reference evidence: `docs/agents/startup-rethink/reference-evidence-rust-analyzer-ruff-ty.md`
- Current implementation assessment: `docs/agents/startup-rethink/current-implementation-assessment.md`
- Phase-by-phase reference assessment: `docs/agents/startup-rethink/phase-by-phase-reference-assessment.md`
- Architecture: `ARCHITECTURE.md`
- Context/glossary: `CONTEXT.md`
- Current startup entrypoint: `crates/djls-server/src/server.rs`
- Current session construction: `crates/djls-server/src/session.rs`
- Current database construction: `crates/djls-db/src/db.rs`
- Current old Project input: `crates/djls-semantic/src/project/input.rs`
- Current old refresh path: `crates/djls-semantic/src/project/sync.rs`
