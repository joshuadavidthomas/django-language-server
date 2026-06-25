use std::collections::BTreeMap;

use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

use crate::db::Db as ProjectDb;
use crate::names::LibraryName;
use crate::names::PyModuleName;
use crate::project::Project;
use crate::settings::StaticKnowledge;
use crate::settings::django_settings;
use crate::settings::installed_app_package_module;
use crate::settings::module_file;
use crate::settings::package_dir;
use crate::settings::settings_module_file;
use crate::templates::TemplateSymbolKind;

use super::registrations::TemplateLibraryAnalysis;
use super::symbols::TemplateSymbol;

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

    let mut libraries = TemplateLibraries {
        knowledge: match settings.installed_apps.knowledge {
            StaticKnowledge::Known => StaticKnowledge::Known,
            StaticKnowledge::Partial | StaticKnowledge::Unknown => StaticKnowledge::Partial,
        },
        ..TemplateLibraries::default()
    };

    if settings.templates.knowledge != StaticKnowledge::Known {
        libraries.knowledge = libraries.knowledge.demoted_to_partial();
    }

    let (knowledge, discovered_libraries) = templatetag_package_libraries(db, project, "django");
    libraries.knowledge = libraries.knowledge.weakened_by(knowledge);
    for (load_name, library) in discovered_libraries {
        libraries.insert_loadable(load_name, library);
    }

    if settings.installed_apps.knowledge != StaticKnowledge::Unknown {
        for installed_app in &settings.installed_apps.values {
            let (knowledge, discovered_libraries) = templatetag_package_libraries(
                db,
                project,
                installed_app_package_module(installed_app),
            );
            libraries.knowledge = libraries.knowledge.weakened_by(knowledge);
            for (load_name, library) in discovered_libraries {
                libraries.insert_loadable(load_name, library);
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
        libraries.knowledge = libraries.knowledge.weakened_by(backend.knowledge);

        for (load_name, module_path) in &backend.libraries {
            let Ok(load_name) = LibraryName::parse(load_name) else {
                libraries.knowledge = libraries.knowledge.demoted_to_partial();
                continue;
            };
            let (knowledge, library) = library_from_module_path(db, project, module_path);
            libraries.knowledge = libraries.knowledge.weakened_by(knowledge);
            if let Some(library) = library {
                libraries.insert_loadable(load_name, library);
            }
        }

        for module_path in DEFAULT_TEMPLATE_BUILTINS
            .iter()
            .copied()
            .chain(backend.builtins.iter().map(String::as_str))
        {
            let (knowledge, library) = library_from_module_path(db, project, module_path);
            libraries.knowledge = libraries.knowledge.weakened_by(knowledge);
            if let Some(library) = library {
                libraries.push_builtin(library);
            }
        }
    }

    libraries
}

fn templatetag_package_libraries(
    db: &dyn ProjectDb,
    project: Project,
    package_module: &str,
) -> (StaticKnowledge, Vec<(LibraryName, TemplateLibrary)>) {
    let mut knowledge = StaticKnowledge::Known;
    let mut libraries = Vec::new();

    if package_module.is_empty() {
        return (knowledge.demoted_to_partial(), libraries);
    }

    if PyModuleName::parse(package_module).is_err() {
        return (knowledge.demoted_to_partial(), libraries);
    }

    let Some(package_dir) = package_dir(db, project, package_module) else {
        return (knowledge.demoted_to_partial(), libraries);
    };

    let templatetags_dir = package_dir.join("templatetags");
    if !db.path_is_file(&templatetags_dir.join("__init__.py")) {
        return (knowledge, libraries);
    }

    let entries = match db.walk_entries(&templatetags_dir, &WalkOptions::shallow()) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Failed to walk template tag package {templatetags_dir}: {err}");
            return (knowledge.demoted_to_partial(), libraries);
        }
    };

    for entry in entries {
        if entry.kind != WalkEntryKind::File || entry.path.extension() != Some("py") {
            continue;
        }

        let Some(stem) = entry.path.file_stem() else {
            continue;
        };
        if stem.starts_with('_') {
            continue;
        }

        let Ok(load_name) = LibraryName::parse(stem) else {
            knowledge = knowledge.demoted_to_partial();
            continue;
        };
        let module_path = format!("{package_module}.templatetags.{stem}");
        let Ok(module) = PyModuleName::parse(&module_path) else {
            knowledge = knowledge.demoted_to_partial();
            continue;
        };

        let file = db.get_or_create_file(&entry.path);
        let analysis = TemplateLibraryAnalysis::from_file(db, file);
        if !analysis.defines_library && analysis.symbols.is_empty() {
            continue;
        }

        let mut library = TemplateLibrary::new(module);
        library.merge_symbols(analysis.symbols);
        libraries.push((load_name, library));
    }

    (knowledge, libraries)
}

