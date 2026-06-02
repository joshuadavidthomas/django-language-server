# Technical Research: startup-rethink

## Summary

DJLS startup currently has a split LSP path: `initialize` constructs a `Session` and bootstraps a single `Project` synchronously, while `initialized` loads a template-library cache and then runs external project refresh through the server queue. On a cache miss, `initialized` waits for the full refresh; on a cache hit, it does not await the refresh, but the refresh task still locks the shared `Session` while it runs.

The current project state is not a complete startup catalog. Salsa inputs cover a `Project`, tracked `File` inputs, template libraries, template directories, known template files, a Python module index, and extracted external facts. Those fields are populated by a mix of synchronous bootstrap, cache load, runtime Django introspection, filesystem walks, and Python static extraction. A confidence-aware static project model and static Django-environment discovery exist, but they are marked as not wired into validation yet.

rust-analyzer separates LSP handshake from workspace loading more aggressively: it sends the initialize response before workspace rediscovery, queues workspace fetching after `initialized`, builds input state such as files/source roots/crate graph, and computes semantic state lazily through Salsa. The closest DJLS equivalents are workspace/project discovery, source roots, file identity, Django Environment candidates, settings/apps/template facts, and runtime-backed enrichment; Cargo-specific crate metadata, crate ownership, build scripts, proc macros, and sysroot machinery do not map exactly to Django.

## Current startup behavior

### Answer

`initialize` blocks on creating the session and initial `Project` input. This includes workspace-root selection, settings/config loading, environment-file loading, Django Settings Module resolution, interpreter/source-root setup, and site-packages probing. It does not run the Python inspector, scan template directories, parse Python modules, walk all project files, or load the inspector cache.

`initialized` first attempts a synchronous cache load for a template-library snapshot. It then enqueues `refresh_external_data`. If the cache was absent, it awaits that refresh. The refresh performs runtime Django introspection, cache write, template-directory file walking, Python model/templatetag indexing, and external rule/model extraction. If the cache was present, the handler returns without awaiting the refresh, but the queued task takes the `Session` mutex before calling `refresh_external_data`, so normal request handlers that use `with_session` / `with_session_mut` may still wait behind the refresh while it holds the lock.

### Findings

- `crates/djls-server/src/server.rs:131-200` — `initialize` calls `Session::new(&params)`, takes `session.client_info().position_encoding()`, stores the new session under `self.session`, then returns static server capabilities.
- `crates/djls-server/src/session.rs:51-75` — `Session::new` selects the first workspace folder or current directory, parses client options, creates `Workspace::new()`, loads settings from the project path with client overrides, and constructs `DjangoDatabase::new(...)`.
- `crates/djls-conf/src/lib.rs:91-165` — settings loading checks project-root config files, including `pyproject.toml` `[tool.djls]`, `.djls.toml`, and `djls.toml`, then applies overrides.
- `crates/djls-db/src/db.rs:88-115` — `DjangoDatabase::new` creates the filesystem handle, source-file registry, settings mutex, project slot, and project introspector; if a project path exists it calls `set_project`, which bootstraps `Project`.
- `crates/djls-semantic/src/project/input.rs:293-325` — `Project::bootstrap` discovers the interpreter, resolves the Django Settings Module, loads `.env` or configured env file values, registers source roots, and creates a `Project` Salsa input with `template_dirs = Unknown`, empty template libraries, empty template files, empty Python index, and empty extracted external maps.
- `crates/djls-semantic/src/project/input.rs:336-416` — env-file loading performs synchronous filesystem checks/reads; Django Settings Module resolution uses configured `django_settings_module`, then `$DJANGO_SETTINGS_MODULE`, then `manage.py` plus hard-coded candidates `settings`, `config.settings`, and `project.settings`.
- `crates/djls-semantic/src/project/resolve.rs:126-177` — search paths include the project root, explicit `PYTHONPATH` entries that are directories, and site-packages discovered from a venv-like directory.
- `crates/djls-server/src/server.rs:203-249` — `initialized` loads the template-library cache, queues `refresh_external_data`, and waits for the queued task only when no cache was loaded.
- `crates/djls-server/src/server.rs:40-71` — all `with_session` / `with_session_mut` access takes the same `Arc<Mutex<Session>>`; queued tasks receive the same session handle.
- `crates/djls-server/src/queue.rs:43-47` and `crates/djls-server/src/queue.rs:83-115` — the server queue is documented and implemented as sequential execution: tasks run one at a time in receive order.
- `crates/djls-semantic/src/project/sync.rs:47-85` — `refresh_external_data` is the imperative refresh boundary and calls, in order, template-dir refresh, template-library refresh, template-file refresh, Python-index refresh, and external semantic-data refresh.
- `crates/djls-semantic/src/project/sync.rs:99-108` and `crates/djls-semantic/src/project/introspector.rs:51-100` — template directories come from a typed project-introspection request; failed introspection returns `None` and leaves the current value unchanged.
- `crates/djls-semantic/src/project/sync.rs:156-184` — template libraries come from a `TemplateLibrarySnapshotRequest`; the snapshot is saved to cache and applied to `Project.template_libraries` only if the value changed.
- `crates/djls-semantic/src/project/sync.rs:367-425` — template files are discovered by walking known template directories; the Python index is rebuilt from discovered model files plus workspace templatetag modules.
- `crates/djls-semantic/src/project/sync.rs:455-516` — external semantic refresh reads external Python files and extracts tag rules, filter arities, block specs, and Django Models outside Salsa file inputs.
- `crates/djls-semantic/src/project/introspector.rs:242-405` — the inspector subprocess is spawned or restarted lazily when an introspection query needs it, and the process environment includes `PYTHONPATH`, `DJANGO_SETTINGS_MODULE`, and env-file variables.

