pub mod ipc;
pub mod pool;
pub mod queries;
mod zipapp;

use serde::Deserialize;
use serde::Serialize;

use crate::db::Db as ProjectDb;
use crate::python::python_environment;
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
/// This tracked function executes inspector queries through the shared pool
/// and caches the results based on project state and query kind.
pub fn inspector_run(db: &dyn ProjectDb, kind: InspectorQueryKind) -> Option<String> {
    let python_env = python_environment(db)?;
    let project_path = db.project_path()?;

    let query = match kind {
        InspectorQueryKind::TemplateTags => crate::inspector::Query::Templatetags,
        InspectorQueryKind::DjangoAvailable | InspectorQueryKind::SettingsModule => {
            crate::inspector::Query::DjangoInit
        }
    };
    let request = crate::inspector::DjlsRequest { query };

    match db
        .inspector_pool()
        .query(&python_env, project_path, &request)
    {
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
