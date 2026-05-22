# Phase-by-phase reference assessment

This note walks the current `startup-rethink` plan phase by phase and compares each phase to rust-analyzer and Ruff/ty patterns. It records where the current plan aligns, where it drifts, and what should be revised before implementation continues.

Evidence sources are summarized in `reference-evidence-rust-analyzer-ruff-ty.md`.

Decision update: `architecture-decision-project-root.md` resolves the assessment's open choice. DJLS will use a stable `djls_project::Project` Salsa input as the semantic root, while loading/progress/quiescence remain server/CLI orchestration state. Treat the earlier A/B/C discussion as diagnosis; the accepted target is the stable Project root.

## Assessment rubric

- **Aligned**: matches rust-analyzer lowered-input/server-orchestration style or Ruff/ty stable-project-input style.
- **Acceptable with revision**: concept is useful, but the ownership/API/readiness shape should change.
- **Drift**: plan commits to an architecture that conflicts with reference evidence or creates likely dual sources of truth.

## Phase 1: protocol-ready startup

Plan intent:

- Make LSP initialize protocol-only.
- Stop bootstrapping the old `Project` in `DjangoDatabase::new`.
- Keep no-project degraded behavior.

Reference comparison:

- rust-analyzer: handshake and server state are separate from analysis inputs; expensive project loading happens through server queues/tasks.
- Ruff/ty: LSP workspace initialization/readiness is session state, not a Salsa input.

Assessment: **Aligned**.

Keep this. It is one of the strongest parts of the rewrite.

## Phase 2: neutral source and workspace loading primitives

Plan intent:

- Add neutral file/source-root/file-set primitives.
- Keep walking mechanics in `djls-workspace`.
- Avoid Django policy in source/workspace crates.

Reference comparison:

- rust-analyzer: VFS `FileSetConfig`, `SourceRoot`, `FileText`, source-root mapping are lowered inputs/primitives.
- Ruff/ty: `Files`, `File`, `FileRoot`, and indexed project files are domain infrastructure separate from project settings.

Assessment: **Aligned**.

The neutral primitives are useful regardless of the higher-level readiness redesign.

## Phase 3A1: `djls-project` helper boundary

Plan intent:

- Create `djls-project`.
- Move interpreter/env helpers.
- Keep helper modules private with explicit root exports.

Reference comparison:

- rust-analyzer has a `project-model` crate for real-world discovery/build-system concerns separate from core analysis.
- Ruff/ty splits `ty_project`, `ty_python_core`, module resolver, files, and server/session.

Assessment: **Aligned**.

Keep the crate boundary. The question is what `djls-project` owns: domain inputs and discovery, not necessarily an ambient loading-state singleton.

## Phase 3A2: source loading state, merge seam, DB materialization

Plan intent:

- Define `ProjectLoadingState` as the single Salsa-visible readiness handle.
- Store `source_files`, `discovery`, `enrichment` availability fields.
- Apply source files by setting `ProjectLoadingState.source_files` to `Ready`, `Loading`, `Stale`, `Unavailable`, `Deferred`, or `Failed`.
- Preserve previous ready files in degraded states.

Reference comparison:

- rust-analyzer does not have a DB-owned project-loading readiness input. It lowers roots/files/crates, while loading/quiescence lives in `GlobalState`.
- Ruff/ty does store a DB-owned `Project` handle, but `Project` is a stable domain root input with metadata/settings/file set/open files/diagnostics, not a readiness bag.

Assessment: **Drift**.

This is the central divergence.

Problem:

- `ProjectLoadingState` is an ambient singleton readiness state.
- It mixes semantic facts (`ReadyProjectSourceFiles`) with lifecycle/readiness (`Loading`, `Stale`, `Failed`).
- It creates a second readiness surface alongside per-partition/node transitions.
- It hand-carries `previous` snapshots, which increases apply-path burden.

Revision needed:

- Decide whether the target is rust-analyzer-style lowered inputs or Ruff/ty-style stable project root.
- If rust-analyzer-style: move loading/stale/progress state out of Salsa; keep source roots/file sets as domain inputs; derive query availability from actual domain inputs and server loading state where needed.
- If Ruff/ty-style: replace `ProjectLoadingState` with a stable `WorkspaceProject`/`ProjectWorkspace` input that owns source roots/file set/settings/discovery diagnostics. Mutate fields via Salsa setters; do not keep a separate readiness bag.

## Phase 3A3: source-file node through CLI

Plan intent:

- Introduce the neutral one-node loading plan.
- Run `source-file-set` through CLI executor.
- Use observer events and terminal projection.

Reference comparison:

- rust-analyzer has server/task orchestration outside core, but the exact loading graph abstraction is DJLS-specific.
- It is acceptable to have a graph as orchestration if it does not become semantic truth.