### Tests

- `crates/djls-workspace/src/workspace.rs:382-512` — workspace tests cover open/update/close behavior, buffer-first file reads, source invalidation, and reverting to disk after close.
- `crates/djls-semantic/src/project/sync.rs:621-657` — cache tests cover deterministic keys, input variation, and round-tripping cached template-library snapshots through the filesystem.
- `crates/djls-semantic/src/project/input.rs:428-537` — env-file tests cover default `.env`, configured paths, missing files, comments/blank lines, and quoted values.

### Gaps

- I found no direct test covering the full LSP `initialize`/`initialized` critical path or characterizing request behavior during a cache-hit background refresh that holds the session lock.
- I found no current startup path that walks all project files into a complete catalog before semantic refresh.

## Current startup task categories

### Answer

The current startup work falls into these categories:

| Timing | Work | Category |
| --- | --- | --- |
| `initialize` | workspace folder/current-dir selection | plain input construction / filesystem state |
| `initialize` | `Settings::new` config loading | external filesystem I/O |
| `initialize` | `Workspace::new` overlay + buffer setup | runtime state construction |
| `initialize` | `DjangoDatabase::new` and `Project::bootstrap` | Salsa input construction |
| `initialize` | interpreter discovery, `.env` read, settings-module auto-detect | filesystem/env I/O and heuristic Django discovery |
| `initialize` | project root and site-packages source-root registration | filesystem discovery for durability roots |
| `initialized` phase 1 | template-library snapshot cache read | cache loading / JSON I/O |
| `initialized` phase 2 | template dirs and template-library snapshot requests | runtime Django introspection / subprocess management |
| `initialized` phase 2 | template directory walk | filesystem discovery |
| `initialized` phase 2 | model-file discovery and templatetag module resolution | filesystem/module discovery |
| `initialized` phase 2 | external model/rule extraction | external file I/O plus Ruff AST static extraction |

### Findings

- `crates/djls-workspace/src/workspace.rs:33-44` — workspace construction only creates buffer storage and an overlay filesystem over the OS filesystem.
- `crates/djls-source/src/files.rs:11-49` — source-file registry state is a path-to-`File` side table plus registered roots; it is not a full catalog of all files.
- `crates/djls-source/src/files.rs:71-99` — file durability is assigned when a `File` is first created based on the longest matching registered root.
- `crates/djls-source/src/file.rs:12-44` — a tracked `File` input stores path and revision; `source()` reads through the database filesystem and `line_index()` is derived from source text.
- `crates/djls-source/src/file.rs:119-158` — file kind is extension-based: `.py` is Python, `.djhtml` / `.html` / `.htm` are templates, and everything else is `Other`.
- `crates/djls-workspace/src/walk.rs:25-97` — a general `walk_files` helper exists with ignore/glob/hidden/link/depth options, but the located startup path does not use it to build a full startup catalog.
- `crates/djls/src/commands/common.rs:35-72` — the CLI check path does eager path/template discovery from explicit paths, configured template dirs, or the project root; this is separate from LSP startup.

### Gaps

- The current LSP startup path does not record ignored/excluded state, config-file candidates, all settings candidates, all template-name candidates, or all Python-module candidates as a single cheap inventory.
- File creation, deletion, and rename are not represented as catalog updates in the located LSP code.

## Existing lazy behavior caused by incomplete inventory

### Answer

Several features depend on project facts that are unavailable until cache load or external refresh. With default `Project` inputs, template directories are `Unknown`, template files are empty, the Python index is empty, and template libraries are empty/default. In those states, template resolution returns not-found with no or limited candidates, `{% load %}` completions can be empty, load-library validation is skipped until active knowledge is known, and workspace model/templatetag extraction has no module list to process.

### Findings

