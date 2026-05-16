use camino::Utf8PathBuf;
use salsa::Setter;
use serde::Deserialize;
use serde::Serialize;

use crate::project::db::Db as ProjectDb;
use crate::project::introspector::IntrospectionRequest;
use crate::project::Project;
use crate::project::TemplateDirs;

#[derive(Serialize)]
struct TemplateDirsRequest;

#[derive(Deserialize)]
struct TemplateDirsResponse {
    dirs: Vec<Utf8PathBuf>,
}

impl IntrospectionRequest for TemplateDirsRequest {
    const NAME: &'static str = "template_dirs";
    type Response = TemplateDirsResponse;
}

/// Refresh template directories from project introspection.
pub(super) fn refresh_template_dirs(db: &mut dyn ProjectDb, project: Project) {
    let Some(dirs) = fetch_template_dirs(db) else {
        return;
    };

    let next = TemplateDirs::Known(dirs);
    if project.template_dirs(db) != &next {
        project.set_template_dirs(db).to(next);
    }
}

fn fetch_template_dirs(db: &dyn ProjectDb) -> Option<Vec<Utf8PathBuf>> {
    tracing::debug!("Requesting template directories from project introspection");

    let response = db.project_introspector().query(db, &TemplateDirsRequest)?;

    let dir_count = response.dirs.len();
    tracing::info!(
        "Retrieved {} template directories from project introspection",
        dir_count
    );

    for (i, dir) in response.dirs.iter().enumerate() {
        tracing::debug!("  Template dir [{}]: {}", i, dir);
    }

    let missing_dirs: Vec<_> = response.dirs.iter().filter(|dir| !dir.exists()).collect();

    if !missing_dirs.is_empty() {
        tracing::warn!(
            "Found {} non-existent template directories: {:?}",
            missing_dirs.len(),
            missing_dirs
        );
    }

    Some(response.dirs)
}