Assessment: **Acceptable with revision**.

The runner/effects/observer split is useful, but node terminal status should be clearly orchestration/progress state. It should not require a global Salsa readiness input as its source of truth.

Revision needed:

- Keep the graph for orchestration and parity between CLI/LSP.
- Make the graph observe or apply domain inputs, not maintain a parallel readiness ontology.
- Make terminal status a projection of domain outcomes or server task outcomes, not a reason to add `ProjectLoadingState` fields.

## Phase 3A4: LSP generation, source-file executor, progress, configuration restart

Plan intent:

- Add server-local generation guard and immutable snapshots.
- Run LSP source-file loading through the neutral graph.
- Add progress lifecycle and log fallback.
- Wire configuration restart.

Reference comparison:

- rust-analyzer: `GlobalState` owns operation queues, progress, and quiescence; Salsa DB receives coherent changes at lowering seams.
- Ruff/ty: workspace/session initialization and deferral are server/session state; project database updates happen in place after config/project discovery.

Assessment: **Mostly aligned, except reset writes into `ProjectLoadingState`**.

Good:

- generation guard is server-local;
- stale document evidence is server-local;
- progress is server-local and now nonblocking;
- LSP executor does not hold session lock across file walking.

Drift:

- `begin_project_loading_run` writes `Loading`/`Stale` into `ProjectLoadingState` before work starts.
- A superseded or failed server operation affects semantic readiness unless carefully repaired.

Revision needed:

- Keep `StartupController`, `GenerationGuard`, and progress.
- Reconsider whether reset should mutate Salsa semantic inputs at all. In rust-analyzer style, a new load attempt would be server state until it can atomically lower coherent facts. If stale/previous facts must be visible to queries, consider representing them as stable project-domain revisions rather than a global loading enum.

## Phase 3B: discovery and enrichment loading-state scaffolding

Plan intent:

- Expand `ProjectLoadingState.discovery` to `Loading`, `Ready(ProjectDiscoverySet)`, `Unavailable`, `Stale`.
- Add `ProjectDiscoverySet`, `RootDiscoveryInput`, `DjangoEnvironmentSeed`, typed discovery issues.
- Keep root config/interpreter/env-file loading unwired.

Reference comparison:

- rust-analyzer’s `ProjectWorkspace` is concrete domain data with partial errors and build-script/rustc fields, later lowered to crate graph/source roots.
- Ruff/ty’s `ProjectMetadata`, `Settings`, `Program`, and `Project` are stable domain inputs with diagnostics.

Assessment: **Drift unless reshaped**.

Good:

- root-scoped `ProjectDiscoverySet` is the right domain concept.
- typed issues and no global selected settings module are right.

Bad:

- wrapping it in `ProjectDiscoveryAvailability` repeats the readiness singleton problem.
- discovery should likely be the stable project/root domain input itself, with diagnostics/errors, not a field inside a loading-state bag.

Revision needed:

- Promote `ProjectDiscoverySet` / root discovery data to the core stable project/workspace input shape.
- Store typed issues as domain diagnostics/provenance on the discovery data.
- Keep `Loading/Stale` in server loading state, or model prior/current project revisions explicitly if queries need stale provenance.

## Phase 3C: root discovery data through shared activity code

Plan intent:

- Structured root settings load.
- Discovery data and DB apply.
- Add `project-discovery-set` graph node.
- Move availability projection to `djls-project::availability`.
- Restart discovery on configuration change.

Reference comparison:

- rust-analyzer keeps project discovery in `project-model` and lowers coherent results into inputs; failed reload can keep old workspace.
- Ruff/ty rediscovery updates `Program` and reloads existing `Project`; failures keep old config and produce diagnostics.

Assessment: **Acceptable with major readiness revision**.

Good:

- structured root settings outcome;
- typed config/interpreter/env issues;
- root-scoped data;
- server does not construct project facts directly;
- config restart uses guarded apply.

Drift:

- planned discovery apply updates `ProjectLoadingState.discovery`.
- degraded request matrix likely reads a global project facts availability enum rather than domain inputs/outcomes.

Revision needed:

- Reframe `apply_project_discovery_data` as updating stable project/root discovery inputs.
- On failed rediscovery, keep old applied discovery when appropriate, with new diagnostics, matching rust-analyzer/Ruff behavior.
- Availability projection should be derived from domain inputs/outcomes and server load state, not raw `ProjectLoadingState` fields.

## Phase 3D: layout, provenance, legacy queue cleanup

Plan intent:

- Add `project_layout_index(db) -> ProjectLayoutIndexOutcome`.
- Branch on `ProjectLoadingState.source_files` for loading/unavailable/stale.
- Clean old queue/cache paths.

Reference comparison:

- rust-analyzer derives source roots/path resolution from VFS/source-root inputs.
- Ruff/ty lazily derives indexed files from `Project` included paths/file set and resets the index on reload.

Assessment: **Mixed**.

Good:

- readiness-bearing layout outcome is better than collapsing absent inputs to empty ready index.
- queue cleanup is aligned.

Drift:

- layout is specified to read `db.project_loading_state().source_files(db)` as the readiness source.
- This bakes the ambient singleton into the first major derived query.

Revision needed:

- Make layout index depend on source roots/file-set domain inputs or a stable project root input.
- If source files are not yet available, that should be represented by absence/incomplete domain inputs plus server loading state, or by a stable project input field, not a separate loading-state singleton.

## Phase 4: Python source model and settings candidates

Plan intent:

- Move domain name newtypes.
- Add Ruff AST anti-corruption layer.
- Add tracked `python_source_model(file)` and `python_source_index(db)`.
- Add `python-source-models` readiness observation node.
- Add settings candidates.

Reference comparison:

- rust-analyzer derives name/module resolution as tracked queries over files/source roots/crates.
- Ruff/ty parses Python and resolves modules from stable `Program` search paths and `Project` settings; readiness is derived from tracked outcomes.

Assessment: **Mostly aligned if Phase 3 state is fixed**.

Good:

- tracked per-file source model;
- DJLS-native anti-corruption types instead of exposing Ruff AST;
- source text through `File::source(db)`;
- readiness observation node reads live tracked query rather than storing a `ProjectLoadingState` field.

Risk:

- `python_source_index(db)` currently has no explicit project/root handle parameter in the plan, implying another ambient singleton read path.
- It depends on source files and discovery, which the current plan makes global via `ProjectLoadingState`.

Revision needed:

- Parameterize source-model/index queries by a stable project/workspace/root handle where practical, or make the stable root available through a real Ruff/ty-style `Project` input.
- Keep the observation node as progress/orchestration, not semantic truth.

## Phase 5: module resolution and Django Environment candidates

Plan intent:

- Add import roots and module resolution.
- Add all Django Environment candidates and file-scoped selection.
- Add `environment-discovery` readiness observation node.
- Add `workspace-ready` milestone.

Reference comparison:

- rust-analyzer module/name resolution derives from crate/source-root inputs.
- Ruff/ty module resolution depends on tracked `Program` search paths and project settings; outcomes are tracked query results.

Assessment: **Aligned in principle, but depends on fixing stable input roots**.

Good:

- no global selected settings module;
- multiple candidates preserved;
- environment selection is file-scoped;
- readiness observation node is derived from live query outcome.

Risks:

- plan still references `ProjectDiscoverySet`, source roots, and source file readiness through the current loading-state architecture.
- milestones can become a third readiness surface if not purely derived.

Revision needed:

- Make import roots/search paths/env candidates stable domain inputs/queries.
- `workspace-ready` should be a server/graph projection over query outcomes and outstanding work, not a semantic input.

## Phase 6: effective settings, installed apps, static template inventory

Plan intent:

- Derive effective settings and installed app projection as tracked queries.
- Add installed-app/template-directory file-loading nodes.
- Merge partitions into aggregate source files.
- Add template inventories and `django-apps-ready` milestone.

Reference comparison:

- rust-analyzer partitions VFS into source roots and derives crates/modules from that.
- Ruff/ty stores project included paths and lazily derives indexed files/inventories through a single mutation/invalidation path.

Assessment: **High drift risk**.

Good:

- installed apps from known `INSTALLED_APPS` only;
- bounded installed-app/template directory file loading;
- inventories are tracked queries;
- database materialization remains policy-free.

Major risk:

- `ProjectLoadingState.source_files` becomes an aggregate merged file set while individual file-loading nodes have separate partition readiness.
- Milestones use partition transitions, queries use aggregate source files, and inventories need root/partition readiness projection. This is a multi-source readiness model.

Revision needed:

- Prefer modeling app/template roots and inventories directly as domain inputs/indexes.
- If partitioned file loading remains, make each partition/inventory have one authoritative readiness owner. Avoid using aggregate `source_files = Ready` as an independent semantic readiness surface.
- Consider Ruff/ty lazy indexed inventory style: settings/root changes reset inventory to lazy; a scheduled node may warm it, but queries derive from the same inventory state.

## Phase 7: semantic features consume static project queries

Plan intent:

- Migrate template resolution, load-library completions/diagnostics, navigation, references, hover to `djls-project` queries.
- Remove/narrow old semantic DB accessors.

Reference comparison:

- Both references favor semantic features consuming derived project/domain queries rather than old global fact bags.

Assessment: **Aligned if prior phases produce stable domain queries**.

Risk:

