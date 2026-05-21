# Implementation Outline: startup-rethink

## Overview
This outline is the high-level structure that fed the implementation plan. `docs/agents/startup-rethink/plan.md` is the authoritative source for implementation phase mechanics, success criteria, and any conflict between this outline and the final slice plan.

The slicing strategy makes protocol readiness observable before any Django discovery, then grows a Salsa-backed source file set and `djls-project` model until existing IDE features consume static Project Facts directly. The riskiest seam is the startup/session boundary: background discovery must not block `initialize`, await `initialized`, hold the shared `Session` lock while work is computed, or accidentally serialize independent startup tasks behind a phase enum.

## Inputs and output
- Ticket: `docs/tickets/startup-rethink.md`
- Questions: `docs/agents/startup-rethink/questions.md`
- Research: `docs/agents/startup-rethink/research.md`
- Progress research: `docs/agents/startup-rethink/progress-lsp-research.md`
- Design: `docs/agents/startup-rethink/design.md`
- Output: `docs/agents/startup-rethink/outline.md`

## Modeling correction: domain models over generic facts
- `Fact<T>` is not a `djls-project` API. The project model keeps Django domain objects primary and models uncertainty at resolver/inference boundaries.
- Provenance is a small shared support type: source locations and origins may attach to domain objects, diagnostics, or resolver results, but they do not wrap every value.
- Human-facing messages are created at the diagnostics/log/progress boundary. Project-model issues are typed enums with provenance, not arbitrary stored strings.
- Issue enums are sparse and boundary-specific. Add them only to unresolved, ambiguous, deferred, or unsupported result branches, and define their variants in the phase where they first appear.
- Domain objects use domain names: `TemplateLibrary`, `InstalledApp`, `TemplateDirectory`, `DjangoEnvironment`, `Template`, and `PythonModule`.
- Resolver outputs use domain-specific states: `ModuleResolution`, `EnvironmentSelection`, `InstalledAppResolution`, `TemplateLookupResult`, and `TemplateLibraryResolution`.
- Temporary unknowns may exist inside settings composition or inference, but final consumer APIs either return concrete domain models, explicit unresolved/ambiguous result variants, or diagnostics.
- Struct fields are private by default in this outline. Snippets name fields to define shape; public accessors emerge during planning/implementation only when a caller needs them.

## Server task integration note
The final plan supersedes this outline's early server-task sketch. Startup orchestration begins only when the shared loading graph exists in Phase 3. Until then, `initialized` does not schedule discovery work. The existing `crates/djls-server/src/queue.rs` queue remains available for legacy ordered server/session mutation, but startup loading must not be rebuilt around the queue.

## Phase 1: protocol-ready without Project bootstrap
This phase proves the server can complete the LSP handshake and answer lightweight template requests while Project Facts are absent. `plan.md` is authoritative for the detailed checklist.

### Phase scope
- In: minimal session construction, preserving workspace roots, client/default settings only, no bootstrapped `Project`, and request behavior with no current Project Facts.
- In: `initialized` logs receipt and returns immediately.
- Out: no `StartupController`, startup generation guards, work-done progress APIs, loading executors, background discovery work, source file set, `djls-project` crate, static Django Discovery, or replacement for the old semantic fact bag.

### File changes
- `crates/djls-server/src/session.rs`:
  - `Session::new(params: &ls_types::InitializeParams) -> Self` records workspace roots, client options, and client/default settings only; it does not call `Settings::new` for project-file config and does not create a bootstrapped `Project`.
  - `Session::workspace_roots(&self) -> &[Utf8PathBuf]` exposes immutable roots for later loading slices. Session does not create or own startup generations.
- `crates/djls-db/src/db.rs`:
  - `DjangoDatabase::new(file_system: Arc<dyn FileSystem>, settings: &Settings) -> Self` constructs runtime/Salsa state without `project_path` and without `set_project`.
  - Temporary scaffolding: `ProjectDb::project()` may still return `None` while old semantic consumers degrade through existing empty/default paths.
- `crates/djls-server/src/server.rs`:
  - `initialize` constructs/stores the minimal session and returns capabilities without sending server requests or notifications before the initialize response.
  - `initialized` only logs and returns.
  - No startup path calls `load_template_library_cache`, `refresh_external_data`, root config loading, startup controller code, or awaited startup work.

### Validation
The Phase 1 validation proves `initialize` returns capabilities, `initialized` does not start or await Project Facts work, request paths degrade rather than panic when `ProjectDb::project()` is `None`, and a black-box pytest-lsp smoke test can initialize the server and make a lightweight request after `initialized`.

## Phase 2: loaded source file set for first-party roots
This phase proves background workspace loading can publish an explicit file-set input that project queries can enumerate without walking the filesystem themselves.

### Phase scope
- In: a neutral loaded source-file-set/source-root Salsa input for first-party files.
- In: ignored/excluded files are excluded by the loader and summarized for observability.
- Out: no Django layout interpretation, no Python AST extraction, no library/site-packages scan, and no file create/delete/rename notifications beyond the file-set update API.

