use crate::protocol::ProtocolError;
use std::io::{BufReader, BufWriter, Read, Write};
use std::process::{ChildStdin, ChildStdout};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Transport {
    reader: Arc<Mutex<BufReader<ChildStdout>>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
}

impl Transport {
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Result<Self, TransportError> {
        Ok(Self {
            reader: Arc::new(Mutex::new(BufReader::new(stdout))),
            writer: Arc::new(Mutex::new(BufWriter::new(stdin))),
        })
    }

    pub fn send_and_receive(&self, request: &[u8]) -> Result<Vec<u8>, TransportError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| TransportError::Lock("Failed to acquire writer lock".into()))?;

        writer
            .write_all(&(request.len() as u32).to_be_bytes())
            .map_err(TransportError::Io)?;
        writer.write_all(request).map_err(TransportError::Io)?;
        writer.flush().map_err(TransportError::Io)?;

        let mut reader = self
            .reader
            .lock()
            .map_err(|_| TransportError::Lock("Failed to acquire reader lock".into()))?;

        let mut buf = [0u8; 1];
        while reader.read_exact(&mut buf).is_ok() {
            if buf[0] == 0 {
                let mut rest = [0u8; 3];
                if reader.read_exact(&mut rest).is_ok() {
                    let length_bytes = [buf[0], rest[0], rest[1], rest[2]];
                    let length = u32::from_be_bytes(length_bytes);

                    if length <= 1_000_000 {
                        let mut response = vec![0u8; length as usize];
                        reader
                            .read_exact(&mut response)
                            .map_err(TransportError::Io)?;
                        return Ok(response);
                    }
                }
            }
        }

        Err(TransportError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Failed to sync with response stream",
        )))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Lock error: {0}")]
    Lock(String),
    #[error("Protocol error: {0}")]
    Protocol(#[from] ProtocolError),
}
