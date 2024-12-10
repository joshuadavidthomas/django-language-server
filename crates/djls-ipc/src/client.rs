use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[async_trait]
pub trait ConnectionTrait: Send {
    async fn write_all(&mut self, buf: &[u8]) -> Result<()>;
    async fn read_line(&mut self, buf: &mut String) -> Result<usize>;
}

pub struct Connection {
    #[cfg(unix)]
    inner: UnixConnection,
    #[cfg(windows)]
    inner: WindowsConnection,
}

#[cfg(unix)]
pub struct UnixConnection {
    stream: tokio::net::UnixStream,
}

#[cfg(windows)]
pub struct WindowsConnection {
    pipe: tokio::net::windows::named_pipe::NamedPipeClient,
}

impl Connection {
    pub async fn connect(path: &Path) -> Result<Box<dyn ConnectionTrait>> {
        #[cfg(unix)]
        {
            let stream = tokio::net::UnixStream::connect(path).await?;
            Ok(Box::new(UnixConnection { stream }))
        }

        #[cfg(windows)]
        {
            let pipe_path = format!(r"\\.\pipe\{}", path.file_name().unwrap().to_string_lossy());
            let pipe = tokio::net::windows::named_pipe::ClientOptions::new().open(&pipe_path)?;
            Ok(Box::new(WindowsConnection { pipe }))
        }
    }
}

#[cfg(unix)]
#[async_trait]
impl ConnectionTrait for UnixConnection {
    async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.stream.write_all(buf).await?;
        Ok(())
    }

    async fn read_line(&mut self, buf: &mut String) -> Result<usize> {
        let mut reader = BufReader::new(&mut self.stream);
        let bytes_read = reader.read_line(buf).await?;
        Ok(bytes_read)
    }
}

#[cfg(windows)]
#[async_trait]
impl ConnectionTrait for WindowsConnection {
    async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.pipe.write_all(buf).await?;
        Ok(())
    }

    async fn read_line(&mut self, buf: &mut String) -> Result<usize> {
        let mut reader = BufReader::new(&mut self.pipe);
        let bytes_read = reader.read_line(buf).await?;
        Ok(bytes_read)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message<T> {
    pub id: u64,
    pub content: T,
}

pub struct Client {
    connection: Box<dyn ConnectionTrait>,
    message_id: u64,
}

impl Client {
    pub async fn connect(path: &Path) -> Result<Self> {
        let connection = Connection::connect(path).await?;
        Ok(Self {
            connection,
            message_id: 0,
        })
    }

    pub async fn send<T, R>(&mut self, content: T) -> Result<R>
    where
        T: Serialize,
        R: for<'de> Deserialize<'de>,
    {
        self.message_id += 1;
        let message = Message {
            id: self.message_id,
            content,
        };

        let msg = serde_json::to_string(&message)? + "\n";
        self.connection.write_all(msg.as_bytes()).await?;

        let mut buffer = String::new();
        self.connection.read_line(&mut buffer).await?;

        let response: Message<R> = serde_json::from_str(&buffer)?;

        if response.id != self.message_id {
            return Err(anyhow::anyhow!(
                "Message ID mismatch. Expected {}, got {}",
                self.message_id,
                response.id
            ));
        }

        Ok(response.content)
    }
}

#[cfg(unix)]
#[cfg(test)]
mod conn_unix_tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::net::UnixListener;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn test_unix_connection() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let socket_path = temp_file.path().to_owned();
        temp_file.close()?;

        // Channel to signal when server is ready
        let (tx, rx) = oneshot::channel();

        // Start server
        let listener = UnixListener::bind(&socket_path)?;

        tokio::spawn(async move {
            // Signal that we're listening
            tx.send(()).unwrap();

            let (stream, _) = listener.accept().await.unwrap();

            loop {
                let mut buf = [0; 1024];
                match stream.try_read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        stream.try_write(&buf[..n]).unwrap();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(e) => panic!("Error reading from socket: {}", e),
                }
            }
        });

        // Wait for server to be ready
        rx.await?;

        // Connect client
        let mut connection = Connection::connect(&socket_path).await?;

        // Test single message
        connection.write_all(b"hello\n").await?;
        let mut response = String::new();
        let n = connection.read_line(&mut response).await?;
        assert_eq!(n, 6);
        assert_eq!(response, "hello\n");

        // Test multiple messages
        for i in 0..3 {
            let msg = format!("message{}\n", i);
            connection.write_all(msg.as_bytes()).await?;
            let mut response = String::new();
            let n = connection.read_line(&mut response).await?;
            assert_eq!(n, msg.len());
            assert_eq!(response, msg);
        }

        // Test large message
        let large_msg = "a".repeat(1000) + "\n";
        connection.write_all(large_msg.as_bytes()).await?;
        let mut response = String::new();
        let n = connection.read_line(&mut response).await?;
        assert_eq!(n, large_msg.len());
        assert_eq!(response, large_msg);

        Ok(())
    }

    #[tokio::test]
    async fn test_unix_connection_nonexistent_path() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let socket_path = temp_file.path().to_owned();
        temp_file.close()?;

        let result = Connection::connect(&socket_path).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_unix_connection_server_disconnect() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let socket_path = temp_file.path().to_owned();
        temp_file.close()?;

        let (tx, rx) = oneshot::channel();
        let listener = UnixListener::bind(&socket_path)?;

        let server_handle = tokio::spawn(async move {
            tx.send(()).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            // Server immediately drops the connection
            drop(stream);
        });

        rx.await?;
        let mut connection = Connection::connect(&socket_path).await?;

        // Write should fail after server disconnects
        connection.write_all(b"hello\n").await?;
        let mut response = String::new();
        let result = connection.read_line(&mut response).await;
        assert!(result.is_err() || result.unwrap() == 0);

        server_handle.await?;
        Ok(())
    }
}

