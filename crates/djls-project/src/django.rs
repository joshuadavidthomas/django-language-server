use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

use crate::db::Db as ProjectDb;
use crate::inspector;
use crate::inspector::InspectorRequest;
use crate::Project;

#[derive(Serialize)]
struct DjangoInitRequest;

#[derive(Deserialize)]
struct DjangoInitResponse;

impl InspectorRequest for DjangoInitRequest {
    const NAME: &'static str = "django_init";
    type Response = DjangoInitResponse;
}

/// Check if Django is available for the current project.
///
/// This tracked function attempts to initialize Django via the inspector.
/// Returns true if Django was successfully initialized, false otherwise.
#[salsa::tracked]
pub fn django_available(db: &dyn ProjectDb, _project: Project) -> bool {
    inspector::query(db, &DjangoInitRequest).is_some()
}

#[derive(Serialize)]
struct TemplateDirsRequest;

#[derive(Deserialize)]
struct TemplateDirsResponse {
    dirs: Vec<Utf8PathBuf>,
}

impl InspectorRequest for TemplateDirsRequest {
    const NAME: &'static str = "template_dirs";
    type Response = TemplateDirsResponse;
}

#[salsa::tracked]
pub fn template_dirs(db: &dyn ProjectDb, _project: Project) -> Option<Vec<Utf8PathBuf>> {
    tracing::debug!("Requesting template directories from inspector");

    let response = inspector::query(db, &TemplateDirsRequest)?;

    let dir_count = response.dirs.len();
    tracing::info!(
        "Retrieved {} template directories from inspector",
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
