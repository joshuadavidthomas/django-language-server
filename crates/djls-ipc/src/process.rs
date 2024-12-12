use crate::transport::{
    Transport, TransportError, TransportMessage, TransportProtocol, TransportResponse,
};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time;

#[derive(Debug)]
pub struct PythonProcess {
    transport: Arc<Mutex<Box<dyn TransportProtocol>>>,
    _child: Child,
    healthy: Arc<AtomicBool>,
}

impl PythonProcess {
    pub fn new(
        module: &str,
        transport: Transport,
        health_check_interval: Option<Duration>,
    ) -> Result<Self, TransportError> {
        let mut child = Command::new("python")
            .arg("-m")
            .arg(module)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let process = Self {
            transport: Arc::new(Mutex::new(transport.create(stdin, stdout)?)),
            _child: child,
            healthy: Arc::new(AtomicBool::new(true)),
        };

        if let Some(interval) = health_check_interval {
            process.start_health_check_task(interval)?;
        }

        Ok(process)
    }

    fn start_health_check_task(&self, interval: Duration) -> Result<(), TransportError> {
        let healthy = self.healthy.clone();
        let transport = self.transport.clone();

        tokio::spawn(async move {
            let mut interval = time::interval(interval);
            loop {
                interval.tick().await;

                if let Ok(mut transport) = transport.lock() {
                    match transport.health_check() {
                        Ok(()) => {
                            healthy.store(true, Ordering::SeqCst);
                        }
                        Err(_) => {
                            healthy.store(false, Ordering::SeqCst);
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    pub fn send(
        &mut self,
        message: TransportMessage,
        args: Option<Vec<String>>,
    ) -> Result<TransportResponse, TransportError> {
        let mut transport = self.transport.lock().unwrap();
        transport.send(message, args)
    }
}
