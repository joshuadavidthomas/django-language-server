# Design Decision Record: startup-rethink

## Summary of change
DJLS startup should stop treating one fat `Project` input as the container for all project knowledge. The server should complete the LSP handshake quickly, load first-party files and source roots in the background, and let `djls-project` derive a Django-shaped project model from file-set inputs and Python source models.

The target model is rust-analyzer-inspired: workspace loading populates analyzer inputs; static Django Discovery derives settings, app, template, and environment candidates; deeper semantic work and runtime Project Introspection happen lazily or as optional enrichment. Startup readiness should be visible through LSP work-done progress when clients support it, with tracing logs as the fallback channel. `Project` remains domain vocabulary for the Django codebase, but it should not remain the central semantic/input API or fact bag.

## Amendment: domain models over generic facts
The project model must not introduce a generic `Fact<T>` wrapper or `*Fact` shadow domain objects such as `TemplateLibraryFact` or `InstalledAppFact`. Static-analysis precedent from rust-analyzer, Pyright, mypy, Pyrefly, TypeScript, Dart analyzer, clangd, PHPStan, and Psalm points the other way: keep domain models primary and put uncertainty in resolver-specific result types, inference-local placeholders, diagnostics, or provenance fields.

Use concrete domain objects such as `DjangoEnvironment`, `InstalledApp`, `TemplateDirectory`, `TemplateLibrary`, `Template`, and `PythonModule`. Use domain-specific result states such as `ModuleResolution`, `EnvironmentSelection`, `InstalledAppResolution`, `TemplateLookupResult`, and `TemplateLibraryResolution` for unresolved, ambiguous, deferred, or partial outcomes. Shared provenance support is allowed, but it should attach to the domain object, resolver result, or diagnostic that owns it.

## Current state
- `initialize` constructs `Session` and `DjangoDatabase`, then bootstraps a single `Project` before returning capabilities (`crates/djls-server/src/server.rs:131-200`, `crates/djls-server/src/session.rs:51-75`, `crates/djls-db/src/db.rs:88-115`).
- `Project::bootstrap` chooses one optional Django Settings Module and seeds `TemplateDirs::Unknown`, empty template libraries, empty template files, an empty Python index, and empty extracted external maps (`crates/djls-semantic/src/project/input.rs:293-325`).
- The current `Project` input mixes identity/config, selected environment, discovered template/Python facts, template libraries, and extracted external semantics (`crates/djls-semantic/src/project/input.rs:229-285`).
- `initialized` loads a template-library cache and queues `refresh_external_data`; on a cache miss it awaits the refresh. Even cache-hit refresh can hold the shared `Session` mutex while background work runs (`crates/djls-server/src/server.rs:203-249`, `crates/djls-server/src/server.rs:40-71`, `crates/djls-semantic/src/project/sync.rs:47-85`).
- Startup/readiness status currently reaches clients only through tracing-backed `window/logMessage`; DJLS does not record `window.workDoneProgress`, call `create_work_done_progress`, emit `$/progress`, or handle `window/workDoneProgress/cancel` (`crates/djls-server/src/logging.rs:31-110`, `crates/djls-server/src/client.rs:95-122`, `crates/djls-server/src/server.rs:173-179`, `crates/djls-server/src/server.rs:386-423`).
- `tower-lsp-server` 0.23.0 has the needed pieces for server-initiated work-done progress, but token creation and progress notifications are separate operations: `Client::progress` emits `$/progress` only after `Client::create_work_done_progress` succeeds.
- `refresh_external_data` is the old monolithic refresh path: template dirs, template libraries, template files, Python index, and external semantic extraction (`crates/djls-semantic/src/project/sync.rs:47-85`, `crates/djls-semantic/src/project/sync.rs:367-516`).
- Existing static scaffolding currently uses confidence-aware `Fact<T>` values and can represent multiple Django Environment candidates, but it is not wired into validation; this design does not carry that generic wrapper forward (`crates/djls-semantic/src/project/static_model.rs:21-47`, `crates/djls-semantic/src/project/static_django_environments.rs:1-6`).
- Django itself loads settings by importing the configured settings module and copying uppercase names; `manage.py` usually sets `DJANGO_SETTINGS_MODULE`; `django.setup()` then populates apps from `INSTALLED_APPS`; template setup uses `TEMPLATES`, app `templates/` dirs, and installed app `templatetags/` packages.