- `crates/djls-semantic/src/project/input.rs:306-319` — bootstrap seeds `TemplateDirs::Unknown`, `TemplateLibraries::default()`, `ProjectTemplateFiles::default()`, and `ProjectPythonIndex::default()`.
- `crates/djls-semantic/src/project/sync.rs:333-340` — when template directories are not known, template files are reset to the default empty set.
- `crates/djls-semantic/src/resolution.rs:12-18` — discovered templates come only from `project.template_files(db)`.
- `crates/djls-semantic/src/resolution.rs:60-81` — resolving a missing template reports attempted paths only when template directories are known; with no project or unknown dirs the attempted list is empty/default.
- `crates/djls-ide/src/navigation.rs:11-35` — goto definition works only for `OffsetContext::TemplateReference`; unresolved templates log a warning and return `None`.
- `crates/djls-ide/src/completions.rs:805-854` — `{% load %}` library completions return an empty list when no `TemplateLibraries` value is available.
- `crates/djls-ide/src/completions.rs:913-945` — filter completions use installed symbols when knowledge is known; otherwise they fall back to discovered names if any exist.
- `crates/djls-semantic/src/validation/scoping.rs:142-170` — load-library diagnostics return early unless `template_libraries.active_knowledge == Knowledge::Known`.
- `crates/djls-semantic/src/project/input.rs:203-217` — `project_model_modules` and `project_templatetag_modules` are tracked queries over `ProjectPythonIndex`; an empty index yields no workspace model/templatetag modules.
- `crates/djls-semantic/src/queries.rs:93-168` — workspace model, tag-rule, filter-arity, and block-spec collection iterate those module lists and then extract from tracked `File` inputs.

## rust-analyzer comparison

### Answer

rust-analyzer's startup evidence shows a practical boundary: the LSP initialize response is sent before workspace discovery, workspace discovery runs on a worker after the client's `initialized` notification, and the resulting workspace data is lowered into analyzer inputs such as VFS file IDs, source roots, and crate graph. The semantic model is documented as derived Salsa state computed lazily/on demand. A second phase handles build-script/proc-macro data and tries to avoid unnecessary invalidation when that data arrives.

The concepts that map cleanly to DJLS are input-state concepts: workspace discovery, project folders, VFS/file IDs, source roots, local/library partitioning, a graph-like project input, background refresh, optional cache priming, and lazy semantic queries. The concepts that do not map exactly are Cargo metadata, crate roots as ownership boundaries, Rust sysroot/crate graph semantics, build scripts, proc macros, cfg-driven crate instances, and Rust module ownership rules.

### External findings

- `rust-lang/rust-analyzer:crates/rust-analyzer/src/bin/main.rs:285-317` — rust-analyzer sends the initialize response before rediscovering workspaces and entering the main loop.
- `rust-lang/rust-analyzer:lib/lsp-server/src/lib.rs:193-208` — `initialize_finish` sends `InitializeResult` first and then waits for the client's `initialized` notification.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/main_loop.rs:176-201` — startup workspace fetching is queued only once `GlobalState::run` starts.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/reload.rs:282-385` — workspace fetching runs on a worker thread and loads `ProjectWorkspace` values from Cargo manifests, JSON projects, detached files, or discovery.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/main_loop.rs:821-838` — completed workspace fetches are stored and set `wants_to_switch` so the fetched state can be applied later.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/reload.rs:471-739` — switching workspaces updates project folders, VFS loader configuration, source-root configuration, and crate graph state.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/global_state.rs:145-170` — rust-analyzer describes workspace loading as two-phase: fast `cargo metadata` first, then `cargo check` for build scripts and proc macros, with partial availability after the first phase.
- `rust-lang/rust-analyzer:docs/book/src/contributing/architecture.md:23-38` — the architecture guide separates input state, specifically files plus `CrateGraph`, from derived semantic state and says derived state is computed lazily/on demand.
- `rust-lang/rust-analyzer:docs/book/src/contributing/architecture.md:136-152` — `base-db` owns the Salsa input layer, `FileId` is opaque, and build-system-specific details are kept out of the core input layer.
- `rust-lang/rust-analyzer:crates/vfs/src/lib.rs:1-25` — VFS records file changes and maps paths to `FileId`s; loading/watching is delegated to a loader, and changes are pushed into Salsa.
- `rust-lang/rust-analyzer:crates/base-db/src/input.rs:81-125` — source roots group files and support relative file resolution without exposing absolute paths to the analyzer core.
- `rust-lang/rust-analyzer:crates/project-model/src/workspace.rs:1-3` — `project-model` lowers Cargo or `rust-project.json` data into Salsa's abstract `CrateGraph`.
- `rust-lang/rust-analyzer:crates/project-model/src/workspace.rs:55-109` — `ProjectWorkspace` variants include Cargo, JSON, and detached-file workspaces.
- `rust-lang/rust-analyzer:docs/book/src/contributing/architecture.md:269-279` — proc macros are isolated in a separate process because they can panic, segfault, and be non-deterministic.
- `rust-lang/rust-analyzer:crates/load-cargo/src/lib.rs:200-202` — cache prefill is optional, which makes cache priming an optimization rather than required startup state.

### Repo findings

