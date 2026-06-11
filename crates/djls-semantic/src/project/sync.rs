//! Synchronize external project state into Salsa inputs.
//!
//! This module is the imperative boundary for project data. It may ask
//! Django, Python, and the filesystem for facts, then writes changed facts to
//! the `Project` input. Pure semantic derivation stays in tracked queries.

use crate::project::db::Db as ProjectDb;
use crate::project::input::Project;
use crate::project::resolve::model_modules;
use crate::project::resolve::templatetag_modules;
use crate::project::settings::settings_dependency_files;

/// Refresh all external project data.
///
/// This is the imperative boundary between the outside world and Salsa inputs:
/// it asks Django/Python/the filesystem for current facts, writes changed facts
/// into the `Project` input, then lets tracked semantic queries handle editor
/// file contents and downstream derivations.
pub fn refresh_external_data(db: &mut dyn ProjectDb) {
    let Some(project) = db.project() else {
        return;
    };

    project.refresh_source_roots(db);
    refresh_python_modules(db, project);
}

fn refresh_python_modules(db: &mut dyn ProjectDb, project: Project) {
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

    for file in settings_dependency_files(db, project) {
        db.bump_file_revision(file);
    }

    let mut file_paths = Vec::new();
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

    file_paths.sort();
    file_paths.dedup();

    for path in file_paths {
        let file = db.get_or_create_file(&path);
        db.bump_file_revision(file);
    }
}
