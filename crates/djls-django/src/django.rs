use djls_ipc::v1::*;
use djls_ipc::IpcCommand;
use djls_ipc::{ProcessError, PythonProcess, TransportError};
use djls_python::Python;
use std::fmt;

#[derive(Debug)]
pub struct DjangoProject {
    py: Python,
    python: PythonProcess,
    version: String,
}

impl DjangoProject {
    fn new(py: Python, python: PythonProcess, version: String) -> Self {
        Self {
            py,
            python,
            version,
        }
    }

    pub fn setup(mut python: PythonProcess) -> Result<Self, ProjectError> {
        let py = Python::setup(&mut python)?;

        match check::GeoDjangoPrereqsRequest::execute(&mut python)?.result {
            Some(messages::response::Result::CheckGeodjangoPrereqs(response)) => {
                if !response.passed {
                    eprintln!("Warning: GeoDjango detected but GDAL is not available.");
                    eprintln!(
                        "Django initialization will be skipped. Some features may be limited."
                    );
                    eprintln!("To enable full functionality, please install GDAL and other GeoDjango prerequisites.");

                    return Ok(Self {
                        py,
                        python,
                        version: String::new(),
                    });
                }
            }
            Some(messages::response::Result::Error(e)) => Err(ProcessError::Health(e.message))?,
            _ => Err(ProcessError::Response)?,
        }

        let response = django::GetProjectInfoRequest::execute(&mut python)?;

        let version = match response.result {
            Some(messages::response::Result::DjangoGetProjectInfo(response)) => {
                response.project.unwrap().version
            }
            _ => {
                return Err(ProjectError::Process(ProcessError::Response));
            }
        };

        Ok(Self {
            py,
            python,
            version,
        })
    }

    pub fn py(&self) -> &Python {
        &self.py
    }

    fn version(&self) -> &String {
        &self.version
    }
}

impl fmt::Display for DjangoProject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Django Project")?;
        writeln!(f, "Version: {}", self.version)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("Django is not installed or cannot be imported")]
    DjangoNotFound,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Packaging(#[from] djls_python::PackagingError),
    #[error("Process error: {0}")]
    Process(#[from] ProcessError),
    #[error(transparent)]
    Python(#[from] djls_python::PythonError),
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
}
