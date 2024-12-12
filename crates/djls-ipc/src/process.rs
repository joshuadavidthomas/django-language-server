use crate::proto::v1::*;
use crate::transport::{Transport, TransportError};
use std::ffi::OsStr;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time;

#[derive(Debug)]
pub struct PythonProcess {
    transport: Arc<Mutex<Transport>>,
    _child: Child,
    healthy: Arc<AtomicBool>,
}

impl PythonProcess {
    pub fn new<I, S>(
        module: &str,
        args: Option<I>,
        health_check_interval: Option<Duration>,
    ) -> Result<Self, ProcessError>
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

        let mut child = command.spawn().map_err(TransportError::Io)?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let transport = Transport::new(stdin, stdout)?;

        let process = Self {
            transport: Arc::new(Mutex::new(transport)),
            _child: child,
            healthy: Arc::new(AtomicBool::new(true)),
        };

        if let Some(interval) = health_check_interval {
            let transport = process.transport.clone();
            let healthy = process.healthy.clone();
            tokio::spawn(async move {
                let mut interval = time::interval(interval);
                loop {
                    interval.tick().await;
                    let _ = PythonProcess::check_health(transport.clone(), healthy.clone()).await;
                }
            });
        }

        Ok(process)
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    pub fn send(
        &mut self,
        request: messages::Request,
    ) -> Result<messages::Response, TransportError> {
        let mut transport = self.transport.lock().unwrap();
        transport.send(request)
    }

    async fn check_health(
        transport: Arc<Mutex<Transport>>,
        healthy: Arc<AtomicBool>,
    ) -> Result<(), ProcessError> {
        let request = messages::Request {
            command: Some(messages::request::Command::CheckHealth(
                check::HealthRequest {},
            )),
        };

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                let mut transport = transport.lock().unwrap();
                transport.send(request)
            }),
        )
        .await
        .map_err(|_| ProcessError::Timeout(5))?
        .map_err(TransportError::Task)?
        .map_err(ProcessError::Transport)?;

        let result = match response.result {
            Some(messages::response::Result::CheckHealth(health)) => {
                if !health.passed {
                    let error_msg = health.error.unwrap_or_else(|| "Unknown error".to_string());
                    Err(ProcessError::Health(error_msg))
                } else {
                    Ok(())
                }
            }
            Some(messages::response::Result::Error(e)) => Err(ProcessError::Health(e.message)),
            _ => Err(ProcessError::Response),
        };

        healthy.store(result.is_ok(), Ordering::SeqCst);
        result
    }
}

impl Drop for PythonProcess {
    fn drop(&mut self) {
        if let Ok(()) = self._child.kill() {
            let _ = self._child.wait();
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ProcessError {
    #[error("Health check failed: {0}")]
    Health(String),
    #[error("Operation timed out after {0} seconds")]
    Timeout(u64),
    #[error("Unexpected response type")]
    Response,
    #[error("Failed to acquire lock: {0}")]
    Lock(String),
    #[error("Process not ready: {0}")]
    Ready(String),
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
}
