# Current Architecture Inventory: startup-rethink

## Purpose

This is a historical current-branch inventory captured before the Django Discovery Run cleanup in `docs/agents/startup-rethink/discovery-run-cleanup-plan.md`. It preserves evidence about the old loading graph shape and should not be read as the live code architecture after the cleanup.

The question here is not “what should we implement next?” It is:

- What is DJLS trying to do from first principles?
- What path does data follow from the LSP server down through the crates?
- Which modules have deep interfaces, and which ones leak too much process knowledge?
- Where does the current code already match the desired shape?
- Where is the shape still spread across too many seams?

## First-principles model

At a high level, DJLS startup is trying to build enough trustworthy project facts to answer template/Python/Django IDE requests without making LSP initialization depend on executing Django.

The natural pipeline is simple:

1. Get the rough list of relevant files.
2. Categorize files by path, extension, source root, and project/library role.
3. Build a layout index so paths can become modules, settings candidates, template names, and app roots.
4. Quickly parse Python files for imports, assignments, calls, class definitions, and function definitions.
5. Derive settings candidates, Django Environment candidates, and file-to-environment selection.
6. Use the environment to expand installed app roots and configured template directories.
7. Load the additional files that those facts reveal.
8. Build template file and template tag library inventories.
9. Run semantic extraction and IDE features from these project facts.
10. Optionally enrich with runtime Django inspection after static facts exist.

The PR is moving DJLS toward that pipeline. The current code now has an explicit loading graph, stable Project Facts input, source-file partitions, static settings/environment discovery, installed-app/template-directory expansion, and runtime enrichment as a late phase.

## Server-to-crate sequence

### LSP handshake

`initialize` builds a `Session`, stores it, and returns LSP capabilities. It does not run the project loading graph inline.

Evidence:

- `crates/djls-server/src/server.rs:123` — `initialize` entrypoint.
- `crates/djls-server/src/server.rs:129` — constructs `Session::new(&params)`.
- `crates/djls-server/src/server.rs:139` — stores the session.
- `crates/djls-server/src/server.rs:196` — `initialized` notification entrypoint.
- `crates/djls-server/src/server.rs:198` — starts project loading after initialization.
- `crates/djls-server/src/session.rs:56` — `Session::new` captures workspace roots and client options.
- `crates/djls-server/src/session.rs:62` — constructs `DjangoDatabase::new(workspace.overlay(), &client_settings)`.
- `crates/djls-server/src/session.rs:435` — test proves `Session::new` uses client settings without project config load.
- `crates/djls-server/src/session.rs:396` — test proves new sessions start with stable but unloaded Project Facts.

Interface shape:

- `djls-server` owns LSP protocol timing.
- `Session` owns workspace overlay, client info, roots, database, and document epoch.
- `DjangoDatabase::new` creates a virtual/unloaded Project Facts root rather than doing project discovery.

This is a good direction: the LSP handshake and Project Facts loading are separated.

### Startup generation and guarded loading

After `initialized`, the server starts a loading generation, captures a snapshot, and runs project loading on a blocking thread.

Evidence:

- `crates/djls-server/src/server.rs:56` — `start_project_loading` creates startup inputs.
- `crates/djls-server/src/server.rs:64` — captures `StartupRunInputs` from the current session.
- `crates/djls-server/src/server.rs:68` — spawns `run_startup_source_files`.
- `crates/djls-server/src/startup.rs:49` — `StartupController` owns generation state.
- `crates/djls-server/src/startup.rs:84` — `GenerationGuard::is_current` checks whether a run is still current.
- `crates/djls-server/src/startup.rs:86` — `GenerationGuard::apply` locks generation state and session before mutation.
- `crates/djls-server/src/startup.rs:812` — loading runs inside `spawn_blocking`.
- `crates/djls-server/src/startup.rs:828` — calls `run_loading_plan(LoadingPlan::phase3(), ...)`.

Interface shape:

- `djls-server` owns cancellation, staleness, progress, session locking, and async/blocking boundaries.
- `djls-project` owns the abstract loading graph and readiness statuses.
- `djls-db` owns concrete Salsa input materialization.

This split is conceptually right, but the loading executor currently knows many project-specific details.

### Loading graph

The loading graph is explicit and ordered.

Evidence:

- `crates/djls-project/src/loading/plan.rs:14` — `NodeId` variants.
- `crates/djls-project/src/loading/plan.rs:25` — `MilestoneId` variants.
- `crates/djls-project/src/loading/plan.rs:50` — `WorkspaceReady` milestone prerequisites.
- `crates/djls-project/src/loading/plan.rs:67` — `DjangoAppsReady` milestone prerequisites.
- `crates/djls-project/src/loading/plan.rs:97` — `LoadingPlan::phase3` node list.
- `crates/djls-project/src/loading/driver.rs:161` — `run_loading_plan`.
- `crates/djls-project/src/loading/driver.rs:177` — `SourceFileSet` execution branch.
- `crates/djls-project/src/loading/driver.rs:202` — `ProjectDiscoverySet` execution branch.
- `crates/djls-project/src/loading/driver.rs:227` — `PythonSourceModels` observation branch.
- `crates/djls-project/src/loading/driver.rs:247` — `EnvironmentDiscovery` observation branch.
- `crates/djls-project/src/loading/driver.rs:267` — `InstalledAppFiles` branch.
- `crates/djls-project/src/loading/driver.rs:281` — `TemplateDirectoryFiles` branch.
- `crates/djls-project/src/loading/driver.rs:295` — `Enrichment` branch.
- `crates/djls-project/src/loading/driver.rs:399` — milestone advancement derives from completed node statuses.

Current phase order:

| Order | Node | Kind |
| --- | --- | --- |
| 1 | `SourceFileSet` | load and apply first-party file inventory |
| 2 | `ProjectDiscoverySet` | load and apply config/env/interpreter/discovery roots |
| 3 | `PythonSourceModels` | observe static Python parse/index query |
| 4 | `EnvironmentDiscovery` | observe Django Environment candidate query |
| 5 | `InstalledAppFiles` | load and apply installed-app file partitions |
| 6 | `TemplateDirectoryFiles` | load and apply configured template-directory partitions |
| 7 | `Enrichment` | load and apply runtime template-library enrichment |

This graph is valuable because readiness is now a real state machine, not prose. The cost is that the effect trait exposes every graph step to both the LSP and CLI executors.

