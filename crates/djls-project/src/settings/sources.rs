use std::collections::BTreeSet;

use camino::Utf8Path;
use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::ModuleImport;
use crate::python::ModuleImportResolver;
use crate::python::ModuleSource;
use crate::python::PythonImport;
use crate::python::PythonModule;
use crate::python::SearchPath;
use crate::python::resolve_module_detail;
use crate::settings::DjangoSettings;
use crate::settings::extraction::extract_settings;

pub(super) fn django_settings_from_file(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> DjangoSettings {
    let mut context = SettingsImportContext::tracked(db, project);
    let Some(source) = context.read_source(file) else {
        return DjangoSettings::default();
    };
    extract_settings(&source, &mut context)
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DjangoSettingsSources(Vec<File>);

impl DjangoSettingsSources {
    fn from_files(db: &dyn ProjectDb, files: impl IntoIterator<Item = File>) -> Self {
        let mut seen = BTreeSet::new();
        let mut deduped = Vec::new();
        for file in files {
            if seen.insert(file.path(db).to_path_buf()) {
                deduped.push(file);
            }
        }

        Self(deduped)
    }

    pub(crate) fn files(&self) -> &[File] {
        &self.0
    }

    pub(crate) fn root(&self) -> Option<File> {
        self.0.first().copied()
    }
}

pub(crate) fn settings_sources(db: &dyn ProjectDb, project: Project) -> DjangoSettingsSources {
    let Some(file) = crate::settings::settings_module_file(db, project) else {
        return DjangoSettingsSources::from_files(db, []);
    };

    // The Django Discovery bump set must cover the same settings source graph
    // the extractor would read against current disk content.
    let mut context = SettingsImportContext::discovery(db, project);
    let Some(source) = context.read_source(file) else {
        return DjangoSettingsSources::from_files(db, [file]);
    };
    let _ = extract_settings(&source, &mut context);

    DjangoSettingsSources::from_files(db, std::iter::once(file).chain(context.resolved))
}

enum SettingsReadMode {
    Tracked,
    Discovery,
}

struct SettingsImportContext<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    mode: SettingsReadMode,
    resolved: Vec<File>,
}

impl<'db> SettingsImportContext<'db> {
    fn tracked(db: &'db dyn ProjectDb, project: Project) -> Self {
        Self {
            db,
            project,
            mode: SettingsReadMode::Tracked,
            resolved: Vec::new(),
        }
    }

    fn discovery(db: &'db dyn ProjectDb, project: Project) -> Self {
        Self {
            db,
            project,
            mode: SettingsReadMode::Discovery,
            resolved: Vec::new(),
        }
    }

    fn read_source(&mut self, file: File) -> Option<ModuleSource> {
        let source = match self.mode {
            SettingsReadMode::Tracked => file.source(self.db).as_str().to_string(),
            SettingsReadMode::Discovery => self.db.read_file(file.path(self.db)).ok()?,
        };
        Some(ModuleSource::new(
            file,
            file.path(self.db).to_path_buf(),
            source,
        ))
    }
}

impl ModuleImportResolver for SettingsImportContext<'_> {
    fn resolve_star_import(
        &mut self,
        import: &ModuleImport,
        importer: &Utf8Path,
    ) -> Option<ModuleSource> {
        let module = self.resolve_python_import(import, importer)?;
        self.read_resolved_module(&module)
    }

    fn resolve_named_import(
        &mut self,
        import: &ModuleImport,
        importer: &Utf8Path,
    ) -> Option<ModuleSource> {
        let module = self.resolve_python_import(import, importer)?;
        let detail = resolve_module_detail(self.db, self.project, module.name().clone());
        if !detail
            .selected_root
            .as_ref()
            .is_some_and(SearchPath::is_first_party)
        {
            return None;
        }

        self.read_resolved_module(&module)
    }
}

impl SettingsImportContext<'_> {
    fn resolve_python_import(
        &mut self,
        import: &ModuleImport,
        importer: &Utf8Path,
    ) -> Option<PythonModule> {
        let import = PythonImport {
            level: import.level,
            module: import.module.as_deref(),
            importer,
        };
        PythonModule::resolve_import(self.db, self.project, import).ok()?
    }

    fn read_resolved_module(&mut self, module: &PythonModule) -> Option<ModuleSource> {
        let file = module.file();
        self.resolved.push(file);
        self.read_source(file)
    }
}