### New data types
- `SourceFileSet` — neutral Salsa input under `djls-source`; named to avoid putting Workspace or Project semantics in the lowest file layer.
- `SourceRoot` / `FileRootKind` — root grouping and local/library durability boundary. Reuse the existing `djls-source` `FileRootKind::{ Project, LibrarySearchPath }`, matching ty/Ruff vocabulary.
- `DiscoveredSourceFile` — workspace-loader output before Salsa `File` handles exist: path, root, and file kind.
- `SourceFileEntry` — applied file entry with path, root, file kind, and `File` handle. Locality is derived from the entry's root.
- `ProjectFilesLoadRequest` / `ProjectFilesLoadResult` — workspace-owned loader request/result. This follows ty's `ProjectFilesWalker` / `ProjectFilesFilter` role.
- `FileSetSummary` — neutral included/excluded counts owned by `djls-source`. No per-file exclusion-reason enum in this slice; add one only when a feature needs to explain a specific exclusion.

### File changes
- `crates/djls-source/src/files.rs`:
  - `pub enum FileRootKind { Project, LibrarySearchPath }` remains the single root-locality type used for durability and source-file-set roots.
- `crates/djls-source/src/file_set.rs`:
  - Placement note: `djls-source` owns the neutral Salsa input because lower crates need `File` identity and source-root enumeration; `djls-workspace` owns discovery/loading because Workspace is the server view of open documents and filesystem contents.
  - `#[salsa::input] pub struct SourceFileSet` with `roots: Vec<SourceRoot>`, `files: Vec<SourceFileEntry>`, and `summary: FileSetSummary`.
  - `pub struct SourceRoot { path: Utf8PathBuf, kind: FileRootKind }`.
  - `pub struct DiscoveredSourceFile { path: Utf8PathBuf, root: Utf8PathBuf, kind: FileKind }`.
  - `pub struct SourceFileEntry { path: Utf8PathBuf, root: Utf8PathBuf, kind: FileKind, file: File }`.
  - `pub struct FileSetSummary { included: usize, excluded: usize }`.
- `crates/djls-source/src/db.rs`:
  - `fn source_file_set(&self) -> Option<SourceFileSet>` — database capability for file-set consumers.
- `crates/djls-workspace/src/project_files.rs`:
  - `pub struct ProjectFilesLoadRequest { roots: Vec<Utf8PathBuf> }`.
  - `pub struct ProjectFilesLoadResult { roots: Vec<SourceRoot>, files: Vec<DiscoveredSourceFile>, summary: FileSetSummary }`.
  - `pub fn load_project_files(request: &ProjectFilesLoadRequest, fs: &dyn FileSystem) -> ProjectFilesLoadResult` — uses workspace ignore rules and cheap path metadata only.
- `crates/djls-db/src/db.rs`:
  - `source_file_set: Arc<Mutex<Option<SourceFileSet>>>`.
  - `pub fn apply_source_file_set(&mut self, result: ProjectFilesLoadResult)` — creates `File` handles, registers roots, and compares before setting. Stale-result rejection happens in `startup.rs` before apply.
- `crates/djls-server/src/startup.rs`:
  - Source file-set loading remains the first hard prerequisite because later static queries enumerate it.
  - After the file set is applied, independent prewarming tasks may run concurrently and report task-level status without changing the aggregate milestone until their readiness prerequisites are met.

### Validation
A workspace fixture with templates, Python files, ignored files, and an open unsaved buffer yields a `SourceFileSet` whose included entries are enumerable through `SourceDb`, whose summary records excluded counts, whose application does not require holding the session lock during filesystem walking, and whose completion unblocks later startup tasks without forcing those tasks to run serially.

## Phase 3: `djls-project` crate and project layout tracer
This phase proves a new project-model crate can build a reusable layout index from the neutral source file set without depending on LSP or old `Project` state.

### Phase scope
- In: `djls-project` crate, database trait boundary, project discovery input, neutral project layout index, and shared provenance support.
- In: the new crate establishes the invariant that domain objects are the facts; there is no generic `Fact<T>` API and no up-front Django role classifier.
- Out: no Ruff AST extraction, no settings composition, no Django Environment selection, no semantic feature migration, and no `PathKind` / `ConfigFileKind` enum.

### New data types
- `ProjectDiscoveryInput` — Salsa input snapshot of root and existing discovery-relevant `Settings` values. It is not a new user-facing config shape and it does not carry startup generation.
- `Provenance` / `ProvenanceSource` / `OriginSet` — source/origin metadata without stored human messages. `SourceFile` provenance points to evidence inside a tracked source file and may carry a `Span` from Python/template parsing; `Path` provenance points to filesystem layout evidence with no source-text span; `Config`, `Runtime`, and `Cache` identify non-source origins.
- `ProjectDiscoveryIssue` — small typed issue enum for project-discovery-level problems in this phase, such as unsupported root shape.
- `ProjectLayoutIndex` — neutral indexed projection over `SourceFileSet`: file id/path lookups, parent/child relationships, descendants, file names, stems, extensions, directory names, and Python package-marker presence.
- Candidate concepts such as settings candidate, model-module candidate, template directory, and Template Tag Library candidate are derived in later phase-specific queries from the layout index.

### File changes
- `Cargo.toml`:
  - `djls-project = { path = "crates/djls-project" }` in workspace dependencies.