## Desired end state
- `initialize` is protocol-only: create minimal runtime state and return capabilities without runtime Django introspection, filesystem-wide Django discovery, or deep semantic extraction.
- `initialized` starts background jobs and client-visible progress, but does not await static project loading before the server can answer requests.
- When the client advertises `window.workDoneProgress`, startup jobs use server-initiated work-done progress: create a token, send begin/report/end `$/progress` messages, and end cleanly on success, degraded completion, or failure. Logs remain the fallback/status-detail channel.
- First-party workspace files are loaded into explicit source-root/file-set inputs. Project queries enumerate those inputs, not the filesystem or `SourceFiles` side tables.
- A new `djls-project` crate owns Django project discovery/lowering: project layout indexing, Python source models, lightweight module resolution, settings candidates, Django Environment candidates, static installed-app projection, and enrichment merge policy.
- Static project indexes are tracked derived queries, not background-written blobs. Background project-model work prewarms selected queries and marks generation-scoped readiness.
- Runtime Project Introspection is optional background enrichment. It may confirm or augment static domain objects, but static file loading and Python source models are authoritative for startup.
- CLI `djls check` and LSP share the same `djls-project` model. The CLI runs the phases synchronously and reports ambiguity in terminal output.

## What we're not doing
- Not eagerly validating every template at startup.
- Not eagerly building the full Model Graph.
- Not running Python, importing project code, calling `django.setup()`, or emulating app `ready()` hooks.
- Not recursively scanning all of `site-packages` at startup.
- Not using `refresh_external_data` as the extension point for the new startup path.
- Not preserving the current fat `Project` API as compatibility glue. Internal consumers should move to the new APIs directly, with tests proving the server still works.
- Not merging ambiguous Django Environments into a fake union environment.

## Patterns to follow
- rust-analyzer separates real-world project modeling from analyzer inputs: `project-model` lowers Cargo/JSON workspace data to neutral inputs, while VFS/source roots and Salsa queries drive semantic analysis.
- Ruff/ty treats first-party roots and library search paths differently for durability. It registers site-packages as high-durability library roots and resolves/reads targeted third-party paths lazily rather than indexing the whole environment.
- DJLS already follows the analysis/infrastructure split: tracked queries compute derived facts, while imperative functions synchronize outside-world state into Salsa inputs (`ARCHITECTURE.md:129-148`). Keep that split, but replace the monolithic refresh with smaller jobs and derived queries.

## Design rules

### Protocol readiness is not project readiness
`initialize` must return after minimal session/runtime setup. `initialized` starts background work and progress reporting. Features must handle missing or ambiguous project facts and republish diagnostics/features when facts arrive.

Alternatives considered:
- Await cheap cataloging during startup: rejected because large workspaces would still make startup feel blocked.
- Build project facts inside `Session::new`: rejected because it makes filesystem/project discovery part of handshake latency.

### `Project` is not the central fact bag
Remove `Project` from the central semantic/input API. Keep Project as domain vocabulary, and possibly as a lightweight discovered-project identity in `djls-project`, but do not store discovered facts such as template files, Python indexes, template libraries, extracted external maps, or one chosen settings module on a fat `Project` input.

Alternatives considered:
- Reduce `Project` to identity/config only: useful intermediate idea, but still keeps a central object every query is tempted to hang from.
- Add a `ProjectCatalog`: rejected because it recreates the blob under another name.

### Bring back `djls-project`
Create `djls-project` as the rust-analyzer-style project-model crate. It owns Django startup-shaped discovery and lowering, not LSP behavior or deep template semantics.

Belongs in `djls-project`:
- Django project layout indexing.
- Python module path candidates.
- Python source models.
- Lightweight module resolution for project discovery.
- Settings candidates and effective settings models.
- Django Environment candidates and file-scoped selection results.
- Static installed-app projection.
- Template Directory and Template Tag Library inventory models.
- Enrichment input types and merge policy.

Stays out:
- Template parsing and validation.
- Tag/filter argument extraction.
- Full Model Graph construction.
- Runtime subprocess lifecycle.
- LSP diagnostics/completions/progress orchestration.

