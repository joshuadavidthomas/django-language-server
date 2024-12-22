use crate::{
    serializers::Serializer,
    transport::{LengthPrefixedTransport, TransportError},
};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::process::{Child, Command};

type ProcessTransport =
    LengthPrefixedTransport<tokio::process::ChildStdout, tokio::process::ChildStdin>;

#[derive(Error, Debug)]
pub enum ProcessError {
    #[error("Failed to spawn process: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("Process exited unexpectedly")]
    Terminated,
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
}

#[derive(Debug)]
pub enum PythonTarget {
    Script(PathBuf), // For direct .py file execution
    Module(String),  // For Python module imports
}

impl From<PathBuf> for PythonTarget {
    fn from(path: PathBuf) -> Self {
        PythonTarget::Script(path)
    }
}

impl From<&Path> for PythonTarget {
    fn from(path: &Path) -> Self {
        PythonTarget::Script(path.to_path_buf())
    }
}

impl From<String> for PythonTarget {
    fn from(s: String) -> Self {
        if s.ends_with(".py") {
            PythonTarget::Script(PathBuf::from(s))
        } else {
            PythonTarget::Module(s)
        }
    }
}

impl<'a> From<&'a str> for PythonTarget {
    fn from(s: &'a str) -> Self {
        if s.ends_with(".py") {
            PythonTarget::Script(PathBuf::from(s))
        } else {
            PythonTarget::Module(s.to_string())
        }
    }
}

#[derive(Debug)]
pub struct ManagedProcess {
    child: Child,
    target: PythonTarget,
}

impl ManagedProcess {
    pub async fn new<S>(
        target: impl Into<PythonTarget>,
    ) -> Result<(Self, ProcessTransport<S>), ProcessError>
    where
        S: Serializer + Send + Sync + 'static,
    {
        let target = target.into();

        let mut command = Command::new("python");
        match &target {
            PythonTarget::Script(path) => {
                command.arg(path);
            }
            PythonTarget::Module(name) => {
                command.arg("-m").arg(name);
            }
        }

        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ProcessError::Spawn(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to get stdin",
            ))
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            ProcessError::Spawn(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to get stdout",
            ))
        })?;

        let transport = LengthPrefixedTransport::<_, _, S>::new(stdout, stdin);

        Ok((Self { child, target }, transport))
    }

    pub async fn shutdown(&mut self) -> Result<(), ProcessError> {
        self.child.kill().await?;
        self.child.wait().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Transport;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_echo_script() -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            r#"
import json
import sys
import struct

while True:
    # Read length prefix (4 bytes)
    length_bytes = sys.stdin.buffer.read(4)
    if not length_bytes:
        break

    length = struct.unpack('>I', length_bytes)[0]

    # Read message
    message_bytes = sys.stdin.buffer.read(length)
    if not message_bytes:
        break

    # Echo back
    sys.stdout.buffer.write(struct.pack('>I', len(message_bytes)))
    sys.stdout.buffer.write(message_bytes)
    sys.stdout.buffer.flush()
"#
        )
        .unwrap();
        file.flush().unwrap();
        file
    }

    #[tokio::test]
    async fn test_process_basic_lifecycle() {
        let script = create_echo_script();
        let (mut process, _transport) = ManagedProcess::new(script.path()).await.unwrap();
        process.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_process_communication() {
        let script = create_echo_script();
        let (mut process, mut transport) = ManagedProcess::new(script.path()).await.unwrap();

        // Test basic message round trip
        let test_data = serde_json::json!({
            "test": "message",
            "number": 42
        });

        transport.send(&test_data).await.unwrap();
        let response: serde_json::Value = transport.receive().await.unwrap();

        assert_eq!(test_data, response);
        process.shutdown().await.unwrap();
    }
}