- `crates/djls-project/Cargo.toml`:
  - Internal dependencies: `djls-conf`, `djls-source`, `djls-workspace` only as needed by public contracts.
  - Third-party dependencies use workspace versions and `[lints] workspace = true`.
- `crates/djls-project/src/lib.rs`:
  - Public facade for `Db`, `ProjectDiscoveryInput`, provenance support, project layout, Python module names, domain models, and tracked query entry points.
- `crates/djls-project/src/db.rs`:
  - `#[salsa::db] pub trait Db: djls_source::Db { fn project_discovery_input(&self) -> Option<ProjectDiscoveryInput>; fn project_enrichment(&self) -> ProjectEnrichmentInput; }`.
- `crates/djls-project/src/input.rs`:
  - `#[salsa::input] pub struct ProjectDiscoveryInput` with `root`, `interpreter`, `django_settings_module`, `django_environments`, `pythonpath`, and `env_vars` fields copied from existing `djls-conf::Settings` getters.
  - No new config keys or normalized settings struct are introduced in this phase.
- `crates/djls-project/src/provenance.rs`:
  - `pub struct Provenance { source: ProvenanceSource, origin: OriginSet }`.
  - `pub enum ProvenanceSource { SourceFile { file: File, span: Option<Span> }, Path(Utf8PathBuf), Config, Runtime, Cache, Unknown }`.
  - `pub struct OriginSet` — bitflag-style provenance for static scan, open document, config, runtime introspection, cache, and user override.
  - `pub enum ProjectDiscoveryIssue { UnsupportedRootShape }`.
- `crates/djls-project/src/layout.rs`:
  - `pub struct ProjectLayoutIndex` with raw lookup APIs such as `file_path`, `file_for_path`, `children`, `descendant_files`, `files_by_name`, `files_by_extension`, `dirs_by_name`, and `python_package_dirs`.
  - `#[salsa::tracked(returns(ref))] pub fn project_layout_index(db: &dyn Db) -> ProjectLayoutIndex`.
- `crates/djls-db/src/db.rs` and `crates/djls-bench/src/db.rs`:
  - Implement `djls_project::Db` and expose discovery/enrichment placeholders.
- `crates/djls-semantic/Cargo.toml`:
  - Depends on `djls-project` for domain names and project-model query APIs formerly trapped in semantic static scaffolding.

### Validation
Given only `SourceFileSet` and `ProjectDiscoveryInput`, `project_layout_index` answers raw layout questions without reading old `Project` fields, walking the filesystem, classifying templates, classifying config files, producing candidate roles, or producing `Fact<T>` wrappers.

## Phase 4: Python source model and settings candidates
This phase proves DJLS can derive reusable Python source models from loaded files and turn common Django startup clues into settings candidates without resolving cross-file semantics.

### Phase scope
- In: Ruff-backed local extraction for Python source models, module-name resolution, `manage.py` defaults, and settings-file candidates.
- In: Ruff AST is translated into DJLS-native source models and value schemas at the extraction boundary.
- Out: no import resolution, no effective settings composition, no installed-app projection, and no runtime Django calls.

### New data types
- `PythonSourceModel` — local one-file outline: imports, assignments, calls, class/function definitions, spans, and provenance. Settings-specific concepts such as uppercase settings are derived by later queries from generic assignments.
- `PythonSourceIndex` — index over local Python files from `SourceFileSet`.
- `PyModuleNameResolution` / `ModuleNameIssue` — module-name outcome for a file; issues cover outside-import-root and ambiguous-module-name cases only.
- `QualifiedName` — syntactic unresolved dotted reference such as `os.environ.setdefault` or `register.simple_tag`; imports remain separate and later resolution may combine imports with qualified references.
- `AssignmentTarget` — DJLS-native assignment target shape, such as a simple name, attribute target, or unsupported target.
- `StaticValueSegment<T>` — known or unknown segment inside a partially evaluated list/string-like value.
- `ImportStatement`, `Assignment`, and `CallExpression` — DJLS-native anti-corruption layer types produced from Ruff AST; Ruff AST nodes do not flow past this boundary.
- `StaticValue` / `StaticValueIssue` — limited static value model for settings composition. Unknown values carry typed issues for unsupported expressions or partially unknown values, not generic messages.
- `SettingsCandidate` / `SettingsCandidateSource` — candidate settings modules derived from explicit config, `DJANGO_SETTINGS_MODULE`, `manage.py` defaults, or conventional module locations.
- Name newtypes — `LibraryName`, `PyModuleName`, and `TemplateSymbolName` move from `djls-semantic`; `TemplateName` becomes a non-interned parsed domain newtype in `djls-project`.