## Project Facts root

The stable project fact root is compact.

Evidence:

- `crates/djls-project/src/loading/state.rs:16` — `Project` is a Salsa input.
- `crates/djls-project/src/loading/state.rs:18` — `Project` contains `source_inventory`.
- `crates/djls-project/src/loading/state.rs:20` — `Project` contains `discovery`.
- `crates/djls-project/src/loading/state.rs:22` — `Project` contains `enrichment`.
- `crates/djls-project/src/loading/state.rs:26` — `Project::virtual_project` starts with source files not loaded, discovery absent, enrichment absent.
- `crates/djls-project/src/loading/state.rs:43` — `ProjectSourceInventory` is `Ready` or `Unavailable`.
- `crates/djls-project/src/discovery.rs:7` — `ProjectDiscovery` is `Absent`, `Ready`, or `Unavailable`.
- `crates/djls-project/src/enrichment.rs:8` — `ProjectEnrichment` is `Absent`, `Disabled`, `Fresh`, or `Unresolved`.

This is one of the deeper interfaces in the current design. Most downstream facts can be framed as derivations from these three axes:

- what files are known,
- what project/environment discovery is known,
- what runtime enrichment is known.

## Data shape changes by stage

### Stage 1: workspace roots to source roots

Inputs:

- LSP workspace folders or current-directory fallback.
- CLI project root.

Outputs:

- `SourceRootsPlan` with canonicalized source roots and duplicate-root issues.

Evidence:

- `crates/djls-server/src/session.rs:65` — session stores workspace roots.
- `crates/djls/src/commands/check.rs:148` — CLI resolves project root before loading.
- `crates/djls-project/src/loading/files.rs:43` — `build_source_roots` creates project roots.
- `crates/djls-project/src/loading/files.rs:48` — `build_source_roots_with_kind` supports project/library root kinds.
- `crates/djls-project/src/loading/files.rs:57` — roots are canonicalized.
- `crates/djls-project/src/loading/files.rs:63` — duplicate source roots produce `ProjectSourceFilesIssue::DuplicateRoot`.

Owner:

- `djls-project` owns root normalization and duplicate-root project facts.
- `djls-source` owns the neutral `SourceRoot` and `SourceRootId` types.

### Stage 2: source roots to rough file list

Inputs:

- `SourceRoot`s.
- `FileLoadPredicate`.
- `WalkOptions`.

Outputs:

- `FilesForRootsResult` with roots, discovered files, summary, and root issues.

Evidence:

- `crates/djls-project/src/loading/files.rs:89` — first-party file predicate includes templates, Python, JSON, TOML, YAML.
- `crates/djls-project/src/loading/files.rs:99` — first-party walk excludes venvs, node_modules, pycache, target.
- `crates/djls-project/src/loading/files.rs:116` — first-party request combines root plan, predicate, and walk options.
- `crates/djls-project/src/loading/files.rs:125` — converts project request into neutral workspace request.
- `crates/djls-workspace/src/file_loader.rs:95` — `load_files_for_roots` performs per-root loading.
- `crates/djls-workspace/src/file_loader.rs:101` — missing roots become `WorkspaceRootIssue::MissingRoot`.
- `crates/djls-workspace/src/file_loader.rs:122` — delegates path traversal to `walk_files`.
- `crates/djls-workspace/src/walk.rs:37` — `walk_files` recursively walks files with ignore/glob options.
- `crates/djls-source/src/file_set.rs:148` — `DiscoveredSourceFile` records path and root.
- `crates/djls-source/src/file_set.rs:165` — `DiscoveredSourceFile::kind` derives file kind from path.

Owner:

- `djls-workspace` owns neutral disk walking and root preflight.
- `djls-project` owns which files matter for the project loading stage.
- `djls-source` owns source roots, discovered files, loaded files, and file-set invariants.

### Stage 3: discovered files to materialized project source inventory

Inputs:

- First-party or partitioned source-file patches.
- Previous ready source inventory.

Outputs:

- `ProjectSourceInventory::Ready(ReadyProjectSourceFiles)` or a degraded/unavailable apply result.

Evidence:

- `crates/djls-project/src/loading/files.rs:329` — partition data merges selected discovered files by path.
- `crates/djls-project/src/loading/files.rs:351` — `FirstPartySourceFilePatch` stores partition, roots, files, summary, issues.
- `crates/djls-project/src/loading/files.rs:360` — `PartitionedSourceFileLoadOutcome` models ready, degraded, deferred, unavailable.
- `crates/djls-project/src/loading/files.rs:375` — `PartitionedSourceFilePatch` stores partition roots/files/issues.
- `crates/djls-db/src/db.rs:213` — `DjangoDatabase::apply_project_source_files` applies a project source-file update.
- `crates/djls-db/src/db.rs:236` — materialization starts from previous `SourceFileSetData`.
- `crates/djls-db/src/db.rs:257` — source roots are registered in the file registry.
- `crates/djls-db/src/db.rs:279` — existing file handles are preserved when possible.
- `crates/djls-db/src/db.rs:289` — new discovered paths get `get_or_create_file` handles.
- `crates/djls-db/src/db.rs:312` — `SourceFileSetData::new` enforces source-set invariants.
- `crates/djls-db/src/db.rs:220` — finalization is delegated back to `djls_project::finalize_project_source_files`.

Owner:

- `djls-project` owns partitions, readiness, merge/finalize policy.
- `djls-db` owns concrete file-handle materialization into Salsa inputs.
- `djls-source` owns `SourceFileSetData` invariants.

This boundary is mostly sound, but callers still perform the read-current → merge → apply choreography themselves.

### Stage 4: source inventory to layout index

Inputs:

- Ready project source inventory.

Outputs:

- `ProjectLayoutIndex` with roots, sorted files, and path-to-file map.

Evidence:

- `crates/djls-project/src/layout.rs:29` — `ProjectLayoutIndex` stores roots, files, and path map.
- `crates/djls-project/src/layout.rs:36` — layout is built from `SourceFileSetData`.
- `crates/djls-project/src/layout.rs:59` — can return a file path from a `File` handle.
- `crates/djls-project/src/layout.rs:67` — can return a `File` for a path.
- `crates/djls-project/src/layout.rs:72` — can find files by basename.
- `crates/djls-project/src/layout.rs:81` — can derive a module name for a path under a known root.
- `crates/djls-project/src/layout.rs:96` — `project_layout_index` derives from `ProjectSourceInventory`.