- `ARCHITECTURE.md:129-148` — DJLS uses a database trait stack and states boundary rules: concrete database structs own storage/runtime infrastructure, tracked queries compute derived values, imperative refresh functions synchronize outside-world data into Salsa inputs, and durability is split across first-party/project/config/external data.
- `CONTEXT.md:9-38` — DJLS terminology already distinguishes Project, Workspace, Project Facts, Django Environment, Django Settings Module, Django Discovery, Static Extraction, and Project Introspection.
- `CONTEXT.md:261-267` — Project Facts are built by Static Extraction and Project Introspection, and a Project may contain one or more Django Environments.

## File and project catalog facts

### Answer

The current repo stores only part of the candidate catalog shape:

- per tracked file: path, revision, derived source text, derived line index, and extension-derived file kind;
- per source root: root path and `Project` vs `LibrarySearchPath` classification for durability assignment;
- per template file: template name, path, and tracked `File` handle;
- per indexed Python module: module path, path, kind (`Model` or `TemplateTag`), and tracked `File` handle;
- per project: root, interpreter, Django Settings Module, pythonpath, env vars, template directories, tag specs, template libraries, template files, Python index, and extracted external semantic maps;
- in the static scaffolding: confidence-aware facts, import roots, module resolutions, installed-app facts, template-backend facts, template-dir facts, template-library facts, and template-symbol facts.

The repo does not currently expose a single eager catalog that records every path, source root, local/library classification, ignored/excluded state, file kind, template-name candidate, Python-module candidate, settings candidate, model-bearing candidate, templatetag candidate, and package/config metadata for all files.

### Findings

- `crates/djls-source/src/file.rs:12-44` — `File` is a Salsa input containing path and revision; `source()` and `line_index()` are tracked derived values.
- `crates/djls-source/src/file.rs:119-158` — file kind is derived from extensions, not from parsing or project configuration.
- `crates/djls-source/src/files.rs:11-49` — `SourceFiles` maps paths to `File` inputs and stores source roots.
- `crates/djls-source/src/files.rs:71-99` — registered roots determine durability for future file creation; existing files keep the durability assigned when created.
- `crates/djls-semantic/src/project/input.rs:22-40` — `TemplateDirs` distinguishes `Unknown` from `Known(Vec<Utf8PathBuf>)`.
- `crates/djls-semantic/src/project/input.rs:42-75` — `ProjectTemplateFiles` stores ordered `ProjectTemplateFile` entries and preserves duplicate template names.
- `crates/djls-semantic/src/project/input.rs:77-119` — `ProjectPythonIndex` stores sorted `ProjectPythonModule` entries and can filter models vs templatetags.
- `crates/djls-semantic/src/project/input.rs:127-155` — `ProjectTemplateFile` contains template name, path, and `File`.
- `crates/djls-semantic/src/project/input.rs:157-201` — `ProjectPythonModule` contains module path, path, kind, and `File`.
- `crates/djls-semantic/src/project/input.rs:219-325` — the `Project` Salsa input contains project root, interpreter, Django Settings Module, pythonpath, env vars, template dirs, tag specs, template libraries, template files, Python index, and extracted external tag/filter/block/model maps.
- `crates/djls-semantic/src/project/static_model.rs:21-47` — the static model has `Fact<T>` values with `Known`, `Partial`, `Unknown`, and `Ambiguous` confidence states.
- `crates/djls-semantic/src/project/static_model.rs:200-346` — static fact types cover fields/reason sources, import roots, module resolution, installed apps, app configs, template backends, template dirs, template libraries, and template symbols.
- `crates/djls-semantic/src/project/static_resolver.rs:32-99` — static import-root discovery can return known, partial, or unknown roots from workspace root, auto `src`, explicit Python paths, site-packages, and `.pth` import roots.
- `crates/djls-semantic/src/project/static_resolver.rs:103-152` — static module resolution returns known, partial, unknown, or ambiguous module facts.
- `crates/djls-server/src/server.rs:157` — LSP capabilities set `file_operations: None`; file create/delete/rename operations are not registered.
- `crates/djls-workspace/src/workspace.rs:67-126` — document open/update/save/close creates or bumps tracked file revisions and updates buffers; it does not update project template/Python catalogs.

### External findings

- Django settings docs, `https://docs.djangoproject.com/en/stable/topics/settings/#designating-the-settings` — `DJANGO_SETTINGS_MODULE` must be a Python-path module such as `mysite.settings`, and the settings module must be importable on `sys.path`.
- Django tutorial project layout, `https://docs.djangoproject.com/en/stable/intro/tutorial01/#creating-a-project` — `startproject` creates `manage.py` plus a project package containing `settings.py`, `urls.py`, `asgi.py`, and `wsgi.py`.
- Django applications docs, `https://docs.djangoproject.com/en/stable/ref/applications/#projects-and-applications` — a Django project is primarily defined by settings, while applications are Python packages listed in `INSTALLED_APPS`.
- Django settings reference, `https://docs.djangoproject.com/en/stable/ref/settings/#installed-apps` — `INSTALLED_APPS` entries are dotted paths to app config classes or app packages; names and labels must be unique.
- Django template settings reference, `https://docs.djangoproject.com/en/stable/ref/settings/#templates` — `TEMPLATES` is a list of engine configurations; `DIRS` define template search directories, `APP_DIRS` controls app template discovery, and `OPTIONS["loaders"]` customizes loaders.
- Django custom template tags docs, `https://docs.djangoproject.com/en/stable/howto/custom-template-tags/#code-layout` — custom libraries live in an installed app's `templatetags/` package; the module filename is the `{% load %}` name; `DjangoTemplates` `libraries` can also map labels to modules.
- `django/django:django/template/utils.py:97-110` on `stable/6.0.x` — app template directories are discovered by iterating app configs and returning existing `<app_config.path>/<dirname>` directories.
- `django/django:django/template/backends/django.py:133-183` on `stable/6.0.x` — template tag discovery considers Django builtins plus `<app_config.name>.templatetags` packages for installed apps and yields modules with `register`.