### File changes
- `crates/djls-project/src/python/source.rs`:
  - `pub struct PythonSourceModel { file: File, module: PyModuleNameResolution, imports: Vec<ImportStatement>, assignments: Vec<Assignment>, calls: Vec<CallExpression>, class_defs: Vec<ClassDef>, function_defs: Vec<FunctionDef> }`.
  - `pub struct QualifiedName(Vec<String>)`.
  - `pub enum PyModuleNameResolution { Resolved(PyModuleName), OutsideImportRoots { issues: Vec<ModuleNameIssue> }, Ambiguous { candidates: Vec<PyModuleName>, issues: Vec<ModuleNameIssue> } }`.
  - `pub enum AssignmentTarget { Name(String), Attribute(QualifiedName), Unsupported { issue: StaticValueIssue } }`.
  - `pub struct Assignment { target: AssignmentTarget, value: StaticValue, span: Span, origin: OriginSet }`.
  - `pub struct CallExpression { callee: QualifiedName, args: Vec<StaticValue>, span: Span, origin: OriginSet }`.
  - `pub enum StaticValueSegment<T> { Known(T), Unknown { issue: StaticValueIssue } }`.
  - `pub enum StaticValue { String(String), StringList(Vec<StaticValueSegment<String>>), Dict(Vec<(String, StaticValue)>), Unknown { issue: StaticValueIssue } }`.
  - `pub enum ModuleNameIssue { OutsideImportRoots, AmbiguousModuleName }`.
  - `pub enum StaticValueIssue { UnsupportedExpression, PartiallyUnknown }`.
  - `#[salsa::tracked(returns(ref))] pub fn python_source_model(db: &dyn Db, file: File) -> PythonSourceModel`.
- `crates/djls-project/src/python.rs`:
  - `#[salsa::tracked(returns(ref))] pub fn python_source_index(db: &dyn Db) -> PythonSourceIndex` over local Python files from `SourceFileSet`.
- `crates/djls-project/src/settings/candidates.rs`:
  - `pub struct SettingsCandidate { module: PyModuleName, file: Option<File>, source: SettingsCandidateSource, origin: OriginSet }`.
  - `pub enum SettingsCandidateSource { ExplicitConfig, EnvironmentVariable, ManagePyDefault, ConventionalModule }`.
  - Source choices are intentionally narrow: explicit config and environment mirror Django selection mechanisms; `ManagePyDefault` captures generated `manage.py`; `ConventionalModule` covers common importable modules such as `settings`, `config.settings`, and `<package>.settings` found through the layout index. There is no generic `FileName` source.
  - `#[salsa::tracked(returns(ref))] pub fn settings_candidates(db: &dyn Db) -> Vec<SettingsCandidate>`.
- `crates/djls-project/src/names.rs`:
  - `LibraryName`, `PyModuleName`, `TemplateSymbolName`, and non-interned `TemplateName` live in `djls-project`; semantic imports these moved types rather than defining duplicates.
  - The existing semantic interned `TemplateName` is removed or renamed to an internal cache identity. Public project/template APIs use `djls_project::TemplateName`.
- `crates/djls-project/src/testing.rs`:
  - Fixture helpers for source file set plus in-memory Python files; no Python interpreter or Django install required.

### Validation
A fixture containing `manage.py`, `project/settings.py`, `settings/dev.py`, imports, assignments, and unsupported expressions produces Python source models with spans/provenance and multiple settings candidates, while unsupported expressions become `StaticValue::Unknown` rather than parser-specific AST fragments or generic facts.

## Phase 5: module resolution and Django Environment candidates
This phase proves DJLS preserves multiple path-scoped Django Environment candidates and exposes late file-scoped selection instead of choosing one global environment at startup.

### Phase scope
- In: import roots, lightweight module resolution, Django Environment candidates, and `environment_for_file` selection.
- In: explicit `[[django_environments]]`, explicit Django Settings Module, environment/default settings clues, `manage.py`, and conventional settings files all contribute candidates with provenance.
- Out: no effective `INSTALLED_APPS`, no template directory assembly, no installed-app package loading, and no feature-specific diagnostics beyond workspace warnings.

### New data types
- `ImportRoot` / `ImportRootKind` — resolved import roots: `ProjectRoot` for first-party import root, `AutoSrc` for conventional `src/`, `ExplicitPythonPath` for configured/env Python paths, `SitePackages` for interpreter package roots, and `PthFile` for roots introduced by `.pth` files.
- `ResolvedModule` — resolved module path and import-root identity.
- `ModuleResolution` / `ModuleResolutionOutcome` — module lookup result.
- `ModuleResolutionIssue` — typed explanation for not-found, ambiguous, deferred, or unavailable-root outcomes; used for logs/diagnostics and tests, not successful resolution.
- `DjangoEnvironmentId` — plain cloned identity: root plus Django Settings Module.
- `DjangoEnvironmentCandidate` / `EnvironmentCandidateSource` — candidate environment with provenance.
- `EnvironmentSelection` — selected, ambiguous, or unknown file-scoped environment result.
- `EnvironmentSelectionIssue` — typed explanation for ambiguous or unknown selection, such as no candidates, file outside candidate roots, or multiple matching candidates.

