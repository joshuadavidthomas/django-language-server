use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::Duration;
use tempfile::{tempdir, TempDir};

pub struct Server {
    #[cfg(unix)]
    socket_path: PathBuf,
    process: Child,
    _temp_dir: TempDir,
}

impl Server {
    pub fn start(python_module: &str, args: &[&str]) -> Result<Self> {
        Self::start_with_options(python_module, args, true)
    }

    pub fn start_script(python_script: &str, args: &[&str]) -> Result<Self> {
        Self::start_with_options(python_script, args, false)
    }

    fn start_with_options(python_path: &str, args: &[&str], use_module: bool) -> Result<Self> {
        let temp_dir = tempdir()?;

        let path = {
            let socket_path = temp_dir.path().join("ipc.sock");
            socket_path
        };

        let mut command = Command::new("python");
        if use_module {
            command.arg("-m");
        }
        command.arg(python_path);
        command.args(args);
        command.arg("--ipc-path").arg(&path);

        let process = command.spawn().context("Failed to start Python process")?;

        sleep(Duration::from_millis(100));

        Ok(Self {
            socket_path: path,
            process,
            _temp_dir: temp_dir,
        })
    }

    pub fn get_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.process.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;
    use serde::{Deserialize, Serialize};

    const FIXTURES_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    async fn setup_server_and_client() -> Result<(Server, crate::client::Client)> {
        let path = format!("{}/echo_server.py", FIXTURES_PATH);
        let server = Server::start_script(&path, &[])?;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let client = Client::connect(server.get_path()).await?;
        Ok((server, client))
    }

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct ComplexMessage {
        field1: String,
        field2: i32,
        field3: bool,
    }

    #[tokio::test]
    async fn test_basic_string_message() -> Result<()> {
        let (_server, mut client) = setup_server_and_client().await?;

        let response: String = client.send("test".to_string()).await?;
        assert_eq!(response, "test");

        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_messages() -> Result<()> {
        let (_server, mut client) = setup_server_and_client().await?;

        for i in 1..=3 {
            let msg = format!("test{}", i);
            let response: String = client.send(msg.clone()).await?;
            assert_eq!(response, msg);
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_complex_message() -> Result<()> {
        let (_server, mut client) = setup_server_and_client().await?;

        let complex = ComplexMessage {
            field1: "hello".to_string(),
            field2: 42,
            field3: true,
        };

        let response: ComplexMessage = client.send(complex.clone()).await?;
        assert_eq!(response, complex);

        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_clients() -> Result<()> {
        let (server, mut client1) = setup_server_and_client().await?;
        let mut client2 = crate::client::Client::connect(server.get_path()).await?;

        let response1: String = client1.send("test1".to_string()).await?;
        assert_eq!(response1, "test1");

        let response2: String = client2.send("test2".to_string()).await?;
        assert_eq!(response2, "test2");

        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_messages() -> Result<()> {
        let (_server, mut client) = setup_server_and_client().await?;

        let mut handles = Vec::new();

        for i in 1..=5 {
            let msg = format!("test{}", i);
            handles.push(tokio::spawn(async move { msg }));
        }

        let mut results = Vec::new();
        for handle in handles {
            let msg = handle.await?;
            let response: String = client.send(msg.clone()).await?;
            results.push((msg, response));
        }

        for (request, response) in results {
            assert_eq!(request, response);
        }

        Ok(())
    }
}
