mod macros;
mod types;

pub use types::*;

use crate::define_messages;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("Duplicate module name found: {0}")]
    DuplicateModuleName(String),
}

pub trait Message: Serialize + DeserializeOwned + JsonSchema {
    type RequestData: Serialize + DeserializeOwned + JsonSchema;
    const TYPE: Messages;
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub enum Messages {
    #[serde(rename = "HEALTH_CHECK")]
    HealthCheck,
    #[serde(rename = "UNKNOWN")]
    Unknown,
    #[serde(rename = "TEST")]
    Test,
}

define_messages! {
    MessageType::HealthCheck {
        request: HealthCheckRequestData,
        response: HealthCheck
    },
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct HealthCheckRequestData {}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct HealthCheck {
    pub status: String,
    pub version: String,
}