### File changes
- `crates/djls-project/src/resolver.rs`:
  - `pub struct ImportRoot { kind: ImportRootKind, path: Utf8PathBuf }`.
  - `pub enum ImportRootKind { ProjectRoot, AutoSrc, ExplicitPythonPath, SitePackages, PthFile }`.
  - `pub struct ResolvedModule { module: PyModuleName, file: Utf8PathBuf, import_root: Utf8PathBuf, location: ModuleLocation }`.
  - `pub struct ModuleResolution { requested: PyModuleName, outcome: ModuleResolutionOutcome }`.
  - `pub enum ModuleResolutionOutcome { Resolved(ResolvedModule), NotFound { issues: Vec<ModuleResolutionIssue> }, Ambiguous { candidates: Vec<ResolvedModule>, issues: Vec<ModuleResolutionIssue> }, Deferred { issue: ModuleResolutionIssue } }`.
  - `pub enum ModuleResolutionIssue { NoImportRoots, RootUnavailable { root: Utf8PathBuf }, NotFound, MultipleCandidates, UnsupportedModuleName }`.
  - `#[salsa::tracked(returns(ref))] pub fn import_roots(db: &dyn Db) -> Vec<ImportRoot>`.
  - `#[salsa::tracked] pub fn resolve_module(db: &dyn Db, requested: PyModuleName) -> ModuleResolution`.
- `crates/djls-project/src/environments.rs`:
  - `pub struct DjangoEnvironmentId { root: Utf8PathBuf, settings_module: PyModuleName }`.
  - `pub struct DjangoEnvironmentCandidate { id: DjangoEnvironmentId, settings: ModuleResolution, source: EnvironmentCandidateSource, origin: OriginSet }`.
  - `pub enum EnvironmentSelection { Selected(DjangoEnvironmentId), Ambiguous { candidates: Vec<DjangoEnvironmentCandidate>, issues: Vec<EnvironmentSelectionIssue> }, Unknown { issues: Vec<EnvironmentSelectionIssue> } }`.
  - `pub enum EnvironmentSelectionIssue { NoCandidates, FileOutsideCandidateRoots { file: File }, MultipleMatchingCandidates }`.
  - `#[salsa::tracked(returns(ref))] pub fn django_environment_candidates(db: &dyn Db) -> Vec<DjangoEnvironmentCandidate>`.
  - `#[salsa::tracked] pub fn environment_for_file(db: &dyn Db, file: File) -> EnvironmentSelection`.
- `crates/djls-server/src/startup.rs`:
  - Project-level ambiguity warning state keyed by generation and candidate set; warnings are deduped and logged/progress-reported once per generation.
- `crates/djls-semantic/src/db.rs`:
  - `pub trait Db: djls_project::Db` replaces direct inheritance from the old project trait for new consumers.

### Validation
The multisite fixture yields two distinct `DjangoEnvironmentCandidate` values, files under each configured root select the matching environment, ambiguous/shared files return `EnvironmentSelection::Ambiguous`, and no global setting collapses the candidates into a union or guessed default.

## Phase 6: effective settings, installed apps, and static template inventory
This phase proves static Django Discovery can follow settings to known installed apps and derive Template Directory and Template Tag Library inventory without runtime Project Introspection.

### Phase scope
- In: effective settings models for first supported composition patterns, static installed-app projection, installed app file loading, Template Directory inventory, and Template Tag Library inventory.
- In: unknown settings segments remain represented with provenance and do not erase known ordered entries.
- Out: no tag/filter definition extraction, no full Model Graph, no app registry emulation, and no recursive scan of all `site-packages`.

### New data types
- `EffectiveSettings` — settings projection needed for Django Discovery, beginning with `INSTALLED_APPS` and `TEMPLATES`.
- `PartialList<T>` / `PartialListSegment<T>` / `SettingsIssue` — statically modeled Python list-like values with ordered known segments and unknown gaps. `INSTALLED_APPS` uses this because expressions such as `BASE_APPS + ["myapp"]` may be only partially known; dict-like settings use structured resolution types or `StaticValue::Dict`, not `PartialList`.
- `TemplateSettingsResolution` — known, partial, or unknown template backend settings.
- `InstalledApp` / `AppConfig` / `InstalledAppResolution` / `InstalledAppIssue` — static installed-app projection.
- `TemplateDirectory`, `ProjectTemplate`, and `TemplateLibrary` — concrete template inventory domain objects.
- `TemplateDirectorySource` — why a Template Directory is present: `SettingsDirs`, `InstalledAppTemplates`, or `RuntimeIntrospection`.
- `TemplateLibrarySource` — why a Template Tag Library is present: `DjangoBuiltin`, `InstalledAppTemplatetags`, `SettingsLibrariesOption`, or `RuntimeIntrospection`.
- `TemplateDirectoryInventory`, `TemplateFileInventory`, and `TemplateTagLibraryInventory` — environment-scoped inventories. Single-field inventories are tuple newtypes; only multi-field inventories use named fields.
- `TemplateLibraryResolution` / `TemplateLibraryIssue` — unresolved or ambiguous library outcomes.

### File changes
- `crates/djls-project/src/settings/composition.rs`:
  - `pub struct EffectiveSettings { installed_apps: PartialList<InstalledAppEntry>, templates: TemplateSettingsResolution }`.
  - `pub struct PartialList<T>(Vec<PartialListSegment<T>>)`.
  - `pub enum PartialListSegment<T> { Known(Vec<T>), Unknown { issue: SettingsIssue } }`.
  - `pub enum TemplateSettingsResolution { Known(Vec<TemplateBackend>), Partial { backends: Vec<TemplateBackend>, issues: Vec<SettingsIssue> }, Unknown { issues: Vec<SettingsIssue> } }`.
  - `pub enum SettingsIssue { UnsupportedExpression, UnknownImport, UnknownListSegment, UnknownTemplateBackend }`.
  - `#[salsa::tracked(returns(ref))] pub fn effective_settings(db: &dyn Db, env: DjangoEnvironmentId) -> EffectiveSettings`.
