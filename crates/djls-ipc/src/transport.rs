use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Debug;
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
        }
    }
}

pub trait TransportProtocol: Debug + Send {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Result<Self, TransportError>
    where
        Self: Sized;
    fn health_check(&mut self) -> Result<(), TransportError>;
    fn clone_box(&self) -> Box<dyn TransportProtocol>;
    fn send_impl(
        &mut self,
        message: &str,
        args: Option<Vec<String>>,
    ) -> Result<String, TransportError>;

    fn send(&mut self, message: &str, args: Option<Vec<String>>) -> Result<String, TransportError> {
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
        self.send_impl("health", None)
            .and_then(|response| match response.as_str() {
                "ok" => Ok(()),
                other => Err(TransportError::Process(format!(
                    "Health check failed: {}",
                    other
                ))),
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
        message: &str,
        args: Option<Vec<String>>,
    ) -> Result<String, TransportError> {
        let mut writer = self.writer.lock().unwrap();

        if let Some(args) = args {
            // Join command and args with spaces
            writeln!(writer, "{} {}", message, args.join(" ")).map_err(TransportError::Io)?;
        } else {
            writeln!(writer, "{}", message).map_err(TransportError::Io)?;
        }

        writer.flush().map_err(TransportError::Io)?;

        let mut reader = self.reader.lock().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).map_err(TransportError::Io)?;
        Ok(line.trim().to_string())
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
        self.send_impl("health", None).and_then(|response| {
            let json: JsonResponse = serde_json::from_str(&response)?;
            match json.status.as_str() {
                "ok" => Ok(()),
                _ => Err(TransportError::Process(
                    json.error.unwrap_or_else(|| "Unknown error".to_string()),
                )),
            }
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
        message: &str,
        args: Option<Vec<String>>,
    ) -> Result<String, TransportError> {
        let command = JsonCommand {
            command: message.to_string(),
            args,
        };

        let mut writer = self.writer.lock().unwrap();
        serde_json::to_writer(&mut *writer, &command)?;
        writeln!(writer).map_err(TransportError::Io)?;
        writer.flush().map_err(TransportError::Io)?;

        let mut reader = self.reader.lock().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).map_err(TransportError::Io)?;
        Ok(line.trim().to_string())
    }
}

pub fn parse_raw_response(response: String) -> Result<String, TransportError> {
    Ok(response)
}

pub fn parse_json_response(response: String) -> Result<JsonResponse, TransportError> {
    serde_json::from_str(&response).map_err(TransportError::Json)
}