Alternatives considered:
- Keep everything under `djls-semantic::project`: rejected because `djls-semantic` is already overstuffed.
- Put Django role classification in `djls-source`/`djls-workspace`: rejected because generic crates should not know Django.

### Explicit file-set/source-root input
Add an explicit loaded file-set/source-root input. The loader writes file existence, roots, local/library classification, and an inclusion/exclusion summary from ignore rules. `SourceFiles` continues to own file handles/revisions, but project queries enumerate the file-set input rather than runtime side tables or the filesystem.

Alternatives considered:
- Make `SourceFiles` enumerable: rejected because that makes runtime state an implicit query input.
- Let project queries walk the filesystem: rejected because it violates the startup model.

### Static indexes are derived, not written
Only file-set/source-root inputs and enrichment inputs are written imperatively. Static project-model indexes are tracked derived queries over those inputs and file text. Background jobs may prewarm them for readiness, but they are not the source of truth.

Layer-specific indexes should stay bounded:
- project layout index
- Python source index
- module-resolution index
- environment-candidate index
- enrichment index

Expose focused query APIs over these indexes so consumers do not rummage through a mega graph.

Alternatives considered:
- Background jobs write all indexes: rejected because stale-state handling becomes imperative and broad.
- One all-knowing `DjangoGraph`: rejected because it recreates the central fact bag.

### Python source modeling is local-only
`djls-project` owns Python source modeling for project discovery. Each `PythonSourceModel` records what is in one file: uppercase assignments, imports/star imports, relative imports, `os.environ.setdefault("DJANGO_SETTINGS_MODULE", ...)`, class/function names, decorator names, spans, source order, and provenance. It does not resolve imports or decide cross-file precedence.

Module-resolution queries compose those source models without rereading files.

Alternatives considered:
- Resolve obvious imports inline in extraction: rejected because it blurs the local-facts boundary.
- Emit effective settings directly from extraction: rejected because it collapses ambiguity too early.

### Dependency shapes stop at anti-corruption layers
Dependency-native shapes should stay at the edge. Ruff AST nodes, inspector JSON payloads, and LSP protocol types must be translated into DJLS-native domain models or resolver results before they reach project-model or semantic consumers.

For Python source modeling, the boundary is `Ruff AST -> PythonSourceModel -> SettingsComposition -> DjangoEnvironmentCandidate`. `DjangoEnvironmentCandidate` must consume resolved module/settings indexes, not parser nodes. Unknown or partial expressions should be represented with DJLS-native unknown/partial value types inside settings composition, not preserved Ruff AST fragments.

For runtime enrichment, the boundary is `inspector response -> ProjectEnrichmentDraft -> enrichment input -> merge policy`. Runtime responses should not mutate environment candidates directly.

Alternatives considered:
- Let Ruff AST nodes flow into settings/environment logic: rejected because it couples project discovery to parser internals and makes future parser or fact-shape changes expensive.
- Let inspector JSON update semantic facts directly: rejected because runtime data needs provenance, staleness, and merge policy.

### Settings composition is the spine of static Django Discovery
The lightweight graph should follow Django startup order: settings candidates, uppercase assignments, effective settings, `INSTALLED_APPS`, static installed-app projection, template dirs, template libraries, model/admin/views/urls/forms role candidates.

First supported settings composition patterns:
- direct uppercase assignment such as `INSTALLED_APPS = [...]`
- simple append/extend/concat such as `INSTALLED_APPS += [...]`, `INSTALLED_APPS = BASE_APPS + [...]`, and cheap starred-list forms
- direct imports and star imports from local settings modules
- source-order precedence where imports apply first and the current file overrides/appends later

Unresolved expressions become unknown segments with provenance.

Alternatives considered:
- Path/layout-only graph: rejected as too limited for the desired Django lay of the land.
- Eager rich semantic graph: rejected because it violates non-goals.
- All-or-nothing settings evaluation: rejected because it throws away useful common partial facts.
- Heuristic guessing: rejected because `INSTALLED_APPS` is canonical hard truth.

### Preserve all Django Environment candidates
Every settings candidate may produce a Django Environment candidate. Explicit `[[django_environments]]` config, `DJANGO_SETTINGS_MODULE`, `manage.py`, and conventional settings files contribute candidates with provenance. Startup must not globally choose one.