- `crates/djls-project/src/apps.rs`:
  - `pub struct AppConfig { module: PyModuleName, class_name: String, app_name: PyModuleName, label: String, path: Utf8PathBuf, origin: OriginSet }`.
  - `pub struct InstalledApp { entry: String, module: PyModuleName, path: Utf8PathBuf, config: Option<AppConfig>, origin: OriginSet }`.
  - `pub enum InstalledAppResolution { Resolved(InstalledApp), MissingModule { entry: String, issues: Vec<InstalledAppIssue> }, AmbiguousModule { entry: String, candidates: Vec<InstalledApp>, issues: Vec<InstalledAppIssue> }, Deferred { entry: String, issue: InstalledAppIssue } }`.
  - `pub enum InstalledAppIssue { MissingModule, AmbiguousModule, UnsupportedAppConfig }`.
  - `#[salsa::tracked(returns(ref))] pub fn installed_apps(db: &dyn Db, env: DjangoEnvironmentId) -> Vec<InstalledAppResolution>`.
- `crates/djls-workspace/src/project_files.rs`:
  - `pub struct InstalledAppFilesLoadRequest(Vec<Utf8PathBuf>)`.
  - `pub fn load_installed_app_files(request: &InstalledAppFilesLoadRequest, fs: &dyn FileSystem) -> ProjectFilesLoadResult` — loads Django-relevant files for known installed apps, whether first-party or dependency packages.
- `crates/djls-project/src/templates/inventory.rs`:
  - `pub struct TemplateDirectory { path: Utf8PathBuf, source: TemplateDirectorySource, origin: OriginSet }`.
  - `pub enum TemplateDirectorySource { SettingsDirs, InstalledAppTemplates, RuntimeIntrospection }`.
  - `pub struct ProjectTemplate { name: TemplateName, file: File, directory: Utf8PathBuf, origin: OriginSet }`.
  - `pub struct TemplateLibrary { load_name: LibraryName, module: PyModuleName, source: TemplateLibrarySource, origin: OriginSet }`.
  - `pub enum TemplateLibrarySource { DjangoBuiltin, InstalledAppTemplatetags, SettingsLibrariesOption, RuntimeIntrospection }`.
  - `pub enum TemplateLibraryResolution { MissingModule { load_name: LibraryName, issues: Vec<TemplateLibraryIssue> }, AmbiguousModule { load_name: LibraryName, candidates: Vec<TemplateLibrary>, issues: Vec<TemplateLibraryIssue> }, Deferred { load_name: LibraryName, issue: TemplateLibraryIssue } }`.
  - `pub enum TemplateLibraryIssue { MissingModule, AmbiguousModule, InventoryUnavailable }`.
  - `pub struct TemplateDirectoryInventory(Vec<TemplateDirectoryEntry>)`.
  - `pub enum TemplateDirectoryEntry { Discovered(TemplateDirectory), UnknownSettingsDir { issue: SettingsIssue } }`.
  - `pub struct TemplateFileInventory(Vec<ProjectTemplate>)`.
  - `pub struct TemplateTagLibraryInventory { libraries: Vec<TemplateLibrary>, unresolved: Vec<TemplateLibraryResolution> }`.
  - `#[salsa::tracked(returns(ref))] pub fn template_directories(db: &dyn Db, env: DjangoEnvironmentId) -> TemplateDirectoryInventory`.
  - `#[salsa::tracked(returns(ref))] pub fn template_files(db: &dyn Db, env: DjangoEnvironmentId) -> TemplateFileInventory`.
  - `#[salsa::tracked(returns(ref))] pub fn template_tag_libraries(db: &dyn Db, env: DjangoEnvironmentId) -> TemplateTagLibraryInventory`.
- `crates/djls-server/src/startup.rs`:
  - `DjangoAppsReady` milestone applies installed app file-set additions for known `INSTALLED_APPS` without scanning all library roots.

### Validation
A settings fixture with direct assignments, imports, append/extend/concat patterns, `TEMPLATES`, known `INSTALLED_APPS`, app `templates/`, and app `templatetags/` produces ordered `InstalledApp` resolution states plus static Template Directory and Template Tag Library inventories with unknown gaps preserved.

## Phase 7: semantic features consume static project queries
This phase proves existing template IDE behavior can use the new static project model directly instead of reading template files, template directories, or Template Tag Libraries from the old `Project` input.

### Phase scope
- In: template resolution, reference indexing, load-library completions, load-library diagnostics, and diagnostics republishing use `djls-project` queries.
- In: degraded behavior is explicit for `Unknown` and `Ambiguous` environment selection.
- Out: no external tag/filter rule extraction migration, no Model Graph migration, and no runtime enrichment merge yet.

