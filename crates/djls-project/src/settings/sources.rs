use std::collections::BTreeSet;

use camino::Utf8Path;
use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::resolve::ImportParts;
use crate::settings::DjangoSettings;
use crate::settings::SettingsSource;
use crate::settings::SettingsSourceResolver;
use crate::settings::SettingsStarImport;
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
    fn empty() -> Self {
        Self(Vec::new())
    }

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
        return DjangoSettingsSources::empty();
    };

    // The refresh bump set must cover the same settings source graph the
    // extractor would read against current disk content.
    let mut reader = RefreshSettingsSourceReader { db };
    let Some(source) = reader.read_source(file) else {
        return DjangoSettingsSources::from_files(db, [file]);
    };
    let mut resolver = ReaderResolver::new(db, project, reader);
    let _ = extract_settings(source.source.as_str(), &source.path, &mut resolver);

    DjangoSettingsSources::from_files(db, std::iter::once(file).chain(resolver.into_resolved()))
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

struct RefreshSettingsSourceReader<'db> {
    db: &'db dyn ProjectDb,
}

impl SettingsSourceReader for RefreshSettingsSourceReader<'_> {
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

    fn into_resolved(self) -> Vec<File> {
        self.resolved
    }
}

impl<R: SettingsSourceReader> SettingsSourceResolver for ReaderResolver<'_, R> {
    fn resolve_star_import(
        &mut self,
        import: &SettingsStarImport,
        importer: &Utf8Path,
    ) -> Option<SettingsSource> {
        let parts = ImportParts {
            level: import.level,
            module: import.module.as_deref(),
            importer,
        };
        let module_name = crate::resolve::resolve_import(self.db, self.project, parts)?;
        let file = crate::resolve::module_file(self.db, self.project, &module_name)?;
        self.resolved.push(file);
        self.reader.read_source(file)
    }
}
