use crate::messages::{Message, Messages};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An untyped Request used only for schema generation.
/// This type should not be constructed or used directly.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "Request")]
pub(crate) struct GenericRequest {
    message: Messages,
    #[schemars(schema_with = "value_schema")]
    data: serde_json::Value,
}

/// A typed Request for use in Rust code
#[derive(Debug, Serialize, Deserialize)]
pub struct Request<M: Message> {
    pub message: Messages,
    pub data: M::RequestData,
}

impl<M: Message> Request<M> {
    pub fn new(data: M::RequestData) -> Self {
        Self {
            message: M::TYPE,
            data,
        }
    }
}

/// An untyped Response used only for schema generation.
/// This type should not be constructed or used directly.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "Response")]
pub(crate) struct GenericResponse {
    message: Messages,
    success: bool,
    #[schemars(schema_with = "value_schema")]
    data: Option<serde_json::Value>,
    error: Option<ErrorResponse>,
}

/// A typed Response for use in Rust code
#[derive(Debug, Serialize, Deserialize)]
pub struct Response<M> {
    pub message: Messages,
    pub success: bool,
    pub data: Option<M>,
    pub error: Option<ErrorResponse>,
}

impl<M: Message> Response<M> {
    pub fn try_into_message(self) -> Result<M, ErrorResponse> {
        match (self.success, self.data) {
            (true, Some(data)) => Ok(data),
            _ => Err(self.error.unwrap_or_else(|| ErrorResponse {
                code: "unknown".to_string(),
                message: "Unknown error".to_string(),
            })),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
}

fn value_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    schemars::schema::Schema::Object(schemars::schema::SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::Object.into()),
        ..Default::default()
    })
}
