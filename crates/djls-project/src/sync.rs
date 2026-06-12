//! Synchronize external project state into Salsa inputs.
//!
//! This module is the imperative boundary for project data. It may ask
//! Django, Python, and the filesystem for facts, then writes changed facts to
//! the `Project` input. Pure semantic derivation stays in tracked queries.

use camino::Utf8PathBuf;
use salsa::Setter;

use crate::db::Db as ProjectDb;
use crate::environment::templatetag_candidate_paths;
use crate::resolve::SearchPaths;
use crate::resolve::model_modules;
use crate::resolve::templatetag_modules;
use crate::settings::settings_source_files;

/// Refresh all external project data.
///
/// This is the imperative boundary between the outside world and Salsa inputs:
/// it asks Django/Python/the filesystem for current facts, writes changed facts
/// into the `Project` input, then lets tracked semantic queries handle editor
/// file contents and downstream derivations.
pub fn refresh_external_data(db: &mut dyn ProjectDb) {
    let Some(refresh) = compute_refresh(db) else {
        return;
    };
    apply_refresh(db, refresh);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshData {
    search_paths: SearchPaths,
    file_paths: Vec<Utf8PathBuf>,
}

pub fn compute_refresh(db: &dyn ProjectDb) -> Option<RefreshData> {
    let project = db.project()?;
    let search_paths = SearchPaths::from_project_settings(
        db.file_system(),
        project.root(db),
        project.interpreter(db),
        project.pythonpath(db),
    );

    let mut file_paths: Vec<_> = settings_source_files(db, project)
        .into_iter()
        .map(|file| file.path(db).to_path_buf())
        .collect();

    file_paths.extend(
        model_modules(db, project)
            .iter()
            .map(|module| module.path().to_path_buf()),
    );

    file_paths.extend(
        templatetag_modules(db, project)
            .iter()
            .map(|module| module.path().to_path_buf()),
    );
    file_paths.extend(templatetag_candidate_paths(db, project));

    file_paths.sort();
    file_paths.dedup();

    Some(RefreshData {
        search_paths,
        file_paths,
    })
}

pub fn apply_refresh(db: &mut dyn ProjectDb, refresh: RefreshData) {
    let Some(project) = db.project() else {
        return;
    };
    let RefreshData {
        search_paths,
        file_paths,
    } = refresh;

    search_paths.register_roots(db);
    if project.search_paths(db) != &search_paths {
        project.set_search_paths(db).to(search_paths);
    }

    // The LSP currently has no watched-file stream for dependency roots. Treat
    // an explicit refresh as the freshness boundary for module discovery and
    // currently discovered Python files.
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
        db.bump_file_revision(file);
    }
}
