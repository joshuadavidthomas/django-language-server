use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;

use djls_source::File;

use super::names::LibraryName;
use super::registrations::TemplateLibraryAnalysis;
use super::symbols::TemplateSymbol;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModulePath;
use crate::resolve::module_file;
use crate::settings::StaticKnowledge;
use crate::settings::django_settings;
use crate::settings::settings_module_file;
use crate::templates::TemplateSymbolKind;
use crate::templates::guess_package_module_from_installed_app_entry;
use crate::templates::templatetag_candidates;
use crate::templates::templatetag_package_candidates;

const DEFAULT_TEMPLATE_BUILTINS: &[&str] = &[
    "django.template.defaulttags",
    "django.template.defaultfilters",
    "django.template.loader_tags",
];

#[salsa::tracked(returns(ref))]
pub fn template_libraries(db: &dyn ProjectDb, project: Project) -> TemplateLibraries {
    project.touch_search_path_roots(db);

    if settings_module_file(db, project).is_none() {
        return TemplateLibraries::default();
    }

    let settings = django_settings(db, project);

    let mut libraries =
        TemplateLibraries::from_knowledge(match settings.installed_apps.knowledge {
            StaticKnowledge::Known => StaticKnowledge::Known,
            StaticKnowledge::Partial | StaticKnowledge::Unknown => StaticKnowledge::Partial,
        });

    if settings.templates.knowledge != StaticKnowledge::Known {
        libraries.demote_knowledge_to_partial();
    }

    let mut installed_template_library_modules = BTreeSet::new();

    let (knowledge, discovered_libraries) = templatetag_package_libraries(db, project, "django");
    libraries.weaken_knowledge(knowledge);
    for (load_name, source, library) in discovered_libraries {
        installed_template_library_modules.insert(library.module_path.clone());
        libraries.insert_loadable(load_name, source, library);
    }

    if settings.installed_apps.knowledge != StaticKnowledge::Unknown {
        for installed_app in &settings.installed_apps.values {
            let (knowledge, discovered_libraries) = templatetag_package_libraries(
                db,
                project,
                guess_package_module_from_installed_app_entry(installed_app),
            );
            libraries.weaken_knowledge(knowledge);
            for (load_name, source, library) in discovered_libraries {
                installed_template_library_modules.insert(library.module_path.clone());
                libraries.insert_loadable(load_name, source, library);
            }
        }
    }

    let backend_count = settings.templates.backends.len();
    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend(backend_count))
    {
        libraries.weaken_knowledge(backend.knowledge);

        for (load_name, module_path) in &backend.libraries {
            let Ok(load_name) = LibraryName::parse(load_name) else {
                libraries.demote_knowledge_to_partial();
                continue;
            };
            let (knowledge, library) = library_from_module_path(db, project, module_path);
            libraries.weaken_knowledge(knowledge);
            if let Some(library) = library {
                let source = LoadableLibrarySource::ConfiguredAlias;
                libraries.insert_loadable(load_name, source, library);
            }
        }

        for module_path in DEFAULT_TEMPLATE_BUILTINS.iter().copied() {
            let (knowledge, library) = library_from_module_path(db, project, module_path);
            libraries.weaken_knowledge(knowledge);
            if let Some(library) = library {
                libraries.push_builtin(BuiltinLibrarySource::DjangoDefault, library);
            }
        }

        for module_path in backend.builtins.iter().map(String::as_str) {
            let (knowledge, library) = library_from_module_path(db, project, module_path);
            libraries.weaken_knowledge(knowledge);
            if let Some(library) = library {
                let source = BuiltinLibrarySource::Configured;
                libraries.push_builtin(source, library);
            }
        }
    }

    libraries.insert_inactive_candidates(db, project, &installed_template_library_modules);
    libraries
}