### Gaps

- Current code does not parse `manage.py` to extract the generated `os.environ.setdefault('DJANGO_SETTINGS_MODULE', ...)`; it only checks whether `manage.py` exists before trying hard-coded settings-module candidates.
- Current project state has no explicit `SettingsCandidate`, `ManagePyCandidate`, `ConfigFileCandidate`, `IgnoredState`, or all-files source-root catalog type.

## Django Environment discovery

### Answer

The current runtime path bootstraps a single `Project` with a single optional Django Settings Module. It does not use the `django_environments` configuration in `Project::bootstrap`. A separate static discovery module can return multiple `ResolvedDjangoEnvironment` values from explicit config, or one environment from legacy Django Settings Module resolution, but that module is explicitly marked as not wired into project validation yet.

Current default selection behavior in the wired runtime path is: configured `django_settings_module`, then `$DJANGO_SETTINGS_MODULE`, then `manage.py`-gated auto-detection among `settings`, `config.settings`, and `project.settings`. For explicit multiple environments, `djls-conf` can load multiple `[[django_environments]]` entries and the static discovery code maps each to a resolved or unknown settings module fact.

### Findings

- `crates/djls-conf/src/django_environments.rs:1-31` — `DjangoEnvironmentConfig` stores a root and optional Django Settings Module.
- `crates/djls-conf/src/lib.rs:71-88` — `Settings` contains both `django_settings_module` and `django_environments`.
- `crates/djls-conf/src/lib.rs:178-186` — getters expose `django_settings_module()` and `django_environments()`.
- `crates/djls-conf/src/lib.rs:339-367` — config tests load two `[[django_environments]]` entries from TOML and assert both roots/settings modules are preserved.
- `crates/djls-conf/src/lib.rs:372-401` — override tests show client/override settings replace configured `django_environments`.
- `crates/djls-semantic/src/project/static_django_environments.rs:1-6` — the module says static Django environment discovery is intentionally not wired into project validation yet.
- `crates/djls-semantic/src/project/static_django_environments.rs:35-65` — `discover_django_environments` returns configured environments if present; otherwise it falls back to legacy `resolve_django_settings(...)` and returns zero or one environment.
- `crates/djls-semantic/src/project/static_django_environments.rs:67-119` — explicit or fallback environment settings modules are parsed, resolved through import roots, and represented as `Fact<ResolvedModule>` with reasons for invalid or missing data.
- `crates/djls-semantic/src/project/input.rs:293-325` — wired `Project::bootstrap` does not call `discover_django_environments`; it stores a single resolved Django Settings Module on `Project`.
- `CONTEXT.md:21-23` and `CONTEXT.md:261-267` — the glossary defines a Django Environment as a path-scoped Django analysis context and states that a Project contains one or more Django Environments.

### External findings

- Django settings docs, `https://docs.djangoproject.com/en/stable/topics/settings/#the-django-admin-utility` — tools can select settings using `DJANGO_SETTINGS_MODULE` or `django-admin --settings=...`.
- Django standalone setup docs, `https://docs.djangoproject.com/en/stable/topics/settings/#calling-django-setup-is-required-for-standalone-django-usage` — standalone tools must set `DJANGO_SETTINGS_MODULE` or configure settings, then call `django.setup()` to load settings and populate the app registry.
- `django/django:django/conf/project_template/manage.py-tpl:7-18` on `stable/6.0.x` — generated `manage.py` sets a default `DJANGO_SETTINGS_MODULE` before calling `execute_from_command_line`.
- `django/django:django/apps/registry.py:86-124` on `stable/6.0.x` — Django app registry population creates app configs from `INSTALLED_APPS`, checks duplicate names/labels, imports each app's models, then calls `ready()`.

### Gaps

- The repo has no wired runtime representation that associates files or source roots with one of several Django Environments.
- The located code has no default-selection policy for multiple discovered settings modules beyond preserving explicit config order in `Settings` and static discovery.

## Salsa and invalidation model

### Answer

