use std::collections::BTreeSet;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::resolve::module_file_in_search_path;
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
        let module_path = resolve_star_import_module(self.db, self.project, importer, import)?;
        let file = module_file(self.db, self.project, &module_path)?;
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
        let module_path = resolve_star_import_module(self.db, self.project, importer, import)?;
        let file = module_file(self.db, self.project, &module_path)?;
        let path = file.path(self.db).to_path_buf();
        self.touched.push(file);
        let source = self.db.read_file(&path).ok()?;

        Some(SettingsSource { source, path })
    }
}

pub(crate) fn module_file(db: &dyn ProjectDb, project: Project, module_path: &str) -> Option<File> {
    project.touch_search_path_roots(db);

    for search_path in project.search_paths(db).iter() {
        let Some(path) =
            module_file_in_search_path(db.file_system(), module_path, search_path.path())
        else {
            continue;
        };
        return Some(db.get_or_create_file(&path));
    }

    None
}

pub(crate) fn package_dir(
    db: &dyn ProjectDb,
    project: Project,
    package_module: &str,
) -> Option<Utf8PathBuf> {
    if package_module.is_empty() {
        return None;
    }

    let relative = package_module.replace('.', "/");
    for search_path in project.search_paths(db).iter() {
        let candidate = search_path.path().join(&relative);
        if db.path_is_dir(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn resolve_star_import_module(
    db: &dyn ProjectDb,
    project: Project,
    base: &Utf8Path,
    import: &SettingsStarImport,
) -> Option<String> {
    if import.level == 0 {
        return import.module.clone();
    }

    let module_file = ModuleFileParts::from_path(db, project, base)?;
    let mut parts = module_file.parts;
    if !module_file.is_package_init {
        parts.pop()?;
    }

    for _ in 1..import.level {
        parts.pop()?;
    }

    if let Some(module) = &import.module {
        parts.extend(
            module
                .split('.')
                .filter(|part| !part.is_empty())
                .map(str::to_string),
        );
    }

    (!parts.is_empty()).then(|| parts.join("."))
}

struct ModuleFileParts {
    parts: Vec<String>,
    is_package_init: bool,
}

impl ModuleFileParts {
    fn from_path(db: &dyn ProjectDb, project: Project, file_path: &Utf8Path) -> Option<Self> {
        let root = project
            .search_paths(db)
            .iter()
            .filter(|search_path| file_path.starts_with(search_path.path()))
            .max_by_key(|search_path| search_path.path().as_str().len())?
            .path();
        let relative = file_path.strip_prefix(root).ok()?;
        let mut parts: Vec<String> = relative
            .components()
            .map(|component| component.as_str().to_string())
            .collect();
        let last = parts.pop()?;

        if last == "__init__.py" {
            return Some(Self {
                parts,
                is_package_init: true,
            });
        }

        let last_path = Utf8Path::new(&last);
        if last_path.extension() != Some("py") {
            return None;
        }
        parts.push(last_path.file_stem()?.to_string());

        Some(Self {
            parts,
            is_package_init: false,
        })
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
