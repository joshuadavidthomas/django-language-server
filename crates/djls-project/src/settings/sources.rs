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
    let source = file.source(db);
    let path = file.path(db);
    let mut resolver = SalsaSettingsResolver { db, project };
    extract_settings(source.as_str(), path, &mut resolver)
}

pub(crate) fn settings_source_files(db: &dyn ProjectDb, project: Project) -> Vec<File> {
    let Some(file) = crate::settings::settings_module_file(db, project) else {
        return Vec::new();
    };

    let path = file.path(db).to_path_buf();
    let Ok(source) = db.read_file(&path) else {
        return vec![file];
    };

    // The refresh bump set must cover the same settings source graph the
    // extractor would read against current disk content.
    let mut resolver = DiskSettingsResolver {
        db,
        project,
        touched: Vec::new(),
    };
    let _ = extract_settings(&source, &path, &mut resolver);

    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    if seen.insert(path) {
        files.push(file);
    }
    for file in resolver.touched {
        let path = file.path(db).to_path_buf();
        if seen.insert(path) {
            files.push(file);
        }
    }
    files
}

struct SalsaSettingsResolver<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
}

impl SettingsSourceResolver for SalsaSettingsResolver<'_> {
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
        let module_path = crate::resolve::resolve_import(self.db, self.project, parts)?;
        let file = crate::resolve::module_file(self.db, self.project, &module_path)?;
        Some(SettingsSource {
            source: file.source(self.db).as_str().to_string(),
            path: file.path(self.db).to_path_buf(),
        })
    }
}

struct DiskSettingsResolver<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    touched: Vec<File>,
}

impl SettingsSourceResolver for DiskSettingsResolver<'_> {
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
        let module_path = crate::resolve::resolve_import(self.db, self.project, parts)?;
        let file = crate::resolve::module_file(self.db, self.project, &module_path)?;
        let path = file.path(self.db).to_path_buf();
        self.touched.push(file);
        let source = self.db.read_file(&path).ok()?;

        Some(SettingsSource { source, path })
    }
}

pub(crate) fn installed_app_package_module(installed_app: &str) -> &str {
    if let Some((module, _)) = installed_app.split_once(".apps.") {
        module
    } else if installed_app.ends_with("Config") {
        installed_app
            .rsplit_once('.')
            .map_or(installed_app, |(module, _)| module)
    } else {
        installed_app
    }
}
