use crate::messages::{Message, Request, Response};
use crate::process::{ManagedProcess, ProcessError, PythonTarget};
use crate::transport::{ProcessTransport, Transport, TransportError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("Process error: {0}")]
    Process(#[from] ProcessError),
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
}

#[derive(Debug)]
pub struct IpcClient {
    process: ManagedProcess,
    transport: ProcessTransport,
}

impl IpcClient {
    pub async fn new(target: impl Into<PythonTarget>) -> Result<Self, ClientError> {
        let (process, transport) = ManagedProcess::new(target).await?;

        Ok(Self { process, transport })
    }

    pub async fn send<M>(&mut self, data: M::RequestData) -> Result<M, ClientError>
    where
        M: Message + Send,
        M::RequestData: Send + Sync,
    {
        let request: Request<M> = Request::new(data);
        self.transport.send(&request).await?;
        let response: Response<M> = self.transport.receive().await?;
        Ok(response
            .try_into_message()
            .map_err(|e| TransportError::Decode(e.message))?)
    }

    pub async fn shutdown(mut self) -> Result<(), ClientError> {
        self.process.shutdown().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::Messages;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use std::fs;
    use std::path::PathBuf;

    #[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
    struct TestMessage {
        value: String,
    }

    #[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
    struct TestRequestData {
        input: String,
    }

    impl Message for TestMessage {
        type RequestData = TestRequestData;
        const TYPE: Messages = Messages::Test;
    }

    fn create_test_script() -> PathBuf {
        let temp_dir = tempfile::tempdir().unwrap();
        let script_path = temp_dir.path().join("test_script.py");

        fs::write(
            &script_path,
            r#"
import json
import sys
import struct

def read_message():
    length_bytes = sys.stdin.buffer.read(4)
    length = struct.unpack('>I', length_bytes)[0]
    message_bytes = sys.stdin.buffer.read(length)
    return json.loads(message_bytes)

def write_message(msg):
    data = json.dumps(msg).encode('utf-8')
    sys.stdout.buffer.write(struct.pack('>I', len(data)))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

while True:
    msg = read_message()
    response = {
        'message': msg['message'],
        'success': True,
        'data': {'value': msg['data']['input']},
        'error': None
    }
    write_message(response)
"#,
        )
        .unwrap();

        std::mem::forget(temp_dir);
        script_path
    }

    #[tokio::test]
    async fn test_script_basic_operation() -> Result<(), ClientError> {
        let script_path = create_test_script();
        let mut client = IpcClient::new(script_path).await?;

        let result = client
            .send::<TestMessage>(TestRequestData {
                input: "test script".to_string(),
            })
            .await?;

        assert_eq!(result.value, "test script");
        client.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_target_types() -> Result<(), ClientError> {
        let script_path = create_test_script();

        // Test PathBuf
        let mut client = IpcClient::new(script_path.clone()).await?;
        let result = client
            .send::<TestMessage>(TestRequestData {
                input: "pathbuf".to_string(),
            })
            .await?;
        assert_eq!(result.value, "pathbuf");
        client.shutdown().await?;

        // Test &Path
        let mut client = IpcClient::new(script_path.clone()).await?;
        let result = client
            .send::<TestMessage>(TestRequestData {
                input: "path".to_string(),
            })
            .await?;
        assert_eq!(result.value, "path");
        client.shutdown().await?;

        // Test String path
        let mut client = IpcClient::new(script_path.to_string_lossy().to_string()).await?;
        let result = client
            .send::<TestMessage>(TestRequestData {
                input: "string".to_string(),
            })
            .await?;
        assert_eq!(result.value, "string");
        client.shutdown().await?;

        Ok(())
    }
}