### New data types
- `TemplateLookupResult` / `TemplateLookupIssue` — found, not found, ambiguous, or deferred template reference lookup. `TemplateLookupIssue` is limited to lookup-precondition failures such as unknown environment, ambiguous environment, unavailable template inventory, or invalid Template Name.
- Static availability adapters — semantic-side translation from `TemplateTagLibraryInventory` into current validation/completion availability states.
- Workspace/project warning key — deduplication key for ambiguity warnings emitted through startup/logging, not per-template diagnostics.

### File changes
- `crates/djls-semantic/src/resolution.rs`:
  - `discover_templates(db: &dyn SemanticDb, env: DjangoEnvironmentId) -> Vec<Template<'_>>` — built from `djls_project::template_files`.
  - `pub enum TemplateLookupResult<'db> { Found(Template<'db>), NotFound { name: String, tried: Vec<Utf8PathBuf> }, Ambiguous { name: String, candidates: Vec<Template<'db>> }, Deferred { name: String, issue: TemplateLookupIssue } }`.
  - `pub enum TemplateLookupIssue { EnvironmentUnknown, EnvironmentAmbiguous, InventoryUnavailable, InvalidTemplateName }`.
  - `resolve_template(db: &dyn SemanticDb, source: File, name: &str) -> TemplateLookupResult<'_>` — selects an environment through `environment_for_file`.
  - `find_references_to_template(db: &dyn SemanticDb, source: File, name: &str) -> Vec<TemplateReference<'_>>` — indexes only the selected environment or returns empty on ambiguity.
- `crates/djls-semantic/src/scoping.rs` and `crates/djls-ide/src/completions.rs`:
  - Template Tag Library inventory comes from `djls_project::template_tag_libraries` plus builtin/static availability models.
  - Ambiguous libraries produce availability states that support suggestions without fabricating a single library source.
- `crates/djls-ide/src/diagnostics.rs`:
  - Environment ambiguity suppresses environment-specific diagnostics and surfaces a workspace/project warning through startup/logging, not per-template diagnostics.
- `crates/djls-db/src/db.rs`:
  - `SemanticDb::template_dirs`, `SemanticDb::template_libraries`, and current-project helper methods are removed or narrowed to new static query adapters.
- `crates/djls-semantic/src/project/input.rs`:
  - Temporary scaffolding removed for `template_dirs`, `template_files`, `python_index`, and `template_libraries` fields once all consumers above have moved.

### Validation
With runtime introspection disabled, a template `{% include %}` can resolve against statically discovered Template Directories, `{% load %}` completions show statically discovered Template Tag Libraries, and ambiguous environments suppress environment-specific diagnostics while parser/builtin validation continues.

## Phase 8: extraction inputs move from Python index to project inventories
This phase proves workspace and installed-app Python extraction can run from `djls-project` inventories rather than the old `ProjectPythonIndex` and external maps.

### Phase scope
- In: model-module candidates, templatetag-module candidates, and installed-app dependency module candidates are exposed as project-model queries.
- In: tag/filter/block extraction and Model Graph queries read those candidates through tracked `File` inputs for workspace files and high-durability installed app files.
- Out: no runtime Project Introspection enrichment and no stale-cache restoration of deep extraction results yet.

### New data types
- `PythonModuleRole` — project-model role labels for Python modules, such as model, templatetag, app config, URL, admin, and forms modules.
- `PythonModule` — module name, `File`, roles, and provenance.
- `PythonModuleInventory` — tuple newtype over environment-scoped Python modules used by extraction and Model Graph queries.

### File changes
- `crates/djls-project/src/python/inventory.rs`:
  - `pub enum PythonModuleRole { Model, TemplateTag, AppConfig, Urls, Admin, Forms }`.
  - `pub struct PythonModuleInventory(Vec<PythonModule>)`.
  - `pub struct PythonModule { module: PyModuleName, file: File, roles: Vec<PythonModuleRole>, origin: OriginSet }`.
  - `#[salsa::tracked(returns(ref))] pub fn python_module_inventory(db: &dyn Db, env: DjangoEnvironmentId) -> PythonModuleInventory`.
- `crates/djls-semantic/src/queries.rs`:
  - Workspace model and templatetag collection query `python_module_inventory` instead of `project_model_modules` / `project_templatetag_modules`.
  - External extraction maps are derived from installed app `File` inputs and high-durability source roots, not stored on `Project`.
- `crates/djls-db/src/scanning.rs`:
  - New imperative boundary for applying installed app file-set updates; no broad `refresh_external_data` orchestration.
- `crates/djls-semantic/src/project/sync.rs`:
  - `refresh_external_data` and old cache/write helpers are deleted or quarantined behind the Phase 9 enrichment path only.

### Validation
Editing a known workspace templatetag module invalidates only the tracked extraction for that file, adding a new app templatetag file through the file-set update makes it appear in `python_module_inventory`, and selected installed-app templatetag modules are extracted without scanning unrelated `site-packages` files.

## Phase 9: runtime Project Introspection as enrichment
This phase proves runtime-backed Project Introspection can augment the static project model without becoming the source of startup readiness or mutating environment candidates directly.