Current Salsa inputs are mainly `File` and `Project`. `File` tracks path and revision, with derived source and line index queries. `Project` tracks root/environment/config/project-fact fields and extracted external maps. Tracked queries consume those fields to derive template resolution, tag specs, filter arities, workspace model graphs, and workspace templatetag extraction. Non-Salsa runtime state includes the LSP `Session`, `Workspace` buffers, overlay filesystem, queue, settings mutex, source-file registry side table, and project introspector process manager.

Durability is split in two places: `Project::bootstrap` assigns high durability to stable project identity and extracted external maps, medium durability to most project fields, and low durability to template files and Python index; `SourceFiles` assigns low durability to project-root files and high durability to library-search-path files. Settings updates and refresh functions compare old vs new values before calling Salsa setters.

Open/save/change/close invalidation currently bumps tracked file revisions. File creation, deletion, rename, and workspace-folder changes do not have located handlers that update the template/Python catalogs.

### Findings

- `crates/djls-source/src/file.rs:12-44` — `File` is an input with path and revision; `source()` depends on revision and database file reads; `line_index()` depends on source.
- `crates/djls-source/src/files.rs:71-99` — source roots choose file durability at file creation time, preserving existing file durabilities afterward.
- `crates/djls-semantic/src/project/input.rs:288-325` — `Project::bootstrap` sets medium durability for the input as a whole, high durability for root and extracted external data, and low durability for template files and Python index.
- `crates/djls-db/src/db.rs:25-47` — `DjangoDatabase` owns runtime state: filesystem, source-file registry, current project, settings, project introspector, and Salsa storage.
- `crates/djls-db/src/db.rs:126-190` — database trait implementations connect file reads, current project, template libraries, filter arities, model graph, diagnostics config, and project introspector to lower crates.
- `crates/djls-db/src/settings.rs:25-113` — settings updates manually compare interpreter, Django Settings Module, pythonpath, env vars, and tag specs before setting project fields; env changes re-register source roots.
- `crates/djls-semantic/src/project/sync.rs:95-184` and `crates/djls-semantic/src/project/sync.rs:367-516` — refresh functions compare current and next values before writing template dirs, template libraries, template files, Python index, and extracted external data.
- `crates/djls-workspace/src/workspace.rs:67-126` — open/save/update/close operations call `db.bump_file_revision(...)` for tracked files and update buffer state.
- `crates/djls-server/src/server.rs:157` — `file_operations` is `None`, so create/delete/rename notifications are not advertised.
- `crates/djls-server/src/server.rs:560-611` — settings/env changes can enqueue and await `refresh_external_data`, then republish open-template diagnostics.

### Tests

- `crates/djls-db/src/db.rs:305-324` — `compute_tag_specs` is cached on repeated access.
- `crates/djls-db/src/db.rs:326-396` — changes to template libraries or tag specs invalidate `compute_tag_specs`; no-op updates do not.
- `crates/djls-db/src/db.rs:447-480` — filter arity extraction is cached on repeated access.
- `crates/djls-db/src/db.rs:482-513` — a file revision bump with unchanged source backdates and does not re-execute extraction.
- `crates/djls-db/src/db.rs:595-619` — a same-value template-library setter does not invalidate `compute_tag_specs`.
- `crates/djls-db/src/db.rs:621-645` — model graph can be empty and is cached on repeated access.

### Gaps

- I found no invalidation tests for adding/removing template files, creating/deleting/renaming Python modules, or changing settings files on disk.
- I found no current catalog-level invalidation model separate from `refresh_external_data` and tracked file revision bumps.

## Introspection, static extraction, and background enrichment

### Answer

Project Introspection currently supplies runtime-derived template directories and template-library snapshots. Those facts are loaded during `initialized` refresh, not during `initialize`. If runtime introspection fails, the query returns `None`; refresh functions return early or leave defaults, so DJLS can continue with whatever cached/default/static facts it already has.

Static extraction has two paths. External module extraction runs imperatively during refresh by reading external Python files and parsing them with Ruff-based extraction helpers. Workspace module extraction is tracked through Salsa once the Python index knows which files are model or templatetag modules. The architecture document states that Project Introspection is expected to shrink as Static Extraction matures.

### Findings

- `ARCHITECTURE.md:152-165` — the Python inspector is described as current runtime-backed Project Introspection; startup has a cache-check phase and a background refresh phase.
- `ARCHITECTURE.md:177-182` — external modules are extracted once during startup and stored on `Project`; workspace modules are indexed and extracted through tracked queries over tracked files.
- `crates/djls-semantic/src/project/introspector.rs:51-100` — `ProjectIntrospector::query` returns `None` on no project, an inspector response with `ok = false`, or an I/O/process error.
- `crates/djls-semantic/src/project/sync.rs:99-108` — template-dir refresh returns early if `fetch_template_dirs` returns `None`.
- `crates/djls-semantic/src/project/sync.rs:156-163` — template-library refresh returns early if the snapshot request fails.
- `crates/djls-semantic/src/project/sync.rs:412-425` — the Python index includes discovered model files and templatetag modules resolved from current template-library registrations.
- `crates/djls-semantic/src/project/sync.rs:455-516` — external semantic extraction scans external rules and models by reading Python source files directly.
- `crates/djls-semantic/src/queries.rs:93-168` — workspace model/templatetag extraction is tracked over `ProjectPythonModule` entries and tracked file sources.
- `ARCHITECTURE.md:105-113` — static extraction never imports Django or runs Python; it parses Python source using the Ruff parser.

