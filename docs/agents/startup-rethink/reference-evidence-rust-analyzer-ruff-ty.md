# Reference evidence: rust-analyzer and Ruff/ty

This note records external evidence gathered while reassessing `startup-rethink` against rust-analyzer and Ruff/ty. It is not an implementation plan; it is a source-backed evidence log for revising the plan.

Pinned upstream revisions used for durable reference:

- `rust-lang/rust-analyzer` `master` at `7f916ab1b1f669cec017960c2d91a9a87b4b7bae`.
- `astral-sh/ruff` `main` at `8c04080b5e449b077500fff1cf1d83c2a69af4c9`.

Use URLs of the form `https://github.com/<owner>/<repo>/blob/<commit>/<path>#Lx-Ly` when citing this evidence from durable docs.

## rust-analyzer: lowered inputs, server-side loading state

rust-analyzer’s core stance is explicit: analyzer core does no I/O; VFS and project model data are lowered into inputs.

Evidence:

- `rust-lang/rust-analyzer:crates/base-db/src/input.rs:1-7` says the module specifies analyzer input and that neither it nor analyzer core performs I/O; actual I/O is done in `vfs` and `project_model` then lowered to input.
- `rust-lang/rust-analyzer:crates/ide-db/src/lib.rs:84-96` shows `RootDatabase` fields as Salsa storage, `Files`, `CratesMap`, and nonce. There is no DB-owned project-loading readiness field.
- `rust-lang/rust-analyzer:crates/base-db/src/lib.rs:94-99` shows `Files` as lookup infrastructure from VFS IDs to Salsa input handles: file text, source root input, file-to-source-root input.
- `rust-lang/rust-analyzer:crates/base-db/src/lib.rs:207-238` shows Salsa inputs for `LibraryRoots`, `LocalRoots`, `FileText`, `FileSourceRootInput`, and `SourceRootInput`.
- `rust-lang/rust-analyzer:crates/base-db/src/input.rs:450-467` shows individual `Crate` Salsa inputs with crate data, workspace data, cfg, and env.
- `rust-lang/rust-analyzer:crates/base-db/src/input.rs:592-722` shows `CrateGraphBuilder::set_in_db` lowering builder data into Salsa `Crate` inputs and `AllCrates`, reusing/updating inputs instead of storing one ambient crate-graph readiness value.
- `rust-lang/rust-analyzer:crates/base-db/src/change.rs:53-88` shows `FileChange::apply` transactionally updating roots, file text, and crate graph.

Loading/progress state is server state, not Salsa:

- `rust-lang/rust-analyzer:crates/rust-analyzer/src/global_state.rs:76-198` shows `GlobalState` owns mutable server state: VFS snapshot, `AnalysisHost`, diagnostics, flycheck, discovery handles, VFS progress, workspaces, and operation queues.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/reload.rs:68-90` computes `is_quiescent` from server-side flags and queues such as `vfs_done`, workspace/build/proc-macro queues, discovery jobs, and VFS progress versions.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/reload.rs:282-468` runs workspace loading, build-data fetching, and proc-macro loading on task pools with progress callbacks; final results are later lowered into the analysis database.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/lsp/utils.rs:116-165` emits LSP work-done progress from `GlobalState`.

Implication for DJLS: if the goal is rust-analyzer-style, progress, active jobs, quiescence, generations, and loading/restart orchestration belong outside Salsa. Salsa should see durable facts: files, source roots, project roots, config/discovery data, environment/search-path facts, inventories, and diagnostics.

## rust-analyzer: project discovery as domain data plus typed partial failure

rust-analyzer’s project-model crate is the discovery/build-system boundary, not a generic readiness state.

Evidence:

- `rust-lang/rust-analyzer:crates/project-model/src/lib.rs:1-17` says the crate handles project discovery, custom build steps, and lowering concrete models to `base_db::CrateGraph`.
- `rust-lang/rust-analyzer:crates/project-model/src/workspace.rs:54-109` shows `ProjectWorkspace` / `ProjectWorkspaceKind`, with Cargo data, optional metadata error, build-script data, rustc-source result, JSON projects, and detached files.
- `rust-lang/rust-analyzer:crates/project-model/src/cargo_workspace.rs:736-815` shows partial cargo metadata support: full metadata failure can return `--no-deps` metadata plus an error.
- `rust-lang/rust-analyzer:crates/project-model/src/build_dependencies.rs:30-60` shows typed build-script/proc-macro outputs (`cfgs`, `envs`, `out_dir`, `proc_macro_dylib_path`) and error fields.
- `rust-lang/rust-analyzer:crates/project-model/src/workspace.rs:944-1009` lowers a concrete `ProjectWorkspace` to `CrateGraphBuilder` and proc-macro paths.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/reload.rs:471-566` keeps old workspace data on failed reload unless switching from an empty workspace.

Implication for DJLS: root discovery should become concrete root/project domain inputs with typed issues and partial results. A coarse `ProjectDiscoveryAvailability::{Loading, Ready, Stale, Unavailable}` is better suited to orchestration/UI than semantic truth.

## Ruff/ty: stable project root input on DB, not loose readiness singleton

Ruff/ty intentionally stores a stable `Project` Salsa input handle on its concrete database. That is a legitimate pattern, but the handle is a semantic project root, not a loose loading state.

Evidence:

