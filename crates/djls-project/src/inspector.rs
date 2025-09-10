pub mod ipc;
pub mod pool;
pub mod queries;
mod zipapp;

use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use crate::db::Db as ProjectDb;
use crate::meta::Project;
use crate::python::python_environment;
use crate::python::resolve_interpreter;
use queries::InspectorQueryKind;
pub use queries::Query;

#[derive(Serialize)]
pub struct DjlsRequest {
    #[serde(flatten)]
    pub query: Query,
}

#[derive(Debug, Deserialize)]
pub struct DjlsResponse {
    pub ok: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Run an inspector query and return the JSON result as a string.
///
/// This tracked function executes inspector queries through a temporary pool
/// and caches the results based on project state and query kind.
#[allow(clippy::drop_non_drop)]
#[salsa::tracked]
pub fn inspector_run(
    db: &dyn ProjectDb,
    project: Project,
    kind: InspectorQueryKind,
) -> Option<String> {
    // Create dependency on project revision
    let _ = project.revision(db);

    // Get interpreter path - required for inspector
    let _interpreter_path = resolve_interpreter(db, project)?;
    let project_path = Path::new(project.root(db));

    // Get Python environment for inspector
    let python_env = python_environment(db, project)?;

    // Create the appropriate query based on kind
    let query = match kind {
        InspectorQueryKind::TemplateTags => crate::inspector::Query::Templatetags,
        InspectorQueryKind::DjangoAvailable | InspectorQueryKind::SettingsModule => {
            crate::inspector::Query::DjangoInit
        }
    };

    let request = crate::inspector::DjlsRequest { query };

    // Create a temporary inspector pool for this query
    // Note: In production, this could be optimized with a shared pool
    let pool = crate::inspector::pool::InspectorPool::new();

    match pool.query(&python_env, project_path, &request) {
        Ok(response) => {
            if response.ok {
                if let Some(data) = response.data {
                    // Convert to JSON string
                    serde_json::to_string(&data).ok()
                } else {
                    None
                }
            } else {
                None
            }
        }
        Err(_) => None,
    }
}
