use std::collections::BTreeSet;

use camino::Utf8Path;
use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonImport;
use crate::python::PythonModule;
use crate::python::SearchPath;
use crate::python::resolve_module_detail;
use crate::settings::DjangoSettings;
use crate::settings::SettingsImport;
use crate::settings::SettingsSource;
use crate::settings::SettingsSourceResolver;
use crate::settings::extract_settings;

pub(super) fn django_settings_from_file(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> DjangoSettings {
    let mut reader = TrackedSettingsSourceReader { db };
    let Some(source) = reader.read_source(file) else {
        return DjangoSettings::default();
    };
    let mut resolver = ReaderResolver::new(db, project, reader);
    extract_settings(source.source.as_str(), &source.path, &mut resolver)
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
    let mut reader = DiscoverySettingsSourceReader { db };
    let Some(source) = reader.read_source(file) else {
        return DjangoSettingsSources::from_files(db, [file]);
    };
    let mut resolver = ReaderResolver::new(db, project, reader);
    let _ = extract_settings(source.source.as_str(), &source.path, &mut resolver);

    DjangoSettingsSources::from_files(db, std::iter::once(file).chain(resolver.resolved))
}

trait SettingsSourceReader {
    fn read_source(&mut self, file: File) -> Option<SettingsSource>;
}

struct TrackedSettingsSourceReader<'db> {
    db: &'db dyn ProjectDb,
}

impl SettingsSourceReader for TrackedSettingsSourceReader<'_> {
    fn read_source(&mut self, file: File) -> Option<SettingsSource> {
        Some(SettingsSource {
            source: file.source(self.db).as_str().to_string(),
            path: file.path(self.db).to_path_buf(),
        })
    }
}

struct DiscoverySettingsSourceReader<'db> {
    db: &'db dyn ProjectDb,
}

impl SettingsSourceReader for DiscoverySettingsSourceReader<'_> {
    fn read_source(&mut self, file: File) -> Option<SettingsSource> {
        let path = file.path(self.db).to_path_buf();
        let source = self.db.read_file(&path).ok()?;
        Some(SettingsSource { source, path })
    }
}

struct ReaderResolver<'db, R> {
    db: &'db dyn ProjectDb,
    project: Project,
    reader: R,
    resolved: Vec<File>,
}

impl<'db, R> ReaderResolver<'db, R> {
    fn new(db: &'db dyn ProjectDb, project: Project, reader: R) -> Self {
        Self {
            db,
            project,
            reader,
            resolved: Vec::new(),
        }
    }
}

impl<R: SettingsSourceReader> SettingsSourceResolver for ReaderResolver<'_, R> {
    fn resolve_star_import(
        &mut self,
        import: &SettingsImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource> {
        let module = self.resolve_python_import(import, importer)?;
        self.read_resolved_module(&module)
    }

    fn resolve_named_import(
        &mut self,
        import: &SettingsImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource> {
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

impl<R: SettingsSourceReader> ReaderResolver<'_, R> {
    fn resolve_python_import(
        &mut self,
        import: &SettingsImport,
        importer: &Utf8Path,
    ) -> Option<PythonModule> {
        let import = PythonImport {
            level: import.level,
            module: import.module.as_deref(),
            importer,
        };
        PythonModule::resolve_import(self.db, self.project, import).ok()?
    }

    fn read_resolved_module(&mut self, module: &PythonModule) -> Option<SettingsSource> {
        let file = module.file();
        self.resolved.push(file);
        self.reader.read_source(file)
    }
}