### Tests

- `crates/djls-semantic/tests/corpus.rs:1-94` — corpus extraction snapshot tests parse every extraction target in the synced corpus and snapshot extracted rules per file.
- `crates/djls-semantic/tests/corpus_models.rs:1-58` — model corpus tests run `extract_model_graph` against every corpus `models.py` and snapshot non-empty graphs.
- `crates/djls-semantic/src/python.rs:449-758` — corpus-grounded templatetag extraction tests cover custom tags, inclusion tags, and Django built-ins.
- `crates/djls-semantic/src/python/registry.rs:484-709` — registry/tag discovery tests cover templatetag modules and real corpus examples.
- `crates/djls-semantic/src/project/resolve.rs:472-675` — discovery tests cover external/workspace model files, nested packages, model packages, and site-packages skipping.

### Gaps

- I found no Project Introspection failure test that proves the LSP behavior visible to clients when Python, Django, the interpreter path, or `DJANGO_SETTINGS_MODULE` fails after a cheap/static catalog succeeds.
- External extraction currently reads external files outside the tracked file API, so external-file changes are represented by explicit refresh, not by normal file revision invalidation.

## Caching and readiness

### Answer

The current startup cache is a template-library snapshot cache. It is keyed by project root, interpreter, Django Settings Module, and pythonpath, then version-gated by `CARGO_PKG_VERSION`. A cache hit applies template libraries and refreshes workspace templatetag modules from cached registration modules. It does not cache template directories, template files, model files, complete Python index state, external extracted rules, external model graphs, project layout inventory, or settings candidates.

A fresh external refresh is still enqueued every startup. With a cache hit, the server logs that cached data is available and does not await the refresh in `initialized`; with a cache miss, it waits for the refresh. Readiness/progress is communicated through tracing logs forwarded to the editor output panel. I found no explicit work-done progress, degraded-mode state, or structured partial-readiness notification.

### Findings

- `crates/djls-semantic/src/project/sync.rs:142-154` — loading a cached template-library snapshot applies it and refreshes templatetag modules if the snapshot changed project libraries.
- `crates/djls-semantic/src/project/sync.rs:225-260` — the cache key hashes root, interpreter, Django Settings Module, and pythonpath.
- `crates/djls-semantic/src/project/sync.rs:263-330` — cache files live under the XDG cache directory in a legacy `inspector/<hash16>/inspector.json` path and are rejected when `djls_version` differs.
- `crates/djls-semantic/src/project/sync.rs:308-348` — saving the template-library snapshot writes the same cache envelope to disk.
- `crates/djls-server/src/server.rs:203-249` — startup always queues external refresh after cache load; only the no-cache path awaits the queued receiver.
- `crates/djls-server/src/logging.rs:31-110` — `LspLayer` forwards tracing events to the LSP client as log messages.
- `crates/djls-server/src/logging.rs:129-171` — tracing is configured with file logging plus LSP forwarding, filtered to info-level and above.
- `crates/djls-server/src/lib.rs:38-50` — server startup passes a closure that calls `client.log_message(...)` for LSP notifications.
- `ARCHITECTURE.md:219-221` — architecture docs state that tracing logs go to both rotating files and the editor output panel via LSP `window/logMessage`.
- `crates/djls-server/src/server.rs:178` — diagnostic capabilities include default work-done progress options, but the located startup code does not create progress tokens or send progress events.

### Gaps

- The template-library cache key does not include env-file variables, settings-file mtimes, installed-package versions, template-directory layout, or source extraction inputs.
- I found no cache freshness checks for project layout, settings candidates, template files, model files, or extracted external semantic data beyond the template-library snapshot version/key.
- I found no structured client-facing degraded-mode or partial-readiness state beyond logs and ordinary feature fallback behavior.

## Migration and test evidence

### Answer

The code paths affected by a startup-model redraw are identifiable from the current flow: `Session::new`, `DjangoDatabase::new`, `Project::bootstrap`, `initialized` cache/refresh orchestration, `refresh_external_data` and its substeps, source-root/file registration, settings updates, workspace document events, and the not-yet-wired static project model. Existing tests cover many lower-level pieces, but not the full startup contract.

Available fixtures/corpus cases include real packages/projects and a multisite fixture. The corpus covers extraction and model discovery against real code, but the located tests do not yet use the corpus to validate LSP startup inventory, multi-environment selection, cache staleness, or background readiness behavior.

### Findings

