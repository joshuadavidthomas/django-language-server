use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Deserialization error: {0}")]
    Deserialization(serde_json::Error),
}

pub trait Protocol {
    fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, ProtocolError>;
    fn deserialize<R: for<'de> Deserialize<'de>>(&self, bytes: &[u8]) -> Result<R, ProtocolError>;
}

#[derive(Debug, Clone)]
pub struct JsonProtocol;

impl Protocol for JsonProtocol {
    fn serialize<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, ProtocolError> {
        serde_json::to_vec(value).map_err(ProtocolError::from)
    }

    fn deserialize<R: for<'de> Deserialize<'de>>(&self, bytes: &[u8]) -> Result<R, ProtocolError> {
        serde_json::from_slice(bytes).map_err(ProtocolError::from)
    }
}
