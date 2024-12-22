use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{ChildStdin, ChildStdout};

use crate::serializers::Serializer;

#[derive(Error, Debug)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Encoding error: {0}")]
    Encode(String),
    #[error("Decoding error: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, TransportError>;

#[async_trait]
pub trait Transport<S: Serializer + Send + Sync> {
    async fn send<T: Serialize + Send + Sync>(&mut self, message: &T) -> Result<()>;
    async fn receive<T: for<'de> Deserialize<'de> + Send>(&mut self) -> Result<T>;
}

#[derive(Debug)]
pub struct LengthPrefixedTransport<R, W, S>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
    S: Serializer + Send + Sync,
{
    reader: BufReader<R>,
    writer: BufWriter<W>,
    _phantom: std::marker::PhantomData<S>,
}

impl<R, W, S> LengthPrefixedTransport<R, W, S>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
    S: Serializer + Send + Sync,
{
    pub fn new(read: R, write: W) -> Self {
        Self {
            reader: BufReader::new(read),
            writer: BufWriter::new(write),
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<R, W, S> Transport for LengthPrefixedTransport<R, W, S>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
    S: Serializer + Send + Sync,
{
    async fn send<T: Serialize + Send + Sync>(&mut self, message: &T) -> Result<()> {
        let encoded = S::encode(message).map_err(|e| TransportError::Encode(e.to_string()))?;
        let length = encoded.len() as u32;

        self.writer.write_all(&length.to_be_bytes()).await?;
        self.writer.write_all(&encoded).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn receive<T: for<'de> Deserialize<'de> + Send>(&mut self) -> Result<T> {
        let mut length_bytes = [0u8; 4];
        self.reader.read_exact(&mut length_bytes).await?;
        let length = u32::from_be_bytes(length_bytes);

        let mut buffer = vec![0u8; length as usize];
        self.reader.read_exact(&mut buffer).await?;
        let decoded = S::decode(&buffer).map_err(|e| TransportError::Decode(e.to_string()))?;

        Ok(decoded)
    }
}

// Type alias for the common process I/O case
pub type ProcessTransport<S> = LengthPrefixedTransport<ChildStdout, ChildStdin, S>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tokio::io::duplex;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestMessage {
        field1: String,
        field2: i32,
    }

    #[tokio::test]
    async fn test_transport_basic_roundtrip() {
        let (client, server) = duplex(1024);
        let mut transport = LengthPrefixedTransport::new(client, server);

        let message = TestMessage {
            field1: "test".to_string(),
            field2: 42,
        };

        transport.send(&message).await.unwrap();
        let received: TestMessage = transport.receive().await.unwrap();

        assert_eq!(message, received);
    }

    #[tokio::test]
    async fn test_transport_large_message() {
        let (client, server) = duplex(1024 * 1024); // 1MB buffer
        let mut transport = LengthPrefixedTransport::new(client, server);

        let large_string = "x".repeat(100_000);
        let message = TestMessage {
            field1: large_string,
            field2: 42,
        };

        transport.send(&message).await.unwrap();
        let received: TestMessage = transport.receive().await.unwrap();

        assert_eq!(message, received);
    }

    #[tokio::test]
    async fn test_transport_multiple_messages() {
        let (client, server) = duplex(1024);
        let mut transport = LengthPrefixedTransport::new(client, server);

        let messages = vec![
            TestMessage {
                field1: "first".to_string(),
                field2: 1,
            },
            TestMessage {
                field1: "second".to_string(),
                field2: 2,
            },
            TestMessage {
                field1: "third".to_string(),
                field2: 3,
            },
        ];

        for msg in &messages {
            transport.send(msg).await.unwrap();
        }

        for expected in &messages {
            let received: TestMessage = transport.receive().await.unwrap();
            assert_eq!(expected, &received);
        }
    }

    #[tokio::test]
    async fn test_transport_bidirectional() {
        // Create two pairs of streams for bidirectional communication
        let (client_rx, server_tx) = duplex(1024);
        let (server_rx, client_tx) = duplex(1024);

        let mut client_transport = LengthPrefixedTransport::new(client_rx, client_tx);
        let mut server_transport = LengthPrefixedTransport::new(server_rx, server_tx);

        // Send client -> server
        let msg1 = TestMessage {
            field1: "client".to_string(),
            field2: 1,
        };
        client_transport.send(&msg1).await.unwrap();
        let received1: TestMessage = server_transport.receive().await.unwrap();
        assert_eq!(msg1, received1);

        // Send server -> client
        let msg2 = TestMessage {
            field1: "server".to_string(),
            field2: 2,
        };
        server_transport.send(&msg2).await.unwrap();
        let received2: TestMessage = client_transport.receive().await.unwrap();
        assert_eq!(msg2, received2);
    }

    #[tokio::test]
    async fn test_transport_error_invalid_json() {
        let (client, server) = duplex(1024);
        let mut transport = LengthPrefixedTransport::new(client, server);

        // Send invalid JSON data
        let length: u32 = 5;
        transport
            .writer
            .write_all(&length.to_be_bytes())
            .await
            .unwrap();
        transport.writer.write_all(b"invalid").await.unwrap();
        transport.writer.flush().await.unwrap();

        // Try to receive it as a TestMessage
        let result: Result<TestMessage> = transport.receive().await;
        assert!(matches!(result, Err(TransportError::Decode(_))));
    }

    #[tokio::test]
    async fn test_transport_empty_message() {
        let (client, server) = duplex(1024);
        let mut transport = LengthPrefixedTransport::new(client, server);

        let message = TestMessage {
            field1: "".to_string(),
            field2: 0,
        };

        transport.send(&message).await.unwrap();
        let received: TestMessage = transport.receive().await.unwrap();

        assert_eq!(message, received);
    }

    // This test requires tokio::test(flavor = "multi_thread")
    #[tokio::test(flavor = "multi_thread")]
    async fn test_transport_concurrent_access() {
        let (client_rx, server_tx) = duplex(1024);
        let (server_rx, client_tx) = duplex(1024);

        let mut client_transport = LengthPrefixedTransport::new(client_rx, client_tx);
        let mut server_transport = LengthPrefixedTransport::new(server_rx, server_tx);

        let handle = tokio::spawn(async move {
            let msg = TestMessage {
                field1: "concurrent".to_string(),
                field2: 42,
            };
            server_transport.send(&msg).await.unwrap();
        });

        let received: TestMessage = client_transport.receive().await.unwrap();
        assert_eq!(received.field1, "concurrent");
        assert_eq!(received.field2, 42);

        handle.await.unwrap();
    }
}
