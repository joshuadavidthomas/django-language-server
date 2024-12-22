use anyhow::Result;
use serde::{Deserialize, Serialize};

pub trait Serializer {
    fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>>;
    fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T>;
}

#[derive(Debug, Clone)]
pub struct JsonSerializer;

impl Serializer for JsonSerializer {
    fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(msg)?)
    }

    fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{HealthCheck, HealthCheckRequestData, Messages, Request};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestStruct {
        field1: String,
        field2: i32,
        field3: Option<String>,
    }

    mod test_struct_serialization {
        use super::*;

        #[test]
        fn test_basic_roundtrip() {
            let original = TestStruct {
                field1: "test".to_string(),
                field2: 42,
                field3: Some("optional".to_string()),
            };
            let encoded = JsonSerializer::encode(&original).unwrap();
            let decoded: TestStruct = JsonSerializer::decode(&encoded).unwrap();
            assert_eq!(original, decoded);
        }

        #[test]
        fn test_unicode_handling() {
            let original = TestStruct {
                field1: "Hello, 世界!".to_string(),
                field2: 42,
                field3: Some("🦀".to_string()),
            };
            let encoded = JsonSerializer::encode(&original).unwrap();
            let decoded: TestStruct = JsonSerializer::decode(&encoded).unwrap();
            assert_eq!(original, decoded);
        }

        #[test]
        fn test_null_handling() {
            let original = TestStruct {
                field1: "test".to_string(),
                field2: 42,
                field3: None,
            };
            let encoded = JsonSerializer::encode(&original).unwrap();
            let decoded: TestStruct = JsonSerializer::decode(&encoded).unwrap();
            assert_eq!(original, decoded);
        }
    }

    mod request_serialization {
        use super::*;

        #[test]
        fn test_health_check_request() {
            let original: Request<HealthCheck> = Request {
                message: Messages::HealthCheck,
                data: HealthCheckRequestData {},
            };
            let encoded = JsonSerializer::encode(&original).unwrap();
            let decoded: Request<HealthCheck> = JsonSerializer::decode(&encoded).unwrap();
            assert_eq!(original, decoded);
        }
    }

    mod error_cases {
        use super::*;

        #[test]
        fn test_invalid_json() {
            let invalid_data = b"{invalid json}";
            let result: Result<TestStruct> = JsonSerializer::decode(invalid_data);
            assert!(result.is_err());
        }

        #[test]
        fn test_empty_input() {
            let empty_data = b"";
            let result: Result<TestStruct> = JsonSerializer::decode(empty_data);
            assert!(result.is_err());
        }

        #[test]
        fn test_invalid_request_json() {
            let invalid_data = b"{invalid json}";
            let result: Result<Request<HealthCheck>> = JsonSerializer::decode(invalid_data);
            assert!(result.is_err());
        }
    }

    mod performance_cases {
        use super::*;

        #[test]
        fn test_large_payload() {
            let large_string = "a".repeat(1000000);
            let original = TestStruct {
                field1: large_string,
                field2: 42,
                field3: None,
            };
            let encoded = JsonSerializer::encode(&original).unwrap();
            let decoded: TestStruct = JsonSerializer::decode(&encoded).unwrap();
            assert_eq!(original, decoded);
        }
    }
}