#[cfg(windows)]
#[cfg(test)]
mod conn_windows_tests {
    use super::*;
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
    use tokio::sync::oneshot;
    use uuid::Uuid;

    async fn create_server_pipe() -> Result<(String, oneshot::Receiver<()>)> {
        let pipe_name = format!(r"\\.\pipe\test_{}", Uuid::new_v4());
        let (tx, rx) = oneshot::channel();

        let server = ServerOptions::new().create(&pipe_name)?;

        tokio::spawn(async move {
            server.connect().await.unwrap();
            tx.send(()).unwrap();

            loop {
                let mut buf = [0; 1024];
                match server.try_read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        server.try_write(&buf[..n]).await.unwrap();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(e) => panic!("Error reading from pipe: {}", e),
                }
            }
        });

        Ok((pipe_name, rx))
    }

    #[tokio::test]
    async fn test_basic_pipe_communication() -> Result<()> {
        let (pipe_name, rx) = create_server_pipe().await?;

        // Wait for server to be ready
        rx.await?;

        let mut connection = Connection::connect(Path::new(&pipe_name)).await?;

        // Test write/read
        connection.write_all(b"hello\n").await?;
        let mut response = String::new();
        let n = connection.read_line(&mut response).await?;
        assert_eq!(n, 6);
        assert_eq!(response, "hello\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_nonexistent_pipe() -> Result<()> {
        let pipe_name = format!(r"\\.\pipe\nonexistent_{}", Uuid::new_v4());
        let result = Connection::connect(Path::new(&pipe_name)).await;
        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_pipe_busy() -> Result<()> {
        // Create a pipe but don't allow connections
        let pipe_name = format!(r"\\.\pipe\busy_{}", Uuid::new_v4());
        let _server = ServerOptions::new().create(&pipe_name)?;

        // Try to connect - should fail because server isn't accepting
        let result = Connection::connect(Path::new(&pipe_name)).await;
        assert!(result.is_err());

        Ok(())
    }
}

#[cfg(test)]
mod client_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct MockConnection {
        written: Arc<Mutex<Vec<u8>>>,
        responses: Vec<Result<String>>,
        response_index: usize,
    }

    impl MockConnection {
        fn new(responses: Vec<Result<String>>) -> Self {
            Self {
                written: Arc::new(Mutex::new(Vec::new())),
                responses,
                response_index: 0,
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::client::ConnectionTrait for MockConnection {
        async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            if self.response_index >= self.responses.len() {
                return Err(anyhow::anyhow!("Connection closed"));
            }
            self.written.lock().unwrap().extend_from_slice(buf);
            Ok(())
        }

        async fn read_line(&mut self, buf: &mut String) -> Result<usize> {
            match self.responses.get(self.response_index) {
                Some(Ok(response)) => {
                    buf.push_str(response);
                    self.response_index += 1;
                    Ok(response.len())
                }
                Some(Err(e)) => Err(anyhow::anyhow!(e.to_string())),
                None => Ok(0),
            }
        }
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestMessage {
        value: String,
    }

    #[tokio::test]
    async fn test_successful_message_exchange() -> Result<()> {
        let mock_conn = MockConnection::new(vec![Ok(
            r#"{"id":1,"content":{"value":"response"}}"#.to_string()
        )]);

        let mut client = Client {
            connection: Box::new(mock_conn),
            message_id: 0,
        };

        let request = TestMessage {
            value: "test".to_string(),
        };
        let response: TestMessage = client.send(request).await?;
        assert_eq!(response.value, "response");
        assert_eq!(client.message_id, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_connection_error() {
        let mock_conn = MockConnection::new(vec![Err(anyhow::anyhow!("Connection error"))]);

        let mut client = Client {
            connection: Box::new(mock_conn),
            message_id: 0,
        };

        let request = TestMessage {
            value: "test".to_string(),
        };
        let result: Result<TestMessage> = client.send(request).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Connection error"));
    }

    #[tokio::test]
    async fn test_id_mismatch() {
        let mock_conn = MockConnection::new(vec![Ok(
            r#"{"id":2,"content":{"value":"response"}}"#.to_string()
        )]);

        let mut client = Client {
            connection: Box::new(mock_conn),
            message_id: 0,
        };

        let request = TestMessage {
            value: "test".to_string(),
        };
        let result: Result<TestMessage> = client.send(request).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Message ID mismatch"));
    }

    #[tokio::test]
    async fn test_invalid_json_response() {
        let mock_conn = MockConnection::new(vec![Ok("invalid json".to_string())]);

        let mut client = Client {
            connection: Box::new(mock_conn),
            message_id: 0,
        };

        let request = TestMessage {
            value: "test".to_string(),
        };
        let result: Result<TestMessage> = client.send(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multiple_messages() -> Result<()> {
        let mock_conn = MockConnection::new(vec![
            Ok(r#"{"id":1,"content":{"value":"response1"}}"#.to_string()),
            Ok(r#"{"id":2,"content":{"value":"response2"}}"#.to_string()),
        ]);

        let mut client = Client {
            connection: Box::new(mock_conn),
            message_id: 0,
        };

        let request1 = TestMessage {
            value: "test1".to_string(),
        };
        let response1: TestMessage = client.send(request1).await?;
        assert_eq!(response1.value, "response1");
        assert_eq!(client.message_id, 1);

        let request2 = TestMessage {
            value: "test2".to_string(),
        };
        let response2: TestMessage = client.send(request2).await?;
        assert_eq!(response2.value, "response2");
        assert_eq!(client.message_id, 2);

        Ok(())
    }
}