- `astral-sh/ruff:crates/ty_project/src/db.rs:26-45` defines `ty_project::Db::project() -> Project` and `ProjectDatabase { project: Option<Project>, ... }`. The comment says the handle must remain stable for the lifetime of the database because many tracked queries branch on the untracked `db.project()` read before consulting tracked `Project` fields; structural reloads must update the existing `Project` via setters instead of swapping the handle.
- `astral-sh/ruff:crates/ty_project/src/lib.rs:48-118` shows `Project` as a `#[salsa::input]` with domain fields: open fileset, indexed file set, metadata, settings, included paths, settings diagnostics, check mode, verbose flag, and force-exclude flag.
- `astral-sh/ruff:crates/ty_project/src/db.rs:85-127` initializes `Program` and then creates/stores one `Project` input handle.
- `astral-sh/ruff:crates/ty_project/src/lib.rs:245-298` reloads by updating existing `Project` fields via Salsa setters.
- `astral-sh/ruff:crates/ty_project/src/db/changes.rs:318-335` calls `project.reload(...)` on structural changes rather than replacing `self.project`.
- `astral-sh/ruff:crates/ty_python_core/src/program.rs:13-23` defines a separate singleton `Program` input for Python version, platform, and search paths.
- `astral-sh/ruff:crates/ty_server/src/session.rs:252-264` and `1585-1624` show LSP readiness/workspace initialization as session state outside Salsa.

Implication for DJLS: if we choose Ruff/ty style, the stable DB-owned handle should be a real `Project`/`WorkspaceProject` root input with fields that affect analysis, not a standalone `ProjectLoadingState` readiness bag. Reload should update tracked fields in place.

## Ruff/ty: project metadata, settings, environment/search paths

Ruff/ty models project discovery and Python environment as stable metadata/settings/program facts, with diagnostics.

Evidence:

- `astral-sh/ruff:crates/ty_project/src/metadata.rs:26-44` shows `ProjectMetadata` storing stable root/name/options and extra config paths to watch.
- `astral-sh/ruff:crates/ty_project/src/metadata.rs:136-267` shows project discovery precedence: closest `pyproject.toml` with `tool.ty` or `ty.toml`, then closest `pyproject.toml`, then virtual default project.
- `astral-sh/ruff:crates/ty_project/src/metadata.rs:285-338` lowers metadata/options to `ProgramSettings` and project `Settings`.
- `astral-sh/ruff:crates/ty_project/src/metadata/options.rs:159-286` discovers or configures Python environment, site-packages, stdlib, and inferred Python version, producing diagnostics/fallbacks.
- `astral-sh/ruff:crates/ty_module_resolver/src/settings.rs:11-35` models module search path settings as domain facts.
- `astral-sh/ruff:crates/ty_python_core/src/program.rs:54-79` updates `Program` settings only when changed.
- `astral-sh/ruff:crates/ty_project/src/metadata/settings.rs:17-24` explicitly recommends narrower Salsa queries/settings to reduce invalidation blast radius.

Implication for DJLS: Django settings candidates, Python paths, interpreter facts, and environment candidates should become stable domain inputs/outcomes. Readiness should be derived from those inputs and query outcomes, not stored as a parallel loading flag.

## File sets and inventories

rust-analyzer and Ruff/ty both avoid competing authoritative aggregate file-readiness state.

Evidence:

- `rust-lang/rust-analyzer:crates/vfs/src/file_set.rs:68-118` defines `FileSetConfig` as path-prefix partitions over a VFS.
- `rust-lang/rust-analyzer:crates/base-db/src/input.rs:84-99` defines `SourceRoot` as a file set; consumers use source roots and path resolution.
- `rust-lang/rust-analyzer:crates/load-cargo/src/lib.rs:396-409` derives `SourceRoot`s by partitioning the VFS with local/library classification.
- `astral-sh/ruff:crates/ty_project/src/lib.rs:48-118` stores open files, lazy indexed file set, metadata/settings, included paths, and check mode on `Project`.
- `astral-sh/ruff:crates/ty_project/src/files.rs:14-24` says indexed files are lazy/cached and all subsequent mutations go through `IndexedMut`, which uses the Salsa setter so Salsa knows when the indexed files change.
- `astral-sh/ruff:crates/ty_project/src/lib.rs:671-703` lazily collects project files and resets `file_set` to lazy on reload.
- `astral-sh/ruff:crates/ty_project/src/db/changes.rs:349-354` says a full project-file reload supersedes incremental project-file updates.

Implication for DJLS: installed-app files, configured template directories, and first-party roots should probably be domain roots/settings/inventory inputs. If we keep partitioned file loading, it needs one authoritative source per inventory/partition; aggregate `ProjectLoadingState.source_files` plus per-node readiness is a dual-source-of-truth risk.

## Runtime enrichment analogies

rust-analyzer separates runtime-derived semantic facts from job/progress state.

Evidence:

- `rust-lang/rust-analyzer:crates/project-model/src/build_dependencies.rs:30-60` stores build-script/proc-macro outputs and degraded dylib states as project-model data.
- `rust-lang/rust-analyzer:crates/project-model/src/workspace.rs:78-90` carries build-script results and optional rustc workspace errors in `ProjectWorkspaceKind::Cargo`.
- `rust-lang/rust-analyzer:crates/project-model/src/workspace.rs:944-980` lowers build-script results into the crate graph.
- `rust-lang/rust-analyzer:crates/hir-expand/src/proc_macro.rs:214-252` represents missing/disabled proc macro expanders as semantic expansion errors.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/flycheck.rs:181-192` models flycheck as a spawned server worker; `flycheck.rs:283-342` separates diagnostics from progress messages.
- `astral-sh/ruff:crates/ty_site_packages/src/lib.rs:1-4` and `ty_module_resolver/src/lib.rs:38-45` show external environment discovery as fact-production for resolver search paths.

Implication for DJLS: runtime Project Introspection should look like rust-analyzer build-script/proc-macro facts once complete, while the act of running Python/Django should look like flycheck/server work. Stable enrichment results can be Salsa facts; subprocesses, cache warmup, progress, cancellation, and generations should remain server/session state.
