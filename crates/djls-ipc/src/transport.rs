use crate::process::ProcessError;
use crate::proto::v1::*;
use prost::Message;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::process::{ChildStdin, ChildStdout};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Transport {
    reader: Arc<Mutex<BufReader<ChildStdout>>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
}

impl Transport {
    pub fn new(mut stdin: ChildStdin, mut stdout: ChildStdout) -> Result<Self, ProcessError> {
        stdin.flush().map_err(TransportError::Io)?;

        let mut ready_line = String::new();
        BufReader::new(&mut stdout)
            .read_line(&mut ready_line)
            .map_err(TransportError::Io)?;

        if ready_line.trim() != "ready" {
            return Err(ProcessError::Ready("Python process not ready".to_string()));
        }

        Ok(Self {
            reader: Arc::new(Mutex::new(BufReader::new(stdout))),
            writer: Arc::new(Mutex::new(BufWriter::new(stdin))),
        })
    }

    pub fn send(
        &mut self,
        message: messages::Request,
    ) -> Result<messages::Response, TransportError> {
        let buf = message.encode_to_vec();

        let mut writer = self.writer.lock().map_err(|_| {
            TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to acquire writer lock",
            ))
        })?;
        writer
            .write_all(&(buf.len() as u32).to_be_bytes())
            .map_err(TransportError::Io)?;
        writer.write_all(&buf).map_err(TransportError::Io)?;
        writer.flush().map_err(TransportError::Io)?;

        let mut reader = self.reader.lock().map_err(|_| {
            TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to acquire reader lock",
            ))
        })?;
        let mut length_bytes = [0u8; 4];
        reader
            .read_exact(&mut length_bytes)
            .map_err(TransportError::Io)?;
        let length = u32::from_be_bytes(length_bytes);

        let mut message_bytes = vec![0u8; length as usize];
        reader
            .read_exact(&mut message_bytes)
            .map_err(TransportError::Io)?;

        messages::Response::decode(message_bytes.as_slice())
            .map_err(|e| TransportError::Decode(e.to_string()))
    }
}

#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Task error: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("Failed to decode message: {0}")]
    Decode(String),
}