### Phase scope
- In: enrichment input/domain types, inspector DTO translation, stale-result guards, cache-as-hint loading, and merge policy over static domain models.
- In: Python/Django failure leaves static readiness intact and records degraded enrichment state.
- Out: no reintroduction of `refresh_external_data`, no inspector-owned Template Directory source of truth, and no direct inspector writes to semantic feature outputs.

### New data types
- `ProjectEnrichmentInput` — Salsa input for runtime/deep enrichment hints. Startup generation stays in server-local apply envelopes.
- `ProjectEnrichmentDraft` — runtime-boundary DTO after inspector JSON has been translated into DJLS-native enrichment data.
- `EnrichmentStatus` / `EnrichmentIssue` — not-started, fresh, stale, or failed enrichment state with typed issues.

### File changes
- `crates/djls-project/src/enrichment.rs`:
  - `#[salsa::input] pub struct ProjectEnrichmentInput` with `runtime_template_dirs`, `runtime_template_libraries`, `runtime_installed_apps`, `deep_extraction_hints`, and `status` fields.
  - `pub enum EnrichmentStatus { NotStarted, Fresh, Stale, Failed { issue: EnrichmentIssue } }`.
  - `pub enum EnrichmentIssue { RuntimeUnavailable, InspectorFailed, CacheStale }`.
  - `pub fn merge_template_libraries(static_inventory: &TemplateTagLibraryInventory, enrichment: &ProjectEnrichmentInput) -> TemplateTagLibraryInventory`.
- `crates/djls-semantic/src/project/introspector.rs`:
  - Inspector JSON response types remain at the runtime boundary and convert into `ProjectEnrichmentDraft` values.
- `crates/djls-db/src/db.rs`:
  - `pub fn apply_enrichment(&mut self, enrichment: ProjectEnrichmentDraft)` — compares before setting. Stale-result rejection happens in `startup.rs` before apply.
- `crates/djls-server/src/startup.rs`:
  - Enrichment job captures immutable config/generation, runs subprocess/cache work outside the `Session` lock, and applies only translated enrichment inputs.
  - `Enriched` and degraded enrichment status are client-visible through work-done progress/logging; static `WorkspaceReady` and `DjangoAppsReady` remain successful if enrichment fails.
  - Optional enrichment may use a separate non-cancellable progress token if it can outlive the core startup progress.
- `crates/djls-semantic/src/project/sync.rs`:
  - Old inspector cache functions are replaced by enrichment cache functions with keys that include discovery-relevant config and provenance/staleness metadata.

### Validation
With a failing Python interpreter or broken Django Settings Module, static Template Directory and Template Tag Library inventory remains available, enrichment status records a failure, and no request waits behind the failed subprocess. With a warm cache, cached enrichment is marked as a hint and a fresh file-set/static pass remains authoritative.

## Phase 10: CLI, e2e readiness, and old Project removal
This phase proves LSP and `djls check` share the same project model and that the old fat `Project` input is gone from the semantic API.

### Phase scope
- In: CLI synchronous loading through the same `djls-project` model, tiny real-LSP startup e2e coverage, corpus smoke coverage for multisite/static discovery, and deletion of legacy re-exports.
- In: documentation reflects protocol-ready, workspace-ready, django-apps-ready, and enriched phases.
- Out: no full feature matrix in the first e2e suite and no compatibility wrapper for the old `Project` fact bag.

### New data types
- No new core data types. This phase removes legacy wrappers and proves the types introduced earlier are the shared CLI/LSP model.

### File changes
- `crates/djls/src/commands/check.rs` and `crates/djls/src/commands/common.rs`:
  - `djls check` builds `SourceFileSet`, `ProjectDiscoveryInput`, the static project model, and installed app inventories synchronously before template validation.
  - Ambiguous Django Environment selection is reported as terminal warning/error according to CLI strictness, not hidden by a default global choice.
- `crates/djls-semantic/src/lib.rs`:
  - Public re-exports of old `Project`, `ProjectTemplateFiles`, `ProjectPythonIndex`, `TemplateDirs`, old cache loaders, and `refresh_external_data` are removed.
  - Public semantic APIs accept `File` plus project-model selection where needed rather than a `Project` handle.
- `crates/djls-semantic/src/project/`:
  - Legacy input/sync/static scaffolding files are deleted or reduced to inspector runtime code that belongs to enrichment.
- `tests/lsp/test_startup.py`:
  - Real `djls serve` startup test sends `initialize`/`initialized`, observes fast handshake, opens a template while startup work is in progress, and observes server-initiated work-done progress when advertised or a log fallback when not.
- `crates/djls-project/tests/`:
  - Multisite, settings composition, static installed apps, template inventory, ignored files, cache-as-hint, and degraded-enrichment fixtures.
- `ARCHITECTURE.md` and `CONTEXT.md`:
  - Startup/readiness and crate responsibility sections describe `djls-project`, explicit source-file-set inputs, static Django Discovery, and enrichment-only Project Introspection.

### Validation
No public or internal semantic consumer depends on `Project` as a fact bag, `djls check` and LSP produce matching project-model outputs for the same fixture, the tiny real-LSP e2e proves handshake, in-progress request behavior, and progress/log fallback behavior, and architecture docs describe the new startup contract without referencing `refresh_external_data` as the startup extension point.
