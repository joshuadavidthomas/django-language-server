pub mod ipc;
pub mod pool;
pub mod queries;

pub use queries::Query;
use serde::Deserialize;
use serde::Serialize;

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