Semantic features use a late file-scoped selection query such as `environment_for_file(file) -> Selected | Ambiguous | Unknown`. Ambiguity degrades features instead of guessing.

Alternatives considered:
- Pick one default environment at startup: rejected because it repeats today's core flaw.
- Require config for ambiguous cases: rejected as too hostile by default.

### Ambiguity is a project warning, not a diagnostic
If DJLS cannot choose one Django Environment for a file or workspace scope, suppress environment-specific diagnostics/features as needed and send a deduped workspace/project-level warning through logs/progress. Do not emit per-file diagnostics or modal popups for server-context ambiguity.

Alternatives considered:
- Union facts across candidates: rejected because it invents a Django world that does not exist.
- Pick the highest-ranked candidate: rejected because it hides uncertainty.
- Per-file diagnostics: rejected because ambiguity is not a template source error.

### Installed apps are canonical
Static installed-app projection uses known `INSTALLED_APPS` entries from effective settings. Do not guess app-like packages from the filesystem. If `INSTALLED_APPS` is partial, project only known ordered entries and preserve unknown gaps.

Static installed-app projection may resolve app package/app-config files, read selected package layout and Python source models, discover app template dirs, and discover conventional `templatetags/*.py` modules. It does not import modules, run app configs, evaluate arbitrary expressions, or emulate Django's app registry lifecycle.

Alternatives considered:
- Guess app-like packages: rejected because Django's canonical source is `INSTALLED_APPS`.
- Partial app registry emulation: rejected because it drifts toward runtime Django.

### Installed app file loading is bounded
Startup eagerly loads first-party files and selected dependency metadata/search roots. Register `site-packages` as high-durability library roots, but do not recursively scan all third-party files.

After settings composition identifies known `INSTALLED_APPS`, load Django-relevant files under those app package roots, such as `apps.py`, `models.py`/`models/`, `templates/`, `templatetags/`, and possibly `admin.py`/`urls.py` as role candidates.

Alternatives considered:
- Ignore libraries until lazy resolution: rejected because selected metadata/library roots are part of the useful lay of the land.
- Recursively scan all `site-packages`: rejected because Python environments are too broad and Ruff/ty points toward bounded installed app file loading.

### Template-library inventory is static; symbols are semantic
`djls-project` should statically discover Template Tag Library inventory from Django builtins, installed app `templatetags` packages, and statically known `TEMPLATES[*].OPTIONS["libraries"]`. Tag/filter definition extraction remains lazy/deep work in `djls-semantic` or enrichment.

Alternatives considered:
- Keep library inventory runtime-backed: rejected because it keeps the old startup dependency.
- Fully extract all libraries during static readiness: rejected as too heavy.

### Runtime introspection is enrichment only
Runtime Project Introspection runs after static discovery as optional enrichment. It can add runtime origins, fill unknowns, or confirm/augment Template Tag Library and Template Directory models, but it must not be required for responsiveness or static project readiness.

`djls-project` owns enrichment input types and merge policy. `djls-server`/`djls-db` own subprocess execution, caches, background scheduling, stale-result guards, and applying enrichment inputs.

Alternatives considered:
- Keep runtime introspection as source of truth for template dirs/libraries: rejected because it preserves the old startup model.
- Remove runtime introspection immediately: rejected as too much behavior loss before static discovery matures.

### Background work does not hold the session lock
Background tasks capture immutable inputs plus a server-local startup generation token, compute outside the shared `Session` lock, then briefly lock to apply input changes. Stale results are discarded if the active startup generation changed before apply.

Replace `refresh_external_data` with a dependency-shaped set of jobs, not a single sequential pipeline. The file-set load is the first hard prerequisite because it creates the inputs other static queries enumerate. After that, independent prewarming and loading tasks may run concurrently where their inputs are available: project layout, Python source models, environment candidates, installed app file loading, optional runtime enrichment, and optional deep semantic enrichment. Readiness milestones are aggregate observations over those task states, not the scheduler.

Alternatives considered:
- One new `reload_project` pipeline: rejected because it risks becoming `refresh_external_data` with a new name.
- Dedicated reload actor now: deferred; useful later if guarded apply steps become tangled.