- If prior phases keep the ambient loading-state singleton, IDE/semantic callers may branch on global readiness instead of domain query outcomes.

Revision needed:

- Keep IDE/semantic APIs result-shaped: `Found`, `NotFound`, `Ambiguous`, `Deferred`, etc.
- Do not expose raw loading-state enums to feature code.

## Phase 8: extraction inputs move from Python index to project inventories

Plan intent:

- Add Python module inventory.
- Move semantic extraction inputs to environment-scoped inventories.
- Delete/quarantine old Python index/external map refresh paths.

Reference comparison:

- Aligned with deriving semantic data from file/module/project inventories.
- Similar to rust-analyzer semantic queries over crate/module maps and Ruff/ty queries over project/module resolver.

Assessment: **Aligned if inventory roots are fixed**.

Revision needed:

- Ensure inventories are stable domain/query outputs, not snapshots copied through loading state.
- Preserve per-file invalidation.

## Phase 9: runtime Project Introspection as enrichment

Plan intent:

- Runtime Project Introspection becomes optional enrichment.
- Inspector/cache/provider infrastructure moves out of semantic analysis.
- `ProjectEnrichmentState` expands and `enrichment` node is optional/scheduler-only.

Reference comparison:

- rust-analyzer build scripts/proc macros: runtime-derived facts become optional/degraded semantic inputs; running jobs/progress remain server-side.
- rust-analyzer flycheck: diagnostics/progress worker remains outside Salsa.
- Ruff/ty: project/system/search-path facts are Salsa; server scheduling/progress outside Salsa.

Assessment: **Conceptually aligned, but current `ProjectEnrichmentState` location may drift**.

Good:

- enrichment does not block static readiness;
- provider/cache/zipapp infrastructure stays outside `djls-project`/semantic;
- only translated drafts/issues cross into project domain;
- failures become explicit degraded facts.

Risk:

- applying enrichment through `ProjectLoadingState.enrichment` repeats the readiness singleton pattern.

Revision needed:

- Store stable enrichment facts as domain inputs/fields on the stable project root, or as tracked query results keyed by project/environment.
- Keep job progress, subprocess handles, cache warmup, and generations server-side.

## Phase 10: CLI, real LSP readiness, old Project removal

Plan intent:

- Audit CLI/LSP parity.
- Extend real LSP tests.
- Remove old semantic Project fact bag.
- Update docs.

Reference comparison:

- Aligned with cleanup and parity goals.

Assessment: **Aligned as a cleanup phase, but too late to fix root architecture**.

Risk:

- If `ProjectLoadingState` remains through phases 3B-9, Phase 10 will remove the old Project bag but replace it with another ambient project-readiness bag.

Revision needed:

- Do the architecture correction before 3B/3C, not in Phase 10.

## Cross-phase drift summary

The current plan mixes three readiness models:

1. **Stored readiness**: `ProjectLoadingState.{source_files, discovery, enrichment}`.
2. **Applied node/partition readiness**: `ProjectSourceFilesApplied.transition` and loading-node status.
3. **Derived query readiness**: `PythonSourceIndexOutcome`, `DjangoEnvironmentCandidatesOutcome`, inventories, and selections.

Reference evidence and the accepted decision choose one semantic truth model:

- Ruff/ty-style stable `djls_project::Project` root input + tracked fields;
- rust-analyzer-style server/CLI loading/progress/quiescence outside Salsa.

The current hybrid is the drift.

## Redraw-board answers

1. Target architecture: stable `djls_project::Project` root input.
2. Replacement for `Db::project_loading_state()`: `Db::project() -> Project`.
3. Stable root fields: workspace/source roots, source inventory, discovery facts/diagnostics, inventories, and enrichment facts as they land.
4. Loading/progress/quiescence may not affect tracked semantic query results. It is server/session/CLI state only.
5. Not-yet-loaded vs loaded-empty is a combination of stable project facts plus server in-flight state at the request boundary. Queries expose domain outcomes; UI/progress explains loading.
6. Failed reload keeps old facts by not writing replacement fields. Durable failure diagnostics may be written when they are Project Facts.
7. Installed-app/template-directory file readiness must have one semantic owner in project source inventory; node/milestone status is derived from apply/query outcomes.

## Plan repair direction

1. Freeze new feature implementation.
2. Use `architecture-decision-project-root.md` as the accepted model.
3. Insert and complete the stable-Project cleanup phase before Phase 3A4d/3B feature work.
4. Revise Phase 3B/3C/3D prose before implementing those phases because they currently entrench `ProjectLoadingState`.
5. Keep completed useful work where possible:
   - source/workspace primitives;
   - file materialization/merge invariants;
   - neutral loading runner;
   - LSP generation/progress controller.
6. Recast the runner as orchestration/progress over Project facts rather than the owner of semantic readiness.
