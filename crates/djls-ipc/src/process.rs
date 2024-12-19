use crate::messages::{ErrorResponse, Message, Request, Response};
use crate::protocol::{Protocol, ProtocolError};
use crate::{Transport, TransportError};
use std::ffi::OsStr;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

pub struct PythonProcess<P: Protocol> {
    transport: Arc<Mutex<Transport>>,
    _child: Child,
    protocol: P,
}

impl<P: Protocol> PythonProcess<P> {
    pub fn new<I, S>(module: &str, args: Option<I>, protocol: P) -> Result<Self, ProcessError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("python");
        command.arg("-m").arg(module);

        if let Some(args) = args {
            command.args(args);
        }

        command.stdin(Stdio::piped()).stdout(Stdio::piped());

        let mut child = command.spawn().map_err(ProcessError::Spawn)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProcessError::Io("Failed to capture stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProcessError::Io("Failed to capture stdout".into()))?;

        let transport = Arc::new(Mutex::new(Transport::new(stdin, stdout)?));

        Ok(Self {
            transport,
            protocol,
            _child: child,
        })
    }

    pub fn send<M: Message>(&self, data: M::RequestData) -> Result<M, ProcessError> {
        let transport = self.transport.lock()?;

        let request: Request<M> = Request::new(data);
        let request_bytes = self.protocol.serialize(&request)?;
        let response_bytes = transport.send_and_receive(&request_bytes)?;
        let response: Response<M> = self.protocol.deserialize(&response_bytes)?;

        response.try_into_message().map_err(ProcessError::Response)
    }
}

impl<P: Protocol> Drop for PythonProcess<P> {
    fn drop(&mut self) {
        if let Ok(()) = self._child.kill() {
            let _ = self._child.wait();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("Failed to spawn process: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("Protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("Lock error: {0}")]
    Lock(String),
    #[error("Response error: {0}")]
    Response(#[from] ErrorResponse),
}

impl<T> From<std::sync::PoisonError<std::sync::MutexGuard<'_, T>>> for ProcessError {
    fn from(_: std::sync::PoisonError<std::sync::MutexGuard<'_, T>>) -> Self {
        ProcessError::Lock("Failed to acquire lock".into())
    }
}
