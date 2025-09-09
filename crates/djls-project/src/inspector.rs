pub mod ipc;
pub mod pool;
pub mod queries;
mod tempfile;

use serde::{Deserialize, Serialize};

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