Owner:

- `djls-project` owns layout because layout is a Project Fact derivation, not a generic workspace fact.

### Stage 5: quick Python parse and index

Inputs:

- Tracked `File` inputs for Python files.
- Layout-derived module names.

Outputs:

- `PythonSourceModel` per file.
- `PythonSourceIndexOutcome` for project-wide parse/index readiness.

Evidence:

- `crates/djls-source/src/file.rs:12` — `File` is a Salsa input with path and revision.
- `crates/djls-source/src/file.rs:22` — `File::source` reads via the DB filesystem and revision.
- `crates/djls-source/src/file.rs:116` — `.py` files are `FileKind::Python`.
- `crates/djls-project/src/python/source.rs:16` — `PythonSourceModel` stores module resolution, parse status, imports, assignments, calls, classes, functions, and operations.
- `crates/djls-project/src/python/source.rs:72` — parse status is `Parsed` or `InvalidSyntax`.
- `crates/djls-project/src/python/source.rs:351` — `python_source_model` parses one file with Ruff.
- `crates/djls-project/src/python/source.rs:394` — `python_source_index` builds the project-wide index.
- `crates/djls-project/src/python/source.rs:398` — unavailable source inventory becomes an unindexed outcome.
- `crates/djls-project/src/python/source.rs:419` — only Python files are indexed.
- `crates/djls-project/src/python/source.rs:425` — layout module names are attached to per-file models.

Owner:

- `djls-project` owns the quick source-derived Project Fact model.
- `djls-semantic` owns deeper extraction from selected model/tag modules.

This split is useful: cheap syntax/static facts are project inventory; semantic extraction is a consumer.

### Stage 6: config and discovery

Inputs:

- Workspace roots.
- Client settings overrides.
- Project config files.
- env file.
- interpreter configuration.

Outputs:

- `ProjectDiscoverySetData`, then `ProjectDiscovery::Ready` or unavailable.

Evidence:

- `crates/djls-conf/src/lib.rs:228` — `Settings::load(project_root, overrides)` is the config loader.
- `crates/djls-conf/src/lib.rs:234` — config source layers are collected with errors.
- `crates/djls-conf/src/lib.rs:251` — config crate builds merged config.
- `crates/djls-conf/src/lib.rs:253` — merged config deserializes into `Settings`.
- `crates/djls-conf/src/lib.rs:257` — caller-provided overrides are applied.
- `crates/djls-project/src/loading/settings.rs:125` — `build_project_discovery_data` builds project discovery data.
- `crates/djls-project/src/loading/settings.rs:138` — project discovery calls `Settings::load` with client settings as overrides.
- `crates/djls-project/src/loading/settings.rs:146` — config load failures become `ProjectDiscoveryIssue::ConfigLoadFailed` and fallback to client settings.
- `crates/djls-project/src/loading/settings.rs:150` — interpreter discovery is recorded.
- `crates/djls-project/src/loading/settings.rs:152` — explicit Django settings module becomes a seed.
- `crates/djls-project/src/loading/settings.rs:155` — configured `django_environments` become environment seeds.
- `crates/djls-project/src/loading/settings.rs:166` — configured Python paths are rooted.
- `crates/djls-project/src/loading/settings.rs:173` — env-file loading is included in discovery data.
- `crates/djls-db/src/db.rs:166` — DB applies `ProjectDiscoverySetData`.
- `crates/djls-db/src/db.rs:185` — DB materializes root discovery data as Salsa `RootDiscoveryInput`s.

Owner:

- `djls-conf` owns schema/config file loading.
- `djls-project` owns project discovery policy and project-shaped issues.
- `djls-db` owns materialization into Salsa inputs.

This is a cleaner boundary than the older shape: config loading is not allowed to own server tolerance policy, and project discovery records config failures as Project Facts issues.

### Stage 7: settings candidates and Django Environment candidates

Inputs:

- Project discovery roots.
- Layout index.
- Python source model for `manage.py`.
- Conventional `settings.py` files.

Outputs:

- `SettingsCandidateOutcome`.
- `DjangoEnvironmentCandidatesOutcome`.
- `EnvironmentSelection` for individual files.

Evidence:

- `crates/djls-project/src/settings/candidates.rs:13` — `SettingsCandidate` stores module, optional file, source, and origin.
- `crates/djls-project/src/settings/candidates.rs:47` — sources include explicit config, configured environment, environment variable, manage.py default, conventional module.
- `crates/djls-project/src/settings/candidates.rs:75` — `settings_candidates` collects all candidate kinds.
- `crates/djls-project/src/settings/candidates.rs:131` — discovery candidates come from project discovery roots.
- `crates/djls-project/src/settings/candidates.rs:183` — `manage.py` candidates are parsed from `os.environ.setdefault("DJANGO_SETTINGS_MODULE", ...)`.
- `crates/djls-project/src/settings/candidates.rs:221` — conventional candidates come from files named `settings.py`.
- `crates/djls-project/src/environments.rs:66` — environment candidate outcomes include ready, ambiguous, unavailable, and deferred.
- `crates/djls-project/src/environments.rs:124` — `django_environment_candidates` lowers settings candidates into environment candidates.
- `crates/djls-project/src/environments.rs:171` — `environment_for_file` selects candidates by file path and environment root.
- `crates/djls-project/src/environments.rs:215` — equal-length matching roots produce an ambiguous file selection.

Owner:

- `djls-project` owns Django Environment discovery and selection.

This is a core Project Facts boundary. It should stay below `djls-semantic` and `djls-ide` so features do not each invent their own environment selection rules.

### Stage 8: static Django settings projection

Inputs:

- Selected Django Environment.
- Settings module resolution.
- Python source model operations.

Outputs:

- `DjangoSettings` with installed apps, template settings, and issues.

Evidence:

- `crates/djls-project/src/settings/composition.rs:12` — `DjangoSettings` stores installed apps, template settings, and issues.
- `crates/djls-project/src/settings/composition.rs:122` — `django_settings` derives settings for an environment.
- `crates/djls-project/src/settings/composition.rs:137` — settings module is found from environment candidates.
- `crates/djls-project/src/settings/composition.rs:159` — unresolved settings modules produce typed settings issues.
- `crates/djls-project/src/settings/composition.rs:183` — settings file parse errors become settings issues.
- `crates/djls-project/src/settings/composition.rs:190` — supported Python operations are applied into the settings model.