fn templatetag_package_libraries(
    db: &dyn ProjectDb,
    project: Project,
    package_module: &str,
) -> (
    StaticKnowledge,
    Vec<(LibraryName, LoadableLibrarySource, AnalyzedTemplateLibrary)>,
) {
    let Ok(source_module) = PythonModulePath::parse(package_module) else {
        return (StaticKnowledge::Partial, Vec::new());
    };
    let source = LoadableLibrarySource::InstalledApp(source_module);
    let (knowledge, candidates) =
        templatetag_package_candidates(db, project, package_module).into_parts();
    let mut libraries = Vec::new();

    for candidate in candidates {
        let analysis = TemplateLibraryAnalysis::from_file(db, candidate.file);
        if !analysis.defines_library && analysis.symbols.is_empty() {
            continue;
        }

        libraries.push((
            candidate.name,
            source.clone(),
            AnalyzedTemplateLibrary::resolved(candidate.module, candidate.file, analysis),
        ));
    }

    (knowledge, libraries)
}

fn library_from_module_path(
    db: &dyn ProjectDb,
    project: Project,
    module_path: &str,
) -> (StaticKnowledge, Option<AnalyzedTemplateLibrary>) {
    let Ok(module) = PythonModulePath::parse(module_path) else {
        return (StaticKnowledge::Partial, None);
    };

    if let Some(file) = module_file(db, project, module.as_str()) {
        let analysis = TemplateLibraryAnalysis::from_file(db, file);
        (
            StaticKnowledge::Known,
            Some(AnalyzedTemplateLibrary::resolved(module, file, analysis)),
        )
    } else {
        (
            StaticKnowledge::Partial,
            Some(AnalyzedTemplateLibrary::unresolved(
                module.clone(),
                TemplateLibraryResolutionError::ModuleNotFound(module),
            )),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AnalyzedTemplateLibrary {
    module_path: PythonModulePath,
    resolution: TemplateLibraryResolution,
    defines_library: bool,
    symbols: Vec<TemplateSymbol>,
}

impl AnalyzedTemplateLibrary {
    fn resolved(
        module_path: PythonModulePath,
        file: File,
        analysis: TemplateLibraryAnalysis,
    ) -> Self {
        Self {
            module_path,
            resolution: TemplateLibraryResolution::Resolved(file),
            defines_library: analysis.defines_library,
            symbols: analysis.symbols,
        }
    }

    fn unresolved(module_path: PythonModulePath, error: TemplateLibraryResolutionError) -> Self {
        Self {
            module_path,
            resolution: TemplateLibraryResolution::Unresolved(error),
            defines_library: false,
            symbols: Vec::new(),
        }
    }

    fn untracked(
        module_path: PythonModulePath,
        defines_library: bool,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self {
            module_path,
            resolution: TemplateLibraryResolution::Untracked,
            defines_library,
            symbols,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TemplateLibraryId {
    kind: TemplateLibraryIdKind,
    occurrence: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum TemplateLibraryIdKind {
    Builtin(PythonModulePath),
    Loadable {
        name: LibraryName,
        module_path: PythonModulePath,
    },
    Inactive {
        name: LibraryName,
        app: PythonModulePath,
        module_path: PythonModulePath,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateLibraryStatus {
    Builtin(BuiltinLibrarySource),
    Loadable(LoadableLibrarySource),
    Inactive(InactiveLibrarySource),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuiltinLibrarySource {
    DjangoDefault,
    Configured,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadableLibrarySource {
    InstalledApp(PythonModulePath),
    ConfiguredAlias,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InactiveLibrarySource {
    app: PythonModulePath,
}

impl InactiveLibrarySource {
    #[must_use]
    pub fn new(app: PythonModulePath) -> Self {
        Self { app }
    }

    #[must_use]
    pub fn app(&self) -> &PythonModulePath {
        &self.app
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum TemplateLibraryResolution {
    Resolved(File),
    Untracked,
    Unresolved(TemplateLibraryResolutionError),
}

#[derive(Clone, PartialEq, Eq)]
pub enum TemplateLibraryResolutionError {
    InvalidModulePath(String),
    ModuleNotFound(PythonModulePath),
    NotATemplateLibrary(File),
}

impl fmt::Debug for TemplateLibraryResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolved(_) => f.debug_tuple("Resolved").field(&"File").finish(),
            Self::Untracked => f.write_str("Untracked"),
            Self::Unresolved(error) => f.debug_tuple("Unresolved").field(error).finish(),
        }
    }
}

impl fmt::Debug for TemplateLibraryResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidModulePath(path) => {
                f.debug_tuple("InvalidModulePath").field(path).finish()
            }
            Self::ModuleNotFound(module) => f.debug_tuple("ModuleNotFound").field(module).finish(),
            Self::NotATemplateLibrary(_) => {
                f.debug_tuple("NotATemplateLibrary").field(&"File").finish()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibrary {
    id: TemplateLibraryId,
    load_name: Option<LibraryName>,
    module_path: PythonModulePath,
    status: TemplateLibraryStatus,
    resolution: TemplateLibraryResolution,
    defines_library: bool,
    symbols: Vec<TemplateSymbol>,
}

impl TemplateLibrary {
    fn new(
        id: TemplateLibraryId,
        load_name: Option<LibraryName>,
        module_path: PythonModulePath,
        status: TemplateLibraryStatus,
        resolution: TemplateLibraryResolution,
        defines_library: bool,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self {
            id,
            load_name,
            module_path,
            status,
            resolution,
            defines_library,
            symbols: merge_symbols(symbols),
        }
    }

    #[must_use]
    pub fn id(&self) -> &TemplateLibraryId {
        &self.id
    }

    #[must_use]
    pub fn load_name(&self) -> Option<&LibraryName> {
        self.load_name.as_ref()
    }

    #[must_use]
    pub fn module_path(&self) -> &PythonModulePath {
        &self.module_path
    }

    #[must_use]
    pub fn status(&self) -> &TemplateLibraryStatus {
        &self.status
    }

    #[must_use]
    pub fn resolution(&self) -> &TemplateLibraryResolution {
        &self.resolution
    }

    #[must_use]
    pub fn resolved_file(&self) -> Option<File> {
        match self.resolution {
            TemplateLibraryResolution::Resolved(file) => Some(file),
            TemplateLibraryResolution::Untracked | TemplateLibraryResolution::Unresolved(_) => None,
        }
    }

    #[must_use]
    pub fn defines_library(&self) -> bool {
        self.defines_library
    }

    #[must_use]
    pub fn symbols(&self) -> &[TemplateSymbol] {
        &self.symbols
    }

    pub fn tags(&self) -> impl Iterator<Item = &TemplateSymbol> + '_ {
        self.symbols
            .iter()
            .filter(|symbol| symbol.kind == TemplateSymbolKind::Tag)
    }

    pub fn filters(&self) -> impl Iterator<Item = &TemplateSymbol> + '_ {
        self.symbols
            .iter()
            .filter(|symbol| symbol.kind == TemplateSymbolKind::Filter)
    }

    #[must_use]
    pub fn inactive_app(&self) -> Option<&PythonModulePath> {
        match &self.status {
            TemplateLibraryStatus::Inactive(source) => Some(source.app()),
            TemplateLibraryStatus::Builtin(_) | TemplateLibraryStatus::Loadable(_) => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedTemplateLibrary<'a> {
    library: &'a TemplateLibrary,
    file: File,
}

impl fmt::Debug for ResolvedTemplateLibrary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedTemplateLibrary")
            .field("library", &self.library)
            .field("file", &"File")
            .finish()
    }
}

impl<'a> ResolvedTemplateLibrary<'a> {
    #[must_use]
    pub fn library(&self) -> &'a TemplateLibrary {
        self.library
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }

    #[must_use]
    pub fn module_path(&self) -> &PythonModulePath {
        self.library.module_path()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstalledSymbolOrigin {
    Builtin {
        id: TemplateLibraryId,
        module: PythonModulePath,
    },
    Loadable {
        id: TemplateLibraryId,
        load_name: LibraryName,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledSymbolCandidate {
    pub symbol: TemplateSymbol,
    pub origin: InstalledSymbolOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraries {
    knowledge: StaticKnowledge,
    records: BTreeMap<TemplateLibraryId, TemplateLibrary>,
    builtin_order: Vec<TemplateLibraryId>,
    loadable_by_name: BTreeMap<LibraryName, TemplateLibraryId>,
    inactive_by_name: BTreeMap<LibraryName, Vec<TemplateLibraryId>>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            knowledge: StaticKnowledge::Unknown,
            records: BTreeMap::new(),
            builtin_order: Vec::new(),
            loadable_by_name: BTreeMap::new(),
            inactive_by_name: BTreeMap::new(),
        }
    }
}

impl TemplateLibraries {
    #[must_use]
    pub fn empty_ref() -> &'static Self {
        static EMPTY: std::sync::LazyLock<TemplateLibraries> =
            std::sync::LazyLock::new(TemplateLibraries::default);
        &EMPTY
    }

    #[must_use]
    pub fn builder() -> TemplateLibrariesBuilder {
        TemplateLibrariesBuilder {
            libraries: TemplateLibraries::default(),
        }
    }

    fn from_knowledge(knowledge: StaticKnowledge) -> Self {
        Self {
            knowledge,
            ..Self::default()
        }
    }

    fn weaken_knowledge(&mut self, knowledge: StaticKnowledge) {
        self.knowledge = self.knowledge.weakened_by(knowledge);
    }

    fn demote_knowledge_to_partial(&mut self) {
        self.knowledge = self.knowledge.demoted_to_partial();
    }

    #[must_use]
    pub fn knowledge(&self) -> StaticKnowledge {
        self.knowledge
    }

    #[must_use]
    pub fn with_knowledge(mut self, knowledge: StaticKnowledge) -> Self {
        self.knowledge = knowledge;
        self
    }

    #[must_use]
    pub fn should_report_unknown_symbols(&self) -> bool {
        self.knowledge == StaticKnowledge::Known
    }

    #[must_use]
    pub fn get(&self, id: &TemplateLibraryId) -> Option<&TemplateLibrary> {
        self.records.get(id)
    }

    pub fn records(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.records.values()
    }

    #[must_use]
    pub(crate) fn registration_modules(&self) -> Vec<PythonModulePath> {
        if self.knowledge == StaticKnowledge::Unknown {
            return Vec::new();
        }

        let mut modules = Vec::new();

        for (_name, library) in self.loadable_libraries() {
            push_unique_module(&mut modules, library.module_path().clone());
        }

        for module in self.builtin_modules() {
            push_unique_module(&mut modules, module.clone());
        }

        modules
    }

    pub fn builtin_modules(&self) -> impl Iterator<Item = &PythonModulePath> + '_ {
        self.builtin_libraries().map(TemplateLibrary::module_path)
    }

    pub fn builtin_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.builtin_order
            .iter()
            .filter_map(|id| self.records.get(id))
    }

    fn loadable_library_names(&self) -> impl Iterator<Item = &LibraryName> + '_ {
        self.loadable_by_name.keys()
    }

    #[must_use]
    pub fn completion_library_names(&self) -> Vec<LibraryName> {
        let mut names: Vec<LibraryName> = self.loadable_library_names().cloned().collect();

        names.sort();
        names.dedup();
        names
    }

    pub fn loadable_libraries(
        &self,
    ) -> impl Iterator<Item = (&LibraryName, &TemplateLibrary)> + '_ {
        self.loadable_by_name
            .iter()
            .filter_map(|(name, id)| self.records.get(id).map(|library| (name, library)))
    }

    fn insert_loadable(
        &mut self,
        name: LibraryName,
        source: LoadableLibrarySource,
        analyzed: AnalyzedTemplateLibrary,
    ) {
        let id = self.next_id(TemplateLibraryIdKind::Loadable {
            name: name.clone(),
            module_path: analyzed.module_path.clone(),
        });
        let library = TemplateLibrary::new(
            id.clone(),
            Some(name.clone()),
            analyzed.module_path,
            TemplateLibraryStatus::Loadable(source),
            analyzed.resolution,
            analyzed.defines_library,
            analyzed.symbols,
        );
        if let Some(previous) = self.loadable_by_name.insert(name, id.clone()) {
            self.records.remove(&previous);
        }
        self.records.insert(id, library);
    }

    fn push_builtin(&mut self, source: BuiltinLibrarySource, analyzed: AnalyzedTemplateLibrary) {
        let id = self.next_id(TemplateLibraryIdKind::Builtin(analyzed.module_path.clone()));
        let library = TemplateLibrary::new(
            id.clone(),
            None,
            analyzed.module_path,
            TemplateLibraryStatus::Builtin(source),
            analyzed.resolution,
            analyzed.defines_library,
            analyzed.symbols,
        );
        self.records.insert(id.clone(), library);
        self.builtin_order.push(id);
    }

    fn insert_inactive_candidates(
        &mut self,
        db: &dyn ProjectDb,
        project: Project,
        installed_template_library_modules: &BTreeSet<PythonModulePath>,
    ) {
        let mut excluded_modules = self.active_modules();
        excluded_modules.extend(installed_template_library_modules.iter().cloned());

        for candidate in templatetag_candidates(db, project).iter().cloned() {
            if excluded_modules.contains(&candidate.module) {
                continue;
            }

            let analysis = TemplateLibraryAnalysis::from_file(db, candidate.file);
            if !analysis.defines_library && analysis.symbols.is_empty() {
                continue;
            }

            self.insert_inactive(
                candidate.name,
                candidate.app,
                AnalyzedTemplateLibrary::resolved(candidate.module, candidate.file, analysis),
            );
        }

        self.sort_and_dedup_inactive();
    }

    fn active_modules(&self) -> BTreeSet<PythonModulePath> {
        self.loadable_libraries()
            .map(|(_name, library)| library.module_path().clone())
            .chain(self.builtin_modules().cloned())
            .collect()
    }

    fn insert_inactive(
        &mut self,
        name: LibraryName,
        app: PythonModulePath,
        analyzed: AnalyzedTemplateLibrary,
    ) {
        let kind = TemplateLibraryIdKind::Inactive {
            name: name.clone(),
            app: app.clone(),
            module_path: analyzed.module_path.clone(),
        };
        if let Some(existing_id) = self.records.keys().find(|id| id.kind == kind).cloned() {
            let ids = self.inactive_by_name.entry(name).or_default();
            if !ids.contains(&existing_id) {
                ids.push(existing_id);
            }
            return;
        }

        let id = self.next_id(kind);
        let library = TemplateLibrary::new(
            id.clone(),
            Some(name.clone()),
            analyzed.module_path,
            TemplateLibraryStatus::Inactive(InactiveLibrarySource::new(app)),
            analyzed.resolution,
            analyzed.defines_library,
            analyzed.symbols,
        );
        self.records.insert(id.clone(), library);
        self.inactive_by_name.entry(name).or_default().push(id);
    }

    fn sort_and_dedup_inactive(&mut self) {
        let records = &self.records;
        for ids in self.inactive_by_name.values_mut() {
            ids.sort_by(|left, right| cmp_inactive_ids(records, left, right));
            ids.dedup_by(|left, right| same_inactive_ids(records, left, right));
        }
    }

    fn next_id(&self, kind: TemplateLibraryIdKind) -> TemplateLibraryId {
        let occurrence = self
            .records
            .keys()
            .filter(|id| id.kind == kind)
            .count()
            .try_into()
            .expect("template library occurrence count should fit in u32");
        TemplateLibraryId { kind, occurrence }
    }

    #[must_use]
    pub fn installed_symbol_candidates(
        &self,
        kind: TemplateSymbolKind,
    ) -> Vec<InstalledSymbolCandidate> {
        let mut candidates = Vec::new();
        let mut builtin_candidates = BTreeMap::new();

        for library in self.builtin_libraries() {
            for symbol in library.symbols() {
                if symbol.kind != kind {
                    continue;
                }

                builtin_candidates.insert(
                    symbol.name.clone(),
                    InstalledSymbolCandidate {
                        symbol: symbol.clone(),
                        origin: InstalledSymbolOrigin::Builtin {
                            id: library.id().clone(),
                            module: library.module_path().clone(),
                        },
                    },
                );
            }
        }
        candidates.extend(builtin_candidates.into_values());

        for (name, library) in self.loadable_libraries() {
            for symbol in library.symbols() {
                if symbol.kind != kind {
                    continue;
                }

                candidates.push(InstalledSymbolCandidate {
                    symbol: symbol.clone(),
                    origin: InstalledSymbolOrigin::Loadable {
                        id: library.id().clone(),
                        load_name: name.clone(),
                    },
                });
            }
        }

        candidates
    }

    #[must_use]
    pub fn loadable_library(&self, name: &LibraryName) -> Option<&TemplateLibrary> {
        self.loadable_by_name
            .get(name)
            .and_then(|id| self.records.get(id))
    }

    #[must_use]
    pub fn loadable_library_str(&self, name: &str) -> Option<&TemplateLibrary> {
        let name = LibraryName::parse(name).ok()?;
        self.loadable_library(&name)
    }

    #[must_use]
    pub fn loadable_library_module(&self, name: &LibraryName) -> Option<&PythonModulePath> {
        self.loadable_library(name)
            .map(TemplateLibrary::module_path)
    }

    #[must_use]
    pub fn is_loadable(&self, name: &LibraryName) -> bool {
        self.loadable_by_name.contains_key(name)
    }

    #[must_use]
    pub fn is_loadable_str(&self, name: &str) -> bool {
        LibraryName::parse(name).is_ok_and(|name| self.is_loadable(&name))
    }

    #[must_use]
    pub fn inactive_library_candidates(&self, name: &LibraryName) -> Vec<&TemplateLibrary> {
        self.inactive_by_name
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|id| self.records.get(id))
            .collect()
    }

    #[must_use]
    pub fn inactive_tag_candidates(&self, tag: &str) -> Vec<&TemplateLibrary> {
        self.inactive_symbol_candidates(tag, TemplateSymbolKind::Tag)
    }

    #[must_use]
    pub fn inactive_filter_candidates(&self, filter: &str) -> Vec<&TemplateLibrary> {
        self.inactive_symbol_candidates(filter, TemplateSymbolKind::Filter)
    }

    fn inactive_symbol_candidates(
        &self,
        symbol_name: &str,
        kind: TemplateSymbolKind,
    ) -> Vec<&TemplateLibrary> {
        let mut candidates: Vec<_> = self
            .inactive_by_name
            .values()
            .flatten()
            .filter_map(|id| self.records.get(id))
            .filter(|library| {
                library
                    .symbols()
                    .iter()
                    .any(|symbol| symbol.kind == kind && symbol.name.as_str() == symbol_name)
            })
            .collect();
        candidates.sort_by(|left, right| cmp_inactive_libraries(left, right));
        candidates
    }

    pub fn resolved_active_libraries(
        &self,
    ) -> impl Iterator<Item = ResolvedTemplateLibrary<'_>> + '_ {
        let mut ids = self.builtin_order.clone();
        let mut seen_loadable = BTreeSet::new();
        ids.extend(
            self.loadable_by_name
                .values()
                .filter(|id| seen_loadable.insert((*id).clone()))
                .cloned(),
        );

        ids.into_iter().filter_map(|id| {
            let library = self.records.get(&id)?;
            let file = library.resolved_file()?;
            Some(ResolvedTemplateLibrary { library, file })
        })
    }
}

pub struct TemplateLibrariesBuilder {
    libraries: TemplateLibraries,
}

impl TemplateLibrariesBuilder {
    #[must_use]
    pub fn knowledge(mut self, knowledge: StaticKnowledge) -> Self {
        self.libraries.knowledge = knowledge;
        self
    }

    #[must_use]
    pub fn loadable_resolved(
        mut self,
        name: LibraryName,
        source: LoadableLibrarySource,
        module_path: PythonModulePath,
        file: File,
        defines_library: bool,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        self.libraries.insert_loadable(
            name,
            source,
            AnalyzedTemplateLibrary {
                module_path,
                resolution: TemplateLibraryResolution::Resolved(file),
                defines_library,
                symbols,
            },
        );
        self
    }

    #[must_use]
    pub fn loadable_untracked(
        mut self,
        name: LibraryName,
        source: LoadableLibrarySource,
        module_path: PythonModulePath,
        defines_library: bool,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        self.libraries.insert_loadable(
            name,
            source,
            AnalyzedTemplateLibrary::untracked(module_path, defines_library, symbols),
        );
        self
    }

    #[must_use]
    pub fn loadable_unresolved(
        mut self,
        name: LibraryName,
        source: LoadableLibrarySource,
        module_path: PythonModulePath,
        error: TemplateLibraryResolutionError,
    ) -> Self {
        self.libraries.insert_loadable(
            name,
            source,
            AnalyzedTemplateLibrary::unresolved(module_path, error),
        );
        self
    }

    #[must_use]
    pub fn builtin_resolved(
        mut self,
        source: BuiltinLibrarySource,
        module_path: PythonModulePath,
        file: File,
        defines_library: bool,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        self.libraries.push_builtin(
            source,
            AnalyzedTemplateLibrary {
                module_path,
                resolution: TemplateLibraryResolution::Resolved(file),
                defines_library,
                symbols,
            },
        );
        self
    }

    #[must_use]
    pub fn builtin_untracked(
        mut self,
        source: BuiltinLibrarySource,
        module_path: PythonModulePath,
        defines_library: bool,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        self.libraries.push_builtin(
            source,
            AnalyzedTemplateLibrary::untracked(module_path, defines_library, symbols),
        );
        self
    }

    #[must_use]
    pub fn builtin_unresolved(
        mut self,
        source: BuiltinLibrarySource,
        module_path: PythonModulePath,
        error: TemplateLibraryResolutionError,
    ) -> Self {
        self.libraries.push_builtin(
            source,
            AnalyzedTemplateLibrary::unresolved(module_path, error),
        );
        self
    }

    #[must_use]
    pub fn inactive_resolved(
        mut self,
        name: LibraryName,
        app: PythonModulePath,
        module_path: PythonModulePath,
        file: File,
        defines_library: bool,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        self.libraries.insert_inactive(
            name,
            app,
            AnalyzedTemplateLibrary {
                module_path,
                resolution: TemplateLibraryResolution::Resolved(file),
                defines_library,
                symbols,
            },
        );
        self
    }

    #[must_use]
    pub fn inactive_untracked(
        mut self,
        name: LibraryName,
        app: PythonModulePath,
        module_path: PythonModulePath,
        defines_library: bool,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        self.libraries.insert_inactive(
            name,
            app,
            AnalyzedTemplateLibrary::untracked(module_path, defines_library, symbols),
        );
        self
    }

    #[must_use]
    pub fn build(mut self) -> TemplateLibraries {
        self.libraries.sort_and_dedup_inactive();
        self.libraries
    }
}

fn cmp_inactive_ids(
    records: &BTreeMap<TemplateLibraryId, TemplateLibrary>,
    left: &TemplateLibraryId,
    right: &TemplateLibraryId,
) -> Ordering {
    let Some(left) = records.get(left) else {
        return Ordering::Equal;
    };
    let Some(right) = records.get(right) else {
        return Ordering::Equal;
    };
    cmp_inactive_libraries(left, right)
}

fn same_inactive_ids(
    records: &BTreeMap<TemplateLibraryId, TemplateLibrary>,
    left: &TemplateLibraryId,
    right: &TemplateLibraryId,
) -> bool {
    let Some(left) = records.get(left) else {
        return false;
    };
    let Some(right) = records.get(right) else {
        return false;
    };
    same_inactive_library(left, right)
}

fn cmp_inactive_libraries(left: &TemplateLibrary, right: &TemplateLibrary) -> Ordering {
    left.inactive_app()
        .cmp(&right.inactive_app())
        .then_with(|| left.load_name().cmp(&right.load_name()))
        .then_with(|| left.module_path().cmp(right.module_path()))
}

fn same_inactive_library(left: &TemplateLibrary, right: &TemplateLibrary) -> bool {
    let (Some(left_app), Some(right_app)) = (left.inactive_app(), right.inactive_app()) else {
        return false;
    };

    left_app == right_app && left.module_path() == right.module_path()
}

fn merge_symbols(symbols: Vec<TemplateSymbol>) -> Vec<TemplateSymbol> {
    let mut merged = Vec::new();
    for symbol in symbols {
        merge_symbol(&mut merged, symbol);
    }
    merged
}

fn merge_symbol(symbols: &mut Vec<TemplateSymbol>, new_symbol: TemplateSymbol) {
    if let Some(existing) = symbols
        .iter_mut()
        .find(|sym| sym.kind == new_symbol.kind && sym.name == new_symbol.name)
    {
        if existing.doc.is_none() {
            existing.doc = new_symbol.doc;
        }

        if new_symbol.definition.rank() > existing.definition.rank() {
            existing.definition = new_symbol.definition;
        }

        return;
    }

    symbols.push(new_symbol);
    symbols.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    symbols.dedup_by(|a, b| a.kind == b.kind && a.name == b.name);
}

fn push_unique_module(modules: &mut Vec<PythonModulePath>, module: PythonModulePath) {
    if !modules.contains(&module) {
        modules.push(module);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::SymbolDefinition;
    use crate::templates::names::TemplateSymbolName;

    fn module(name: &str) -> PythonModulePath {
        PythonModulePath::parse(name).unwrap()
    }

    fn library_name(name: &str) -> LibraryName {
        LibraryName::parse(name).unwrap()
    }

    fn symbol(kind: TemplateSymbolKind, name: &str, doc: Option<&str>) -> TemplateSymbol {
        TemplateSymbol {
            kind,
            name: TemplateSymbolName::parse(name).unwrap(),
            definition: SymbolDefinition::Unknown,
            doc: doc.map(str::to_string),
        }
    }

    #[test]
    fn builtin_candidates_keep_last_builtin_symbol() {
        let a_second = module("a_second");
        let libraries = TemplateLibraries::builder()
            .knowledge(StaticKnowledge::Known)
            .builtin_untracked(
                BuiltinLibrarySource::DjangoDefault,
                module("z_first"),
                true,
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("first"),
                )],
            )
            .builtin_untracked(
                BuiltinLibrarySource::DjangoDefault,
                a_second.clone(),
                true,
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("second"),
                )],
            )
            .build();

        let candidates = libraries.installed_symbol_candidates(TemplateSymbolKind::Filter);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol.doc.as_deref(), Some("second"));
        assert!(matches!(
            &candidates[0].origin,
            InstalledSymbolOrigin::Builtin { module, .. } if module == &a_second
        ));
    }

    #[test]
    fn registration_modules_keep_deterministic_precedence_order() {
        let libraries = TemplateLibraries::builder()
            .knowledge(StaticKnowledge::Known)
            .loadable_untracked(
                library_name("project_tags"),
                LoadableLibrarySource::ConfiguredAlias,
                module("project.tags"),
                true,
                Vec::new(),
            )
            .builtin_untracked(
                BuiltinLibrarySource::DjangoDefault,
                module("z.templatetags.tags"),
                true,
                Vec::new(),
            )
            .builtin_untracked(
                BuiltinLibrarySource::DjangoDefault,
                module("django.template.defaulttags"),
                true,
                Vec::new(),
            )
            .builtin_untracked(
                BuiltinLibrarySource::DjangoDefault,
                module("z.templatetags.tags"),
                true,
                Vec::new(),
            )
            .build();

        let modules: Vec<_> = libraries
            .registration_modules()
            .into_iter()
            .map(|module| module.as_str().to_string())
            .collect();

        assert_eq!(
            modules,
            vec![
                "project.tags",
                "z.templatetags.tags",
                "django.template.defaulttags",
            ]
        );
    }

    #[test]
    fn registration_modules_keep_known_partial_modules() {
        let libraries = TemplateLibraries::builder()
            .knowledge(StaticKnowledge::Partial)
            .loadable_untracked(
                library_name("project_tags"),
                LoadableLibrarySource::ConfiguredAlias,
                module("project.tags"),
                true,
                Vec::new(),
            )
            .build();

        let modules: Vec<_> = libraries
            .registration_modules()
            .into_iter()
            .map(|module| module.as_str().to_string())
            .collect();

        assert_eq!(modules, vec!["project.tags"]);
    }
}
