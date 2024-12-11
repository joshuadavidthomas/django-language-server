use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Clone, Copy, Debug)]
struct ConnectionConfig {
    max_retries: u16,
    initial_delay_ms: u16,
    max_delay_ms: u32,
    backoff_factor: f32,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_factor: 2.0,
        }
    }
}

#[derive(Debug)]
struct Connection {
    stream: UnixStream,
}

impl Connection {
    async fn connect(path: &Path) -> Result<Self> {
        Self::connect_with_config(path, ConnectionConfig::default()).await
    }

    async fn connect_with_config(path: &Path, config: ConnectionConfig) -> Result<Self> {
        let mut current_delay = u64::from(config.initial_delay_ms);
        let mut last_error = None;

        for attempt in 0..config.max_retries {
            match UnixStream::connect(path).await {
                Ok(stream) => return Ok(Self { stream }),
                Err(e) => {
                    last_error = Some(e);

                    if attempt < config.max_retries - 1 {
                        tokio::time::sleep(Duration::from_millis(current_delay)).await;

                        current_delay = ((current_delay as f32 * config.backoff_factor) as u64)
                            .min(u64::from(config.max_delay_ms));
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Unknown error"))
            .into())
    }

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

#[derive(Debug, Serialize, Deserialize)]
pub struct Message<T> {
    pub id: u64,
    pub content: T,
}

#[derive(Debug)]
pub struct Client {
    connection: Connection,
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

#[cfg(test)]
mod conn_tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::net::UnixListener;
    use tokio::sync::oneshot;

    fn test_config() -> ConnectionConfig {
        ConnectionConfig {
            max_retries: 5,
            initial_delay_ms: 10,
            max_delay_ms: 100,
            backoff_factor: 2.0,
        }
    }

    #[tokio::test]
    async fn test_unix_connection() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let socket_path = temp_file.path().to_owned();
        temp_file.close()?;

        // Channel to signal when server is ready
        let (tx, rx) = oneshot::channel();

        let listener = UnixListener::bind(&socket_path)?;

        tokio::spawn(async move {
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

        rx.await?;

        let mut connection = Connection::connect(&socket_path).await?;

        // single message
        connection.write_all(b"hello\n").await?;
        let mut response = String::new();
        let n = connection.read_line(&mut response).await?;
        assert_eq!(n, 6);
        assert_eq!(response, "hello\n");

        // multiple messages
        for i in 0..3 {
            let msg = format!("message{}\n", i);
            connection.write_all(msg.as_bytes()).await?;
            let mut response = String::new();
            let n = connection.read_line(&mut response).await?;
            assert_eq!(n, msg.len());
            assert_eq!(response, msg);
        }

        // large message
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

    #[tokio::test]
    async fn test_connection_retry() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let socket_path = temp_file.path().to_owned();
        temp_file.close()?;

        let socket_path_clone = socket_path.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;

            let listener = tokio::net::UnixListener::bind(&socket_path_clone).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            drop(stream);
        });

        let start = std::time::Instant::now();
        let _connection = Connection::connect_with_config(&socket_path, test_config()).await?;
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(30),
            "Connection succeeded too quickly ({:?}), should have retried",
            elapsed
        );

        assert!(
            elapsed < Duration::from_millis(100),
            "Connection took too long ({:?}), too many retries",
            elapsed
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_connection_max_retries() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let socket_path = temp_file.path().to_owned();
        temp_file.close()?;

        let start = std::time::Instant::now();
        let result = Connection::connect_with_config(&socket_path, test_config()).await;
        let elapsed = start.elapsed();

        assert!(result.is_err());

        // Should have waited approximately
        // 0 + 10 + 20 + 40 + 80 ~= 150ms
        assert!(
            elapsed >= Duration::from_millis(150),
            "Didn't retry enough times ({:?})",
            elapsed
        );
        assert!(
            elapsed < Duration::from_millis(200),
            "Retried for too long ({:?})",
            elapsed
        );

        Ok(())
    }
}

#[cfg(test)]
mod client_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
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

    impl MockConnection {
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

    #[derive(Debug)]
    struct TestClient {
        connection: MockConnection,
        message_id: u64,
    }

    impl TestClient {
        fn new(mock_conn: MockConnection) -> Self {
            Self {
                connection: mock_conn,
                message_id: 0,
            }
        }

        async fn send<T, R>(&mut self, content: T) -> Result<R>
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

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestMessage {
        value: String,
    }

    #[tokio::test]
    async fn test_successful_message_exchange() -> Result<()> {
        let mock_conn = MockConnection::new(vec![Ok(
            r#"{"id":1,"content":{"value":"response"}}"#.to_string()
        )]);

        let mut client = TestClient::new(mock_conn);

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

        let mut client = TestClient::new(mock_conn);

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

        let mut client = TestClient::new(mock_conn);

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

        let mut client = TestClient::new(mock_conn);

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

        let mut client = TestClient::new(mock_conn);

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
