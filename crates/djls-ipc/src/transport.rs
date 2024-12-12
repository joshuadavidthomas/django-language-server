use djls_types::proto::*;
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Debug;
use std::io::Read;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{ChildStdin, ChildStdout};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Process error: {0}")]
    Process(String),
}

pub enum Transport {
    Raw,
    Json,
    Protobuf,
}

impl Transport {
    pub fn create(
        &self,
        mut stdin: ChildStdin,
        mut stdout: ChildStdout,
    ) -> Result<Box<dyn TransportProtocol>, TransportError> {
        let transport_type = match self {
            Transport::Raw => "raw",
            Transport::Json => "json",
            Transport::Protobuf => "protobuf",
        };

        writeln!(stdin, "{}", transport_type).map_err(TransportError::Io)?;
        stdin.flush().map_err(TransportError::Io)?;

        let mut ready_line = String::new();
        BufReader::new(&mut stdout)
            .read_line(&mut ready_line)
            .map_err(TransportError::Io)?;
        if ready_line.trim() != "ready" {
            return Err(TransportError::Process(
                "Python process not ready".to_string(),
            ));
        }

        match self {
            Transport::Raw => Ok(Box::new(RawTransport::new(stdin, stdout)?)),
            Transport::Json => Ok(Box::new(JsonTransport::new(stdin, stdout)?)),
            Transport::Protobuf => Ok(Box::new(ProtobufTransport::new(stdin, stdout)?)),
        }
    }
}

#[derive(Debug)]
pub enum TransportMessage {
    Raw(String),
    Json(String),
    Protobuf(ToAgent),
}

#[derive(Debug)]
pub enum TransportResponse {
    Raw(String),
    Json(String),
    Protobuf(FromAgent),
}

pub trait TransportProtocol: Debug + Send {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Result<Self, TransportError>
    where
        Self: Sized;
    fn health_check(&mut self) -> Result<(), TransportError>;
    fn clone_box(&self) -> Box<dyn TransportProtocol>;
    fn send_impl(
        &mut self,
        message: TransportMessage,
        args: Option<Vec<String>>,
    ) -> Result<TransportResponse, TransportError>;

    fn send(
        &mut self,
        message: TransportMessage,
        args: Option<Vec<String>>,
    ) -> Result<TransportResponse, TransportError> {
        self.health_check()?;
        self.send_impl(message, args)
    }
}

impl Clone for Box<dyn TransportProtocol> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Debug)]
pub struct RawTransport {
    reader: Arc<Mutex<BufReader<ChildStdout>>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
}

impl TransportProtocol for RawTransport {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Result<Self, TransportError> {
        Ok(Self {
            reader: Arc::new(Mutex::new(BufReader::new(stdout))),
            writer: Arc::new(Mutex::new(BufWriter::new(stdin))),
        })
    }

    fn health_check(&mut self) -> Result<(), TransportError> {
        self.send_impl(TransportMessage::Raw("health".to_string()), None)
            .and_then(|response| match response {
                TransportResponse::Raw(s) if s == "ok" => Ok(()),
                TransportResponse::Raw(other) => Err(TransportError::Process(format!(
                    "Health check failed: {}",
                    other
                ))),
                _ => Err(TransportError::Process(
                    "Unexpected response type".to_string(),
                )),
            })
    }

    fn clone_box(&self) -> Box<dyn TransportProtocol> {
        Box::new(RawTransport {
            reader: self.reader.clone(),
            writer: self.writer.clone(),
        })
    }

