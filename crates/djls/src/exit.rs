use std::error::Error;
use std::fmt;

use anyhow::Result;

type ExitMessage = Option<String>;

#[derive(Debug)]
pub enum ExitStatus {
    Success,
    Error,
}

impl ExitStatus {
    pub fn as_raw(&self) -> i32 {
        match self {
            ExitStatus::Success => 0,
            ExitStatus::Error => 1,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            ExitStatus::Success => "Command succeeded",
            ExitStatus::Error => "Command error",
        }
    }
}

impl From<ExitStatus> for i32 {
    fn from(status: ExitStatus) -> Self {
        status.as_raw()
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = self.as_str();
        write!(f, "{msg}")
    }
}

#[derive(Debug)]
pub struct Exit {
    status: ExitStatus,
    message: ExitMessage,
}

impl Exit {
    fn new(status: ExitStatus) -> Self {
        Self {
            status,
            message: None,
        }
    }

    pub fn success() -> Self {
        Self::new(ExitStatus::Success)
    }

    pub fn error() -> Self {
        Self::new(ExitStatus::Error)
    }

    pub fn with_message<S: Into<String>>(mut self, message: S) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn process_exit(self) -> ! {
        if let Some(message) = self.message {
            println!("{message}");
        }
        std::process::exit(self.status.as_raw())
    }

    #[allow(dead_code)]
    pub fn ok(self) -> Result<()> {
        match self.status {
            ExitStatus::Success => Ok(()),
            ExitStatus::Error => Err(self.into()),
        }
    }

    #[allow(dead_code)]
    pub fn as_raw(&self) -> i32 {
        self.status.as_raw()
    }
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status_str = self.status.as_str();
        match &self.message {
            Some(msg) => write!(f, "{status_str}: {msg}"),
            None => write!(f, "{status_str}"),
        }
    }
}

impl Error for Exit {}