Owner:

- `djls-project` owns the static Django settings projection because it is a Project Fact used to load apps/templates.
- The Python parser supplies operations; it does not own Django settings meaning.

### Stage 9: installed app files and template directory files

Inputs:

- Django Environment candidates.
- Static `DjangoSettings`.
- Module resolver.
- AppConfig static assignments.

Outputs:

- Partitioned source-file patches for installed apps and configured template directories.

Evidence:

- `crates/djls-project/src/apps.rs:40` — installed app entries are resolved from settings segments.
- `crates/djls-project/src/apps.rs:67` — app package entries resolve via module resolution.
- `crates/djls-project/src/apps.rs:95` — AppConfig entries parse static `name`, `label`, and `path` assignments.
- `crates/djls-project/src/apps.rs:185` — `installed_apps` derives app facts for an environment.
- `crates/djls-project/src/apps.rs:210` — installed app roots are loaded as `LibrarySearchPath` roots.
- `crates/djls-project/src/apps.rs:250` — `installed_app_file_load_outcome` drives installed-app loading.
- `crates/djls-project/src/apps.rs:276` — resolved apps become load roots; unresolved apps become gaps.
- `crates/djls-project/src/apps.rs:329` — installed app file predicate loads app-relevant Python/templates/templatetags paths.
- `crates/djls-project/src/templates/loading.rs:25` — configured template-directory load request.
- `crates/djls-project/src/templates/loading.rs:32` — template directory files use project roots and template-file predicate.
- `crates/djls-project/src/templates/loading.rs:51` — roots come from `TEMPLATES[*].DIRS`.
- `crates/djls-project/src/templates/loading.rs:80` — `template_directory_file_load_outcome` drives configured-template-directory loading.

Owner:

- `djls-project` owns installed-app/template-directory expansion.
- `djls-workspace` remains the neutral file loader.

This is the current center of the PR: static Project Facts reveal more roots, and those roots feed back into the source inventory as partitions.

### Stage 10: template and library inventories

Inputs:

- Ready source inventory with partition readiness.
- Django settings template config.
- Installed app facts.
- Runtime enrichment, if present.

Outputs:

- `TemplateFileInventory`.
- `TemplateTagLibraryInventory`.
- `LoadableTemplateLibraryInventory`.

Evidence:

- `crates/djls-project/src/templates/inventory.rs:25` — `TemplateDirectory` stores path and source.
- `crates/djls-project/src/templates/inventory.rs:38` — template directory entries distinguish discovered, unknown settings dir, deferred, unavailable, and stale.
- `crates/djls-project/src/templates/inventory.rs:72` — `ProjectTemplate` stores path, template name, file, and directory.
- `crates/djls-project/src/templates/inventory.rs:238` — `template_files` derives template files for a selected environment.
- `crates/djls-project/src/templates/inventory.rs:244` — unavailable source inventory defers discovered directories.
- `crates/djls-project/src/templates/inventory.rs:260` — directory readiness is checked against source inventory partition readiness.
- `crates/djls-project/src/templates/inventory.rs:273` — template names are relative to the template directory.
- `crates/djls-project/src/templates/inventory.rs:289` — `template_tag_libraries` derives builtins, installed-app libraries, and settings libraries.
- `crates/djls-project/src/templates/inventory.rs:338` — `loadable_template_libraries` merges static inventory with runtime enrichment.
- `crates/djls-project/src/templates/inventory.rs:391` — directory readiness maps partition readiness to discovered/deferred/unavailable/stale entries.
- `crates/djls-project/src/templates/inventory.rs:451` — configured settings dirs and installed app template dirs become directory entries.

Owner:

- `djls-project` owns physical template inventory and library availability.
- `djls-semantic` owns lowering those facts into semantic `Template` entities and template validation/reference behavior.

This boundary is improving. `djls_semantic::resolution` now calls `djls_project::template_files` directly and constructs semantic `Template` values only at the use sites.

### Stage 11: semantic and IDE consumers

Inputs:

- Project Facts through `djls_project::Db`.
- Template parser output.
- Tag specs, filter arities, model graph.
- Template/library inventories.

Outputs:

- diagnostics, completions, hover, goto definition, references, folding, symbols, formatting.

Evidence:

- `crates/djls-semantic/src/db.rs:15` — semantic DB extends `djls_project::Db`.
- `crates/djls-semantic/src/queries.rs:21` — tag specs derive from project template-tag modules.
- `crates/djls-semantic/src/queries.rs:53` — filter arity specs derive from project template-tag modules.
- `crates/djls-semantic/src/queries.rs:69` — model graph derives from project model modules.
- `crates/djls-project/src/python/inventory.rs:33` — model modules derive from ready source inventory and known environments.
- `crates/djls-project/src/python/inventory.rs:65` — template tag modules derive from project template tag library inventory.
- `crates/djls-semantic/src/resolution.rs:56` — static template resolution consumes project template file inventory.
- `crates/djls-semantic/src/resolution.rs:97` — public template resolution parses a name and selects an environment for the source file.
- `crates/djls-semantic/src/resolution.rs:126` — template libraries for a file come from project loadable template libraries.
- `crates/djls-semantic/src/resolution.rs:411` — reference search selects an environment for the source file.
- `crates/djls-semantic/src/resolution.rs:445` — static template reference index scans project template inventory.
- `crates/djls-semantic/src/lib.rs:85` — template validation entrypoint parses and validates a file.
- `crates/djls-ide/src/diagnostics.rs:87` — IDE diagnostics trigger semantic validation and convert accumulators to LSP diagnostics.
- `crates/djls-ide/src/navigation.rs:11` — goto definition consumes semantic template resolution.
- `crates/djls-ide/src/navigation.rs:46` — references consume semantic template reference search.
- `crates/djls-ide/src/hover.rs:15` — hover consumes file-specific template libraries and template resolution.
- `crates/djls-ide/src/completions.rs:104` — completion entrypoint receives template libraries, tag specs, and available symbols.
- `crates/djls-server/src/server.rs:235` — LSP completion prepares semantic/project data before calling IDE completions.
- `crates/djls-server/src/server.rs:310` — LSP hover delegates to IDE hover.
- `crates/djls-server/src/server.rs:337` — LSP diagnostics delegates to IDE diagnostics.
- `crates/djls-server/src/server.rs:419` — LSP goto definition delegates to IDE navigation.
- `crates/djls-server/src/server.rs:443` — LSP references delegates to IDE navigation.