    fn send_impl(
        &mut self,
        message: TransportMessage,
        args: Option<Vec<String>>,
    ) -> Result<TransportResponse, TransportError> {
        let mut writer = self.writer.lock().unwrap();

        match message {
            TransportMessage::Raw(msg) => {
                if let Some(args) = args {
                    writeln!(writer, "{} {}", msg, args.join(" ")).map_err(TransportError::Io)?;
                } else {
                    writeln!(writer, "{}", msg).map_err(TransportError::Io)?;
                }
            }
            _ => {
                return Err(TransportError::Process(
                    "Raw transport only accepts raw messages".to_string(),
                ))
            }
        }

        writer.flush().map_err(TransportError::Io)?;

        let mut reader = self.reader.lock().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).map_err(TransportError::Io)?;
        Ok(TransportResponse::Raw(line.trim().to_string()))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonCommand {
    command: String,
    args: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonResponse {
    status: String,
    data: Option<Value>,
    error: Option<String>,
}

impl JsonResponse {
    pub fn data(&self) -> &Option<Value> {
        &self.data
    }
}

#[derive(Debug)]
pub struct JsonTransport {
    reader: Arc<Mutex<BufReader<ChildStdout>>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
}

impl TransportProtocol for JsonTransport {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Result<Self, TransportError> {
        Ok(Self {
            reader: Arc::new(Mutex::new(BufReader::new(stdout))),
            writer: Arc::new(Mutex::new(BufWriter::new(stdin))),
        })
    }

    fn health_check(&mut self) -> Result<(), TransportError> {
        self.send_impl(TransportMessage::Json("health".to_string()), None)
            .and_then(|response| match response {
                TransportResponse::Json(json) => {
                    let resp: JsonResponse = serde_json::from_str(&json)?;
                    match resp.status.as_str() {
                        "ok" => Ok(()),
                        _ => Err(TransportError::Process(
                            resp.error.unwrap_or_else(|| "Unknown error".to_string()),
                        )),
                    }
                }
                _ => Err(TransportError::Process(
                    "Unexpected response type".to_string(),
                )),
            })
    }

    fn clone_box(&self) -> Box<dyn TransportProtocol> {
        Box::new(JsonTransport {
            reader: self.reader.clone(),
            writer: self.writer.clone(),
        })
    }

    fn send_impl(
        &mut self,
        message: TransportMessage,
        args: Option<Vec<String>>,
    ) -> Result<TransportResponse, TransportError> {
        let mut writer = self.writer.lock().unwrap();

        match message {
            TransportMessage::Json(msg) => {
                let command = JsonCommand { command: msg, args };
                serde_json::to_writer(&mut *writer, &command)?;
                writeln!(writer).map_err(TransportError::Io)?;
            }
            _ => {
                return Err(TransportError::Process(
                    "JSON transport only accepts JSON messages".to_string(),
                ))
            }
        }

        writer.flush().map_err(TransportError::Io)?;

        let mut reader = self.reader.lock().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).map_err(TransportError::Io)?;
        Ok(TransportResponse::Json(line.trim().to_string()))
    }
}

#[derive(Debug)]
pub struct ProtobufTransport {
    reader: Arc<Mutex<BufReader<ChildStdout>>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
}

impl TransportProtocol for ProtobufTransport {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Result<Self, TransportError> {
        Ok(Self {
            reader: Arc::new(Mutex::new(BufReader::new(stdout))),
            writer: Arc::new(Mutex::new(BufWriter::new(stdin))),
        })
    }

    fn health_check(&mut self) -> Result<(), TransportError> {
        let request = ToAgent {
            command: Some(to_agent::Command::HealthCheck(HealthCheck {})),
        };

        match self.send_impl(TransportMessage::Protobuf(request), None)? {
            TransportResponse::Protobuf(FromAgent {
                message: Some(from_agent::Message::Error(e)),
            }) => Err(TransportError::Process(e.message)),
            TransportResponse::Protobuf(FromAgent {
                message: Some(from_agent::Message::HealthCheck(_)),
            }) => Ok(()),
            _ => Err(TransportError::Process("Unexpected response".to_string())),
        }
    }

    fn clone_box(&self) -> Box<dyn TransportProtocol> {
        Box::new(ProtobufTransport {
            reader: self.reader.clone(),
            writer: self.writer.clone(),
        })
    }

    fn send_impl(
        &mut self,
        message: TransportMessage,
        _args: Option<Vec<String>>,
    ) -> Result<TransportResponse, TransportError> {
        let mut writer = self.writer.lock().unwrap();

        match message {
            TransportMessage::Protobuf(msg) => {
                let buf = msg.encode_to_vec();
                writer
                    .write_all(&(buf.len() as u32).to_be_bytes())
                    .map_err(TransportError::Io)?;
                writer.write_all(&buf).map_err(TransportError::Io)?;
            }
            _ => {
                return Err(TransportError::Process(
                    "Protobuf transport only accepts protobuf messages".to_string(),
                ))
            }
        }

        writer.flush().map_err(TransportError::Io)?;

        let mut reader = self.reader.lock().unwrap();
        let mut length_bytes = [0u8; 4];
        reader
            .read_exact(&mut length_bytes)
            .map_err(TransportError::Io)?;
        let length = u32::from_be_bytes(length_bytes);

        let mut message_bytes = vec![0u8; length as usize];
        reader
            .read_exact(&mut message_bytes)
            .map_err(TransportError::Io)?;

        let response = FromAgent::decode(message_bytes.as_slice())
            .map_err(|e| TransportError::Process(e.to_string()))?;

        Ok(TransportResponse::Protobuf(response))
    }
}

pub fn parse_raw_response(response: String) -> Result<String, TransportError> {
    Ok(response)
}

pub fn parse_json_response(response: String) -> Result<JsonResponse, TransportError> {
    serde_json::from_str(&response).map_err(TransportError::Json)
}