### Readiness is phased and reported with work-done progress
Keep internal readiness state independent from LSP progress UI. These are readiness milestones, not a serial work queue:
- protocol-ready: LSP handshake completed; do not create server-initiated progress before the initialize response
- workspace-ready: first-party file set, Python source models, and environment candidates prewarmed for the current generation
- django-apps-ready: installed app package files loaded for known `INSTALLED_APPS`
- enriched: optional runtime/deep enrichment applied or explicitly skipped/degraded

After `initialized`, use server-initiated work-done progress when `window.workDoneProgress` is supported. Create a single startup progress token per `StartupGeneration`, title it around loading Project Facts, then report task and milestone transitions with short messages and optional percentages only when a real total exists. End that token when the mandatory static startup milestones have reached `django-apps-ready` or a degraded terminal state. Optional runtime/deep enrichment may use a separate token if it can outlive the core startup load.

`Client::progress` must only be used after `Client::create_work_done_progress` succeeds. If the client lacks support or token creation fails, do not send `$/progress`; emit the same phase transitions through tracing logs instead. Progress is observational only: features may answer in degraded mode before later phases complete and should republish diagnostics/features when relevant project-model inputs change.

Alternatives considered:
- Logs-only readiness: rejected because LSP has a standard progress UI and rust-analyzer uses it for analogous startup/background work.
- Request-attached `workDoneToken` or partial-result progress for startup: rejected because startup jobs are server-initiated background work, not one LSP request result stream.
- Delay features until static graph ready: rejected because it reintroduces readiness coupling.

### Progress cancellation is deferred
Do not mark startup progress cancellable in the first implementation slice. LSP defines `window/workDoneProgress/cancel`, but `tower-lsp-server` 0.23.0 does not expose a `LanguageServer` callback for it. If a future dependency version exposes cancellation, only cancel work that is safe to drop, such as query prewarming or optional enrichment; file-set/source-root input application must remain atomic and generation-checked.

Alternatives considered:
- Mark startup progress cancellable now: rejected because the pinned server crate cannot deliver the cancellation notification to DJLS handlers.
- Ignore cancellation permanently: deferred because long optional enrichment or indexing jobs may become worth canceling later.

### Caches are hints
Fresh file loading and Python source models are authoritative. Caches may seed expensive runtime/deep enrichment with provenance/staleness, but a fresh file-set pass and tracked Python source modeling remain the source of truth.

Alternatives considered:
- Cache the static project graph and validate later: rejected because invalidation complexity is high and stale layout facts are dangerous.
- Ignore caches entirely: rejected because existing expensive enrichment data can still be useful.

### CLI and LSP share the model
`djls check` should use the same `djls-project` inputs and queries as the LSP path. It runs project loading/model derivation synchronously and reports ambiguous environment selection as terminal warnings or errors according to CLI strictness.

Alternatives considered:
- Keep CLI on old setup temporarily: rejected because it preserves two discovery models.

### Tests must prove the new contract
Because this is a clean internal rewrite, tests must prove behavior rather than preserve compatibility layers.

Required test groups:
- startup/LSP tests proving `initialize` does not run runtime introspection or await project loading
- request behavior while project loading is in progress
- work-done progress capability handling: supported clients receive token creation plus begin/report/end, while unsupported or create-failed clients get logs only
- file-set/source-root loading, ignore/exclude behavior, create/delete/rename updates
- project layout indexing
- local Python source models
- settings composition and module resolution
- multiple Django Environment candidates and file-scoped selection
- ambiguity warnings and degraded features
- runtime introspection failure with static project models still available
- cache-as-hint behavior

Add a tiny separate real-LSP e2e foundation, likely pytest/pytest-lsp based, focused on the hard startup/readiness contract: spawn `djls serve`, send `initialize`/`initialized`, assert fast handshake, open a file and make a lightweight request while project loading is in progress, then observe work-done progress or log fallback plus any diagnostic update. Do not make the first e2e test a full feature matrix.

Corpus coverage should follow once the model stabilizes, except cheap reuse of existing multisite fixtures is encouraged.

## Open design questions
- What exact neutral input types replace the current `Project` input in Salsa, and which crate owns each trait/API?
- What is the first implementation slice for introducing `djls-project` without leaving the old `Project` fact bag in place?
- What pytest LSP plugin and CI target should host the tiny e2e startup contract test?