Owner:

- `djls-project` owns project inventories and environment selection.
- `djls-semantic` owns parsed-template meaning, semantic extraction, validation, and semantic query adapters.
- `djls-ide` owns LSP-shaped feature outputs.
- `djls-server` owns protocol plumbing, request routing, and session access.

## Runtime enrichment

Runtime enrichment is a late project-facts phase, not the source of startup truth.

Evidence:

- `crates/djls-project/src/enrichment.rs:8` — enrichment distinguishes absent, disabled, fresh, unresolved.
- `crates/djls-project/src/enrichment.rs:14` — runtime template libraries are typed as `BTreeMap<LibraryName, PyModuleName>`.
- `crates/djls-project/src/enrichment/runtime.rs:57` — `load_runtime_project_enrichment` builds a runtime request or returns unresolved.
- `crates/djls-project/src/enrichment/runtime.rs:96` — runtime request requires ready project discovery.
- `crates/djls-project/src/enrichment/runtime.rs:105` — runtime request requires ready environment candidates.
- `crates/djls-project/src/enrichment/runtime.rs:134` — request carries Python path, root, settings module, pythonpath, env vars.
- `crates/djls-project/src/enrichment/runtime.rs:170` — inspector command uses the discovered interpreter.
- `crates/djls-db/src/db.rs:145` — DB loads runtime project enrichment by calling `djls_project::load_runtime_project_enrichment`.
- `crates/djls-db/src/db.rs:149` — DB applies enrichment by setting the Project input only if it changed.

This is the right conceptual placement: runtime Django may improve answers, but static project facts should exist without it.

## Outside-world interfaces

### LSP client interface

`djls-server` is the only LSP-facing crate. It owns:

- `initialize` and `initialized` timing,
- capability declaration,
- document open/change/save/close,
- completion/hover/diagnostic/navigation/reference request routing,
- configuration change reload and restart policy,
- progress reporting and startup generation cancellation.

Evidence:

- `crates/djls-server/src/server.rs:123` — initialize.
- `crates/djls-server/src/server.rs:196` — initialized.
- `crates/djls-server/src/server.rs:203` — shutdown.
- `crates/djls-server/src/server.rs:207` — did_open.
- `crates/djls-server/src/server.rs:235` — completion.
- `crates/djls-server/src/server.rs:306` — hover.
- `crates/djls-server/src/server.rs:326` — diagnostic.
- `crates/djls-server/src/server.rs:411` — goto definition.
- `crates/djls-server/src/server.rs:435` — references.
- `crates/djls-server/src/server.rs:506` — configuration change.

### Filesystem interface

`djls-workspace` and `djls-source` together form the file substrate.

Evidence:

- `crates/djls-workspace/src/workspace.rs:28` — `Workspace::new` creates buffers and overlay filesystem.
- `crates/djls-workspace/src/workspace.rs:43` — overlay checks buffers first, then disk.
- `crates/djls-workspace/src/workspace.rs:60` — open document creates a Salsa file and stores buffer content.
- `crates/djls-workspace/src/workspace.rs:73` — save bumps file revision.
- `crates/djls-workspace/src/workspace.rs:78` — update document bumps revision and updates buffer.
- `crates/djls-workspace/src/workspace.rs:105` — close removes buffer and bumps revision.
- `crates/djls-source/src/db.rs:7` — source DB trait exposes files and file reading.
- `crates/djls-source/src/db.rs:20` — `get_or_create_file` creates tracked file handles.
- `crates/djls-source/src/db.rs:25` — `bump_file_revision` invalidates dependent queries.

### Config interface

`djls-conf` owns config schema and load errors. It does not own whether a caller treats load errors as fatal.

Evidence:

- `crates/djls-conf/src/lib.rs:50` — `SettingsLoadError` is typed.
- `crates/djls-conf/src/lib.rs:228` — `Settings::load` returns `Result<Settings, Vec<SettingsLoadError>>`.
- `crates/djls/src/commands/check.rs:150` — CLI treats config load failure as command failure.
- `crates/djls-project/src/loading/settings.rs:138` — project discovery treats config load failure as a Project Discovery issue and continues with client settings.
- `crates/djls-server/src/server.rs:513` — config reload logs errors and keeps the server alive.

### Python/Django runtime interface

Runtime Django inspection is isolated in `djls-project` enrichment.

Evidence:

- `crates/djls-project/src/enrichment/runtime.rs:160` — inspector zipapp bytes are embedded.
- `crates/djls-project/src/enrichment/runtime.rs:170` — inspector command is spawned with project root, settings, pythonpath, and env vars.
- `crates/djls-project/src/enrichment/runtime.rs:225` — inspector process has a timeout.

## Deep modules

### `djls-source`

Depth:

- It exposes a small DB trait and source/file primitives.
- It hides file handle registration, file revision invalidation, and line/source derivation behind a compact interface.

Evidence:

- `crates/djls-source/src/db.rs:7` — source DB trait.
- `crates/djls-source/src/file.rs:12` — tracked `File` input.
- `crates/djls-source/src/file.rs:22` — tracked `File::source` query.
- `crates/djls-source/src/file.rs:28` — tracked `File::line_index` query.
- `crates/djls-source/src/file_set.rs:17` — `SourceFileSetData::new` enforces root/file invariants.

### `djls-workspace`

Depth:

- It owns buffer overlay behavior and neutral file walking.
- It does not know Django concepts.

Evidence:

- `crates/djls-workspace/src/workspace.rs:1` — workspace facade explicitly manages buffers and filesystem components.
- `crates/djls-workspace/src/file_loader.rs:13` — neutral file-loading request preserves caller-provided roots.
- `crates/djls-workspace/src/walk.rs:37` — neutral walk helper.

### `djls-conf`

Depth:

- It owns schema, source layering, TOML/pyproject loading, and typed load errors.
- It leaves tolerance/fatal policy to callers.

Evidence:

- `crates/djls-conf/src/lib.rs:119` — config layers are either file-backed or TOML string-backed.
- `crates/djls-conf/src/lib.rs:139` — config sources include user config, pyproject, `.djls.toml`, and `djls.toml`.
- `crates/djls-conf/src/lib.rs:228` — one settings loader.

### `Project` facts root

Depth:

- The Project input itself is small and stable.
- Most derivations can hang off source inventory, discovery, and enrichment.