fn library_from_module_path(
    db: &dyn ProjectDb,
    project: Project,
    module_path: &str,
) -> (StaticKnowledge, Option<TemplateLibrary>) {
    let Ok(module) = PyModuleName::parse(module_path) else {
        return (StaticKnowledge::Partial, None);
    };

    let mut library = TemplateLibrary::new(module);
    if let Some(file) = module_file(db, project, library.module().as_str()) {
        library.merge_symbols(TemplateLibraryAnalysis::from_file(db, file).symbols);
        (StaticKnowledge::Known, Some(library))
    } else {
        (StaticKnowledge::Partial, Some(library))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibrary {
    pub module: PyModuleName,
    pub symbols: Vec<TemplateSymbol>,
}

impl TemplateLibrary {
    #[must_use]
    pub fn new(module: PyModuleName) -> Self {
        Self {
            module,
            symbols: Vec::new(),
        }
    }

    #[must_use]
    pub fn module(&self) -> &PyModuleName {
        &self.module
    }

    pub fn merge_symbol(&mut self, new_symbol: TemplateSymbol) {
        if let Some(existing) = self
            .symbols
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

        self.symbols.push(new_symbol);
        self.symbols
            .sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
        self.symbols
            .dedup_by(|a, b| a.kind == b.kind && a.name == b.name);
    }

    pub(crate) fn merge_symbols(&mut self, symbols: impl IntoIterator<Item = TemplateSymbol>) {
        for symbol in symbols {
            self.merge_symbol(symbol);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstalledSymbolOrigin {
    Builtin { module: PyModuleName },
    Loadable { load_name: LibraryName },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledSymbolCandidate {
    pub symbol: TemplateSymbol,
    pub origin: InstalledSymbolOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraries {
    pub knowledge: StaticKnowledge,
    pub loadable: BTreeMap<LibraryName, TemplateLibrary>,
    pub builtins: Vec<TemplateLibrary>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            knowledge: StaticKnowledge::Unknown,
            loadable: BTreeMap::new(),
            builtins: Vec::new(),
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
    pub fn registration_modules(&self) -> Vec<PyModuleName> {
        if self.knowledge == StaticKnowledge::Unknown {
            return Vec::new();
        }

        let mut modules = Vec::new();

        for (_name, library) in self.loadable_libraries() {
            push_unique_module(&mut modules, library.module().clone());
        }

        for module in self.builtin_modules() {
            push_unique_module(&mut modules, module.clone());
        }

        modules
    }

    pub fn builtin_modules(&self) -> impl Iterator<Item = &PyModuleName> + '_ {
        self.builtins.iter().map(TemplateLibrary::module)
    }

    pub fn builtin_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.builtins.iter()
    }

    pub fn builtin_libraries_by_module(
        &self,
    ) -> impl Iterator<Item = (&PyModuleName, &TemplateLibrary)> + '_ {
        self.builtins
            .iter()
            .map(|library| (library.module(), library))
    }

    pub fn loadable_library_names(&self) -> impl Iterator<Item = &LibraryName> + '_ {
        self.loadable.keys()
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
        self.loadable.iter()
    }

    pub(crate) fn insert_loadable(&mut self, name: LibraryName, library: TemplateLibrary) {
        self.loadable.insert(name, library);
    }

    pub(crate) fn push_builtin(&mut self, library: TemplateLibrary) {
        if !self
            .builtins
            .iter()
            .any(|existing| existing.module() == library.module())
        {
            self.builtins.push(library);
        }
    }

    #[must_use]
    pub fn installed_symbol_candidates(
        &self,
        kind: TemplateSymbolKind,
    ) -> Vec<InstalledSymbolCandidate> {
        let mut candidates = Vec::new();
        let mut builtin_candidates = BTreeMap::new();

        for (module, library) in self.builtin_libraries_by_module() {
            for symbol in &library.symbols {
                if symbol.kind != kind {
                    continue;
                }

                builtin_candidates.insert(
                    symbol.name.clone(),
                    InstalledSymbolCandidate {
                        symbol: symbol.clone(),
                        origin: InstalledSymbolOrigin::Builtin {
                            module: module.clone(),
                        },
                    },
                );
            }
        }
        candidates.extend(builtin_candidates.into_values());

        for (name, library) in self.loadable_libraries() {
            for symbol in &library.symbols {
                if symbol.kind != kind {
                    continue;
                }

                candidates.push(InstalledSymbolCandidate {
                    symbol: symbol.clone(),
                    origin: InstalledSymbolOrigin::Loadable {
                        load_name: name.clone(),
                    },
                });
            }
        }

        candidates
    }

    #[must_use]
    pub fn loadable_library(&self, name: &LibraryName) -> Option<&TemplateLibrary> {
        self.loadable.get(name)
    }

    #[must_use]
    pub fn loadable_library_str(&self, name: &str) -> Option<&TemplateLibrary> {
        let name = LibraryName::parse(name).ok()?;
        self.loadable_library(&name)
    }

    #[must_use]
    pub fn loadable_library_module(&self, name: &LibraryName) -> Option<&PyModuleName> {
        self.loadable_library(name).map(TemplateLibrary::module)
    }

    #[must_use]
    pub fn loadable_library_module_str(&self, name: &str) -> Option<&PyModuleName> {
        let name = LibraryName::parse(name).ok()?;
        self.loadable_library_module(&name)
    }

    #[must_use]
    pub fn is_loadable(&self, name: &LibraryName) -> bool {
        self.loadable.contains_key(name)
    }

    #[must_use]
    pub fn is_loadable_str(&self, name: &str) -> bool {
        LibraryName::parse(name).is_ok_and(|name| self.is_loadable(&name))
    }
}

fn push_unique_module(modules: &mut Vec<PyModuleName>, module: PyModuleName) {
    if !modules.contains(&module) {
        modules.push(module);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::TemplateSymbolName;
    use crate::templates::SymbolDefinition;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
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

    fn builtin_library(module_name: &str, symbols: Vec<TemplateSymbol>) -> TemplateLibrary {
        let mut library = TemplateLibrary::new(module(module_name));
        for symbol in symbols {
            library.merge_symbol(symbol);
        }
        library
    }

    fn loadable_library(module_name: &str) -> TemplateLibrary {
        TemplateLibrary::new(module(module_name))
    }

    #[test]
    fn builtin_candidates_keep_last_builtin_symbol() {
        let mut libraries = TemplateLibraries {
            knowledge: StaticKnowledge::Known,
            ..TemplateLibraries::default()
        };
        let a_second = module("a_second");
        libraries.builtins.push(builtin_library(
            "z_first",
            vec![symbol(
                TemplateSymbolKind::Filter,
                "duplicate",
                Some("first"),
            )],
        ));
        libraries.builtins.push(builtin_library(
            a_second.as_str(),
            vec![symbol(
                TemplateSymbolKind::Filter,
                "duplicate",
                Some("second"),
            )],
        ));

        let candidates = libraries.installed_symbol_candidates(TemplateSymbolKind::Filter);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol.doc.as_deref(), Some("second"));
        assert_eq!(
            candidates[0].origin,
            InstalledSymbolOrigin::Builtin { module: a_second }
        );
    }

    #[test]
    fn registration_modules_keep_deterministic_precedence_order() {
        let mut libraries = TemplateLibraries {
            knowledge: StaticKnowledge::Known,
            ..TemplateLibraries::default()
        };
        libraries.loadable.insert(
            library_name("project_tags"),
            loadable_library("project.tags"),
        );
        for module_name in [
            "z.templatetags.tags",
            "django.template.defaulttags",
            "z.templatetags.tags",
        ] {
            let module = module(module_name);
            if !libraries
                .builtins
                .iter()
                .any(|library| library.module() == &module)
            {
                libraries
                    .builtins
                    .push(builtin_library(module.as_str(), Vec::new()));
            }
        }

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
        let mut libraries = TemplateLibraries {
            knowledge: StaticKnowledge::Partial,
            ..TemplateLibraries::default()
        };
        libraries.loadable.insert(
            library_name("project_tags"),
            loadable_library("project.tags"),
        );

        let modules: Vec<_> = libraries
            .registration_modules()
            .into_iter()
            .map(|module| module.as_str().to_string())
            .collect();

        assert_eq!(modules, vec!["project.tags"]);
    }
}