- `crates/djls-server/src/session.rs:51-75` — `Session::new` is where current LSP startup picks the root, loads settings, creates workspace state, and creates the database.
- `crates/djls-db/src/db.rs:88-115` — database construction bootstraps `Project` if a project path exists.
- `crates/djls-semantic/src/project/input.rs:293-325` — `Project::bootstrap` is the source of initial Project input fields and durability assignments.
- `crates/djls-server/src/server.rs:203-249` — `initialized` is the orchestration point for cache load, background refresh, and no-cache waiting.
- `crates/djls-semantic/src/project/sync.rs:47-85` — `refresh_external_data` contains the current imperative refresh pipeline.
- `crates/djls-db/src/settings.rs:60-109` — settings changes update project environment fields and determine whether an external-data refresh is needed.
- `crates/djls-semantic/src/project/static_model.rs:1-5` — static project model types are intended for later population from resolver, settings extractor, app registry, and template assembly.
- `crates/djls-semantic/src/project/static_resolver.rs:1-6` — static Python module resolver is not wired into validation yet.
- `crates/djls-semantic/src/project/static_django_environments.rs:1-6` — static Django Environment discovery is not wired into validation yet.

### Tests and fixtures

- `crates/djls-workspace/src/workspace.rs:382-512` — workspace buffer/source tests cover open/update/close and overlay reads.
- `crates/djls-semantic/src/project/sync.rs:621-657` — cache tests cover key determinism, key variation, and filesystem round-trip behavior.
- `crates/djls-semantic/src/project/resolve.rs:371-467` — module resolution tests cover workspace vs external resolution, sys.path order, package `__init__.py`, and partitioning.
- `crates/djls-semantic/src/project/resolve.rs:472-675` — model discovery tests cover workspace/external model files, empty model files, model packages, nested packages, and site-packages skipping.
- `crates/djls-conf/src/lib.rs:339-401` — configuration tests cover loading and overriding multiple `django_environments`.
- `crates/djls-semantic/src/project/static_resolver.rs:476-672` — static resolver tests cover import roots, module resolution, file-module mapping, relative import resolution, and ambiguity cases.
- `crates/djls-semantic/src/project/static_django_environments.rs:172-215` — static environment tests cover explicit environments and fallback behavior.
- `crates/djls-semantic/tests/corpus.rs:76-93` — extraction corpus test iterates all extraction targets and snapshots per-file extraction output.
- `crates/djls-semantic/tests/corpus_models.rs:34-57` — model corpus test iterates corpus model files and snapshots non-empty model graphs.
- `crates/djls-corpus/manifest.toml:21-24` — corpus includes `django-allauth` with a settings module.
- `crates/djls-corpus/manifest.toml:52-54` — corpus includes `django-crispy-forms`.
- `crates/djls-corpus/manifest.toml:230-260` — corpus includes `netbox`, `pretix`, `readthedocs.org`, and `sentry`, with settings modules for several entries.
- `crates/djls-corpus/manifest.toml:273-291` — corpus includes multiple Wagtail versions.
- `crates/djls-corpus/manifest.toml:293-299` — corpus includes `gh401-multisite` with multiple settings modules (`site1.settings.dev`, `site2.settings.dev`).

### Gaps

- I found no integration test proving `initialize` returns without runtime Django introspection.
- I found no test proving no-cache `initialized` blocks until refresh while cache-hit `initialized` does not await refresh.
- I found no test for request behavior while the background refresh holds the session mutex.
- I found no startup inventory test for ambiguous settings modules, multiple Django Environments, or a workspace root containing multiple Projects.
- I found no cache-staleness test for changed env-file variables, changed installed packages, changed template directories, or changed project layout.

## Sources

- `docs/agents/startup-rethink/questions.md` — selected research agenda.
- `CONTEXT.md:7-38`, `CONTEXT.md:261-267`, `CONTEXT.md:321-323` — canonical DJLS terms and relationships.
- `ARCHITECTURE.md:129-182`, `ARCHITECTURE.md:219-282` — Salsa boundaries, Python inspector startup phases, static extraction split, observability, and testing layers.
- rust-analyzer source and docs on upstream `master`, accessed 2026-05-19: `crates/rust-analyzer/src/bin/main.rs`, `lib/lsp-server/src/lib.rs`, `crates/rust-analyzer/src/main_loop.rs`, `crates/rust-analyzer/src/reload.rs`, `crates/rust-analyzer/src/global_state.rs`, `crates/vfs/src/lib.rs`, `crates/base-db/src/input.rs`, `crates/project-model/src/workspace.rs`, `crates/load-cargo/src/lib.rs`, and `docs/book/src/contributing/architecture.md`.
- Django stable docs, accessed 2026-05-19: settings, applications, templates, template loaders, custom template tags, and django-admin/manage.py pages under `https://docs.djangoproject.com/en/stable/`.
- Django source on `stable/6.0.x`, accessed 2026-05-19: `django/conf/project_template/manage.py-tpl`, `django/conf/project_template/project_name/settings.py-tpl`, `django/apps/registry.py`, `django/template/backends/django.py`, `django/template/utils.py`, and `django/template/engine.py`.