Evidence:

- `crates/djls-project/src/loading/state.rs:16` — `Project` Salsa input.
- `crates/djls-project/src/loading/state.rs:18` — source inventory.
- `crates/djls-project/src/loading/state.rs:20` — discovery.
- `crates/djls-project/src/loading/state.rs:22` — enrichment.

## Shallow or leaky interfaces

These are observations, not prescriptions.

### Loading has no single deep owner yet

The loading graph lives in `djls-project`, but the effect implementations in the LSP server and CLI both know the detailed choreography:

- build source roots,
- build requests,
- call workspace loader,
- read current ready source inventory,
- merge patches,
- call DB apply methods,
- observe project queries,
- load partitioned patches,
- apply each patch.

Evidence:

- `crates/djls-project/src/loading/effects.rs:40` — `LoadingEffects` exposes a wide step protocol.
- `crates/djls-server/src/startup.rs:884` — LSP executor implements the whole protocol.
- `crates/djls-server/src/startup.rs:893` — LSP executor builds first-party load request.
- `crates/djls-server/src/startup.rs:903` — LSP executor reads current inventory, merges, and applies.
- `crates/djls-server/src/startup.rs:931` — LSP executor builds project discovery data.
- `crates/djls-server/src/startup.rs:956` — LSP executor observes Python source index from a DB snapshot.
- `crates/djls-server/src/startup.rs:983` — LSP executor observes environment candidates.
- `crates/djls-server/src/startup.rs:1010` — LSP executor loads installed-app patches.
- `crates/djls-server/src/startup.rs:1021` — LSP executor loads template-directory patches.
- `crates/djls-server/src/startup.rs:1034` — LSP executor applies partitioned source-file patches.
- `crates/djls/src/loading.rs:33` — CLI executor implements the same trait.
- `crates/djls/src/loading.rs:42` — CLI executor builds first-party load request.
- `crates/djls/src/loading.rs:51` — CLI executor reads current inventory, merges, and applies.
- `crates/djls/src/loading.rs:63` — CLI executor builds project discovery data.
- `crates/djls/src/loading.rs:77` — CLI executor observes Python source index.
- `crates/djls/src/loading.rs:89` — CLI executor observes environment candidates.
- `crates/djls/src/loading.rs:97` — CLI executor loads installed-app patches.
- `crates/djls/src/loading.rs:104` — CLI executor loads template-directory patches.

This is the strongest Ousterhout-style smell in the current code: the important algorithm is distributed across a driver, a trait, two executors, DB materialization, and project free functions.

The desired future shape can still preserve LSP-specific cancellation and CLI-specific fatal policy, but the repeated project-loading algorithm is not yet hidden behind a deep interface.

### `djls-project` crate root remains broad

The crate root exports many things that are probably real public boundary types, plus many things that look like current loading machinery.

Evidence:

- `crates/djls-project/src/lib.rs:15` — exports installed-app file loading outcome.
- `crates/djls-project/src/lib.rs:21` — exports discovery issues and discovery set types.
- `crates/djls-project/src/lib.rs:34` — exports env-file loading.
- `crates/djls-project/src/lib.rs:39` — exports loading root/request/merge/finalize functions.
- `crates/djls-project/src/lib.rs:45` — exports loading effects and run-control types.
- `crates/djls-project/src/lib.rs:57` — exports source-file update/materialization types.
- `crates/djls-project/src/lib.rs:75` — exports Python module/index queries.
- `crates/djls-project/src/lib.rs:80` — exports template inventory/loading queries.

Usage evidence:

- `crates/djls-server/src/startup.rs:8` — server imports many loading helpers directly.
- `crates/djls/src/loading.rs:3` — CLI imports many of the same helpers directly.
- `crates/djls-db/src/db.rs:15` — DB imports Project Facts and materialization types.
- `crates/djls-semantic/src/queries.rs:21` — semantic queries consume project module inventories.
- `crates/djls-semantic/src/resolution.rs:63` — semantic resolution consumes project template inventory.
- `crates/djls/src/commands/check.rs:337` — CLI check consumes project environment selection for warnings.

The broad facade does not prove every export is wrong. It does show the public API is carrying several different audiences at once:

- stable Project Facts DB trait and root input,
- loading graph execution,
- DB materialization data,
- project discovery data,
- semantic consumer queries,
- CLI/server helper functions,
- tests and fixtures.

That makes it harder to tell which names are stable concepts and which are current implementation seams.

### `LoadingEffects` is a protocol, not a domain interface

`LoadingEffects` reads like a serialized script rather than a narrow capability interface.

Evidence:

- `crates/djls-project/src/loading/effects.rs:40` — trait begins.
- `crates/djls-project/src/loading/effects.rs:42` — source file loading hook.
- `crates/djls-project/src/loading/effects.rs:43` — source file apply hook.
- `crates/djls-project/src/loading/effects.rs:46` — project discovery load hook.
- `crates/djls-project/src/loading/effects.rs:48` — project discovery apply hook.
- `crates/djls-project/src/loading/effects.rs:51` — Python source index observation hook.
- `crates/djls-project/src/loading/effects.rs:54` — Django environment candidates observation hook.
- `crates/djls-project/src/loading/effects.rs:57` — installed app file load hook.
- `crates/djls-project/src/loading/effects.rs:58` — template directory file load hook.
- `crates/djls-project/src/loading/effects.rs:59` — partitioned source-file apply hook.
- `crates/djls-project/src/loading/effects.rs:63` — enrichment load hook.
- `crates/djls-project/src/loading/effects.rs:64` — enrichment apply hook.

The trait lets the driver be runtime-agnostic, which is useful. But the cost is that every executor becomes responsible for understanding the whole sequence.

### `djls-db` is an intentional materialization seam, but it also sees project construction details

`DjangoDatabase` probably should own Salsa input materialization because it is the concrete DB. The current boundary is still worth documenting because it constructs project discovery inputs and source-file-set materialization.

Evidence:

- `crates/djls-db/src/db.rs:56` — `DjangoDatabase` stores filesystem, file registry, settings, project facts, semantic settings revision, and Salsa storage.
- `crates/djls-db/src/db.rs:166` — `apply_project_discovery_data` accepts project discovery data.
- `crates/djls-db/src/db.rs:185` — it constructs `RootDiscoveryInput` Salsa inputs.
- `crates/djls-db/src/db.rs:195` — it constructs a `ProjectDiscoverySet`.
- `crates/djls-db/src/db.rs:213` — `apply_project_source_files` materializes source-file updates.
- `crates/djls-db/src/db.rs:371` — implements `djls_project::Db`.
- `crates/djls-db/src/db.rs:381` — implements `djls_source::Db`.
- `crates/djls-db/src/db.rs:392` — implements `djls_semantic::Db`.

This may be the right boundary. The important thing is to name it clearly: `djls-db` is the anti-corruption layer between pure project facts and concrete Salsa/file-system storage.

### Semantic template-library state still has old and new paths

The newer path is file/environment-aware:

- server/IDE asks for the current file,
- semantic selects a project environment,
- semantic lowers project loadable template libraries into semantic `TemplateLibraries`.

The older path is DB-global:

- `SemanticDb::template_libraries()` still exists,
- production DB returns `TemplateLibraries::empty_ref()`,
- some semantic/scoping/test/bench code still uses the DB-global interface.

Evidence:

- `crates/djls-semantic/src/db.rs:33` — `SemanticDb::template_libraries` remains in the trait.
- `crates/djls-db/src/db.rs:412` — production DB returns `TemplateLibraries::empty_ref()`.
- `crates/djls-semantic/src/resolution.rs:126` — `template_libraries_for_file` is the project/environment-aware adapter.
- `crates/djls-semantic/src/resolution.rs:137` — it starts from `db.template_libraries().clone()` and lowers project libraries into it.
- `crates/djls-semantic/src/lib.rs:96` — `validate_template_file` uses `template_libraries_for_file`.
- `crates/djls-semantic/src/lib.rs:123` — `validate_nodelist` still validates with `db.template_libraries()`.
- `crates/djls-semantic/src/scoping.rs:48` — `compute_symbol_index` still uses `db.template_libraries()`.
- `crates/djls-ide/src/hover.rs:17` — hover falls back to `db.template_libraries().clone()` when file-specific libraries are unavailable.
- `crates/djls-semantic/src/testing.rs:268` — test DB supplies template libraries.
- `crates/djls-bench/src/db.rs:146` — bench DB supplies template libraries.

This is a migration seam. The doc should treat it as coexistence, not necessarily a bug. The authoritative runtime path for file-specific IDE behavior is moving toward `template_libraries_for_file`.

### IDE completion still takes many separate pieces

Completion is currently passed source text, position, encoding, file kind, template libraries, tag specs, available symbols, and snippet support as separate arguments.

Evidence:

- `crates/djls-ide/src/completions.rs:104` — `handle_completion` entrypoint.
- `crates/djls-ide/src/completions.rs:106` — source text argument.
- `crates/djls-ide/src/completions.rs:107` — LSP position.
- `crates/djls-ide/src/completions.rs:108` — position encoding.
- `crates/djls-ide/src/completions.rs:109` — file kind.
- `crates/djls-ide/src/completions.rs:110` — optional template libraries.
- `crates/djls-ide/src/completions.rs:111` — optional tag specs.
- `crates/djls-ide/src/completions.rs:112` — optional available symbols.
- `crates/djls-ide/src/completions.rs:113` — snippet support.
- `crates/djls-server/src/server.rs:253` — server prepares all completion inputs.
- `crates/djls-server/src/server.rs:262` — server asks semantic for template libraries.
- `crates/djls-server/src/server.rs:264` — server asks DB for tag specs.
- `crates/djls-server/src/server.rs:268` — server computes available symbols for load-scoped completions.
- `crates/djls-server/src/server.rs:288` — server calls `djls_ide::handle_completion`.

This is a presentation-layer smell rather than a Project Facts smell: the server must assemble too much feature-specific semantic context before calling the IDE layer.

## Current tests that lock in the shape

- `crates/djls-server/src/session.rs:396` — new sessions initialize stable Project Facts.
- `crates/djls-server/src/session.rs:435` — new sessions do not load project config.
- `crates/djls-server/src/startup.rs:1557` — Python source model requests can proceed while startup is running.
- `crates/djls-project/src/loading/plan.rs:354` — phase plan order is tested.
- `crates/djls-project/src/loading/driver.rs:753` — workspace and Django apps milestones are observed in loading tests.
- `crates/djls-project/src/loading/driver.rs:845` — `DjangoAppsReady` degrades for deferred app files.
- `crates/djls-project/src/settings/candidates.rs:392` — settings candidates collect explicit env, manage.py, and conventional modules.
- `crates/djls-project/src/templates/inventory.rs:717` — configured template directory is deferred until loaded.
- `crates/djls-project/src/templates/inventory.rs:740` — loaded configured directory files are listed.
- `crates/djls-project/src/templates/inventory.rs:786` — loaded empty template directory is not deferred.
- `crates/djls-project/src/templates/inventory.rs:810` — built-in and installed tag libraries are included.
- `crates/djls-semantic/src/resolution.rs:218` — unloaded configured template directory yields deferred template resolution.
- `crates/djls-semantic/src/resolution.rs:329` — template validation uses static inventory.
- `crates/djls-semantic/src/resolution.rs:368` — loaded configured template resolves statically.

## What the current architecture is trying to achieve

The current branch is trying to make Project Facts a deep module.

A useful restatement:

`djls-project` should answer questions like:

- What files are part of this project?
- Which roots are first-party, configured template directories, or installed apps?
- Which files are Python/template/config-like?
- Which Python files are potential settings modules, model modules, or templatetag modules?
- Which Django Environment candidates exist?
- Which environment applies to this file?
- What do static settings say about installed apps and templates?
- Which installed app and template-directory files should be loaded next?
- Which templates and template libraries are known, deferred, unavailable, or stale?
- What runtime enrichment, if any, is available?

`djls-semantic` should answer questions like:

- Given a project-selected file/environment, what does this template reference resolve to?
- Which template references target this template?
- Which tag/filter/model facts can be extracted from the project-selected Python files?
- What validation errors does this parsed template have under the available project facts?

`djls-ide` should answer questions like:

- What LSP completions/hover/diagnostics/navigation/references should the user see?

`djls-server` should answer questions like:

- When should startup run?
- How is startup cancelled or superseded?
- How do requests access a coherent session snapshot?
- How do project fact changes cause diagnostics to republish?

The exhausting part is that these responsibilities are not fully hidden yet. The code has many of the right concepts, but some important workflows still require multiple crates to know the same choreography.

