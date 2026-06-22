//! Synchronize external project state into Salsa inputs.
//!
//! This module is the imperative boundary for project data. It may ask
//! Django, Python, and the filesystem for facts, then writes changed facts to
//! the `Project` input. Pure semantic derivation stays in tracked queries.

use camino::Utf8PathBuf;
use salsa::Setter;

use crate::db::Db as ProjectDb;
use crate::environment::templatetag_candidate_paths;
use crate::project::Project;
use crate::resolve::SearchPaths;
use crate::resolve::model_modules;
use crate::resolve::templatetag_modules;
use crate::settings::settings_source_files;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshData {
    search_paths: SearchPaths,
    file_paths: Vec<Utf8PathBuf>,
}

impl RefreshData {
    #[must_use]
    pub fn from_parts(search_paths: SearchPaths, mut file_paths: Vec<Utf8PathBuf>) -> Self {
        file_paths.sort();
        file_paths.dedup();

        Self {
            search_paths,
            file_paths,
        }
    }

    #[must_use]
    pub fn file_paths(&self) -> &[Utf8PathBuf] {
        &self.file_paths
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshStage {
    ResolveEnvironment,
    ScanSettings,
    DiscoverModelModules,
    DiscoverTemplateLibraries,
    DiscoverTemplateTagCandidates,
}

impl RefreshStage {
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::ResolveEnvironment => "Resolving environment",
            Self::ScanSettings => "Scanning settings",
            Self::DiscoverModelModules => "Discovering model modules",
            Self::DiscoverTemplateLibraries => "Discovering template libraries",
            Self::DiscoverTemplateTagCandidates => "Discovering template tag candidates",
        }
    }
}

pub fn compute_refresh_search_paths(db: &dyn ProjectDb, project: Project) -> SearchPaths {
    SearchPaths::from_project_settings(
        db.file_system(),
        project.root(db),
        project.interpreter(db),
        project.pythonpath(db),
    )
}

pub fn compute_refresh_settings_source_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    settings_source_files(db, project)
        .into_iter()
        .map(|file| file.path(db).to_path_buf())
        .collect()
}

pub fn compute_refresh_model_module_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    model_modules(db, project)
        .iter()
        .map(|module| module.path().to_path_buf())
        .collect()
}

pub fn compute_refresh_template_library_module_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    templatetag_modules(db, project)
        .iter()
        .map(|module| module.path().to_path_buf())
        .collect()
}

pub fn compute_refresh_template_tag_candidate_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    templatetag_candidate_paths(db, project)
}

pub fn apply_refresh(db: &mut dyn ProjectDb, refresh: RefreshData) {
    let Some(project) = db.project() else {
        return;
    };
    let RefreshData {
        search_paths,
        file_paths,
    } = refresh;

    let search_paths_changed = project.search_paths(db) != &search_paths;
    if search_paths_changed {
        search_paths.register_roots(db);
        project.set_search_paths(db).to(search_paths);
    }

    let roots: Vec<_> = project
        .search_paths(db)
        .iter()
        .filter_map(|search_path| db.files().root(db, search_path.path()))
        .collect();

    for root in roots {
        db.bump_file_root_revision(root);
    }

    for path in file_paths {
        let file = db.get_or_create_file(&path);
        let current = file.source(db);
        let latest = db.read_file(&path).unwrap_or_default();

        if current.as_str() != latest {
            db.bump_file_revision(file);
        }
    }
}
