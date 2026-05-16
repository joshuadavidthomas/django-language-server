use crate::project::db::Db;

/// Refresh all external project data.
///
/// This is the imperative boundary between the outside world and Salsa inputs:
/// it asks Django/Python/the filesystem for current facts, writes changed facts
/// into the `Project` input, then lets tracked semantic queries handle editor
/// file contents and downstream derivations.
pub fn refresh_external_data(db: &mut dyn Db) {
    let Some(project) = db.project() else {
        return;
    };

    super::django::refresh_template_dirs(db, project);
    super::symbols::refresh_template_libraries(db, project);
    project.refresh_template_files(db);
    project.refresh_python_index(db);
    super::external::refresh_external_semantic_data(db);
}