## Current pressure points

### 1. Loading algorithm spread

The project-loading algorithm is split across:

- `LoadingPlan`,
- `run_loading_plan`,
- `LoadingEffects`,
- `LspLoadingExecutor`,
- `CliLoadingExecutor`,
- `DjangoDatabase` apply methods,
- project free functions for load outcomes and merges.

This makes the startup path hard to hold in your head even though the first-principles sequence is simple.

### 2. Public API surface is larger than the conceptual boundary

`djls-project` exports concepts from several layers at once. Some are stable domain concepts; others are loading implementation details. Because they are all crate-root exports, it is hard to tell which names are intentional long-term interfaces.

### 3. Environment selection is centralized, but consumers still see many degraded states

`environment_for_file` is the right central seam. But callers still each decide what to do with selected/unknown/ambiguous/deferred behavior.

Evidence:

- `crates/djls-semantic/src/resolution.rs:112` — template resolution maps unknown/ambiguous environment selection to `Deferred`.
- `crates/djls-semantic/src/resolution.rs:128` — template libraries return `None` on unknown/ambiguous environment.
- `crates/djls-semantic/src/resolution.rs:420` — reference search returns empty on unknown/ambiguous environment.
- `crates/djls/src/commands/check.rs:337` — CLI check emits a warning for ambiguous environment selection.

### 4. Template-library migration is incomplete by design

`template_libraries_for_file` is the newer project-aware path. `db.template_libraries()` remains for fixtures, bench, fallbacks, and older semantic paths.

This should be documented as a migration seam so future cleanup does not accidentally preserve both as equal authorities.

### 5. DB materialization boundary needs a crisp name

`djls-db` is the concrete place where pure project data becomes Salsa input state. That is probably the right responsibility. The code will be easier to reason about if the docs name it explicitly as the materialization/anti-corruption layer between project facts and runtime storage.

## Current ownership matrix

| Concern | Current owner | Evidence | Notes |
| --- | --- | --- | --- |
| LSP protocol lifecycle | `djls-server` | `server.rs:123`, `server.rs:196` | Good boundary. |
| Session state | `djls-server::Session` | `session.rs:35` | Owns workspace, client info, roots, DB. |
| Config schema/load | `djls-conf` | `lib.rs:228` | Caller owns strict/tolerant policy. |
| Config reload policy | `djls-server` | `server.rs:506` | Server logs errors and stays alive. |
| CLI config strictness | `djls` | `check.rs:150` | CLI fails command on load errors. |
| Buffer overlay | `djls-workspace` | `workspace.rs:28` | Good substrate boundary. |
| Neutral file walking | `djls-workspace` | `file_loader.rs:95`, `walk.rs:37` | Does not know Django. |
| Source file handles/invariants | `djls-source` | `file.rs:12`, `file_set.rs:17` | Deep, small interface. |
| Project source partitions/readiness | `djls-project` | `loading/files.rs:351`, `loading/files.rs:360` | Correct domain owner. |
| Project facts root | `djls-project` | `loading/state.rs:16` | Compact and deep. |
| Salsa materialization | `djls-db` | `db.rs:213`, `db.rs:166` | Needs explicit boundary name. |
| Loading graph | `djls-project` | `loading/plan.rs:97`, `driver.rs:161` | Good concept, wide effect interface. |
| LSP loading execution | `djls-server` | `startup.rs:884` | Owns cancellation/progress/session locking. |
| CLI loading execution | `djls` | `loading.rs:33` | Duplicates project loading choreography. |
| Settings candidates | `djls-project` | `settings/candidates.rs:75` | Good Project Fact. |
| Environment selection | `djls-project` | `environments.rs:171` | Good central seam. |
| Static Django settings projection | `djls-project` | `settings/composition.rs:122` | Good Project Fact. |
| Installed app expansion | `djls-project` | `apps.rs:250` | Good Project Fact. |
| Template directory expansion | `djls-project` | `templates/loading.rs:80` | Good Project Fact. |
| Template inventory | `djls-project` | `templates/inventory.rs:238` | Good Project Fact. |
| Template reference/validation semantics | `djls-semantic` | `resolution.rs:97`, `lib.rs:85` | Uses project facts. |
| LSP feature presentation | `djls-ide` | `diagnostics.rs:87`, `navigation.rs:11`, `completions.rs:104` | Some context assembly still lives in server. |
| Runtime enrichment | `djls-project` + `djls-db` | `enrichment/runtime.rs:57`, `db.rs:145` | Late phase, not startup truth. |

## A concise mental model

The code is moving toward this shape:

```text
LSP / CLI
  owns protocol and fatal/tolerant policy

DjangoDatabase
  owns concrete Salsa storage and file-handle materialization

Project Facts
  owns project file inventory, discovery, environments, static settings,
  installed apps, template directories, template inventories, enrichment state

Semantic
  owns parsing-derived meaning, extraction, validation, resolution adapters

IDE
  owns LSP-shaped presentation
```

The current branch is closest to that model at the data-model level. It is less close at the execution-interface level, because startup loading still requires server and CLI executors to know too much of the project loading algorithm.

## Open questions for the next design pass

These are documentation/design questions, not implementation tasks.

1. Which `djls-project` crate-root exports are stable public Project Facts API, and which are loading implementation details?
2. Should the startup loading algorithm have a deeper project-owned interface, with LSP/CLI supplying only runtime policy hooks?
3. Is `DjangoDatabase` formally the Salsa materialization boundary for Project Facts? If yes, document that as an intentional anti-corruption layer.
4. Which remaining `SemanticDb::template_libraries()` consumers are fixture/bench-only, and which are production migration leftovers?
5. Should environment-selection degradation be mapped once below IDE presentation instead of separately in resolution, libraries, references, diagnostics, completion, and CLI warning paths?
6. Should completion/hover/diagnostics receive a semantic request context instead of many independently prepared inputs from the server?
7. What is the minimal Project Facts API a feature should need to answer “what applies to this file?”

## Current status statement

The work is not conceptually too hard. The target pipeline is straightforward:

```text
roots -> files -> layout -> quick Python facts -> settings candidates -> environments
-> app/template roots -> more files -> inventories -> semantic/IDE consumers
-> optional runtime enrichment
```

The exhaustion comes from interface depth, not domain complexity. The concepts now exist, but the execution path still exposes too much of the machinery to too many places.
