use djls_ipc::v1::*;
use djls_ipc::{ProcessError, PythonProcess};
use std::fmt;

#[derive(Debug)]
pub struct App(String);

impl App {
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Default)]
pub struct Apps(Vec<App>);

impl Apps {
    pub fn from_strings(apps: Vec<String>) -> Self {
        Self(apps.into_iter().map(App).collect())
    }

    pub fn apps(&self) -> &[App] {
        &self.0
    }

    pub fn has_app(&self, name: &str) -> bool {
        self.apps().iter().any(|app| app.0 == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &App> {
        self.apps().iter()
    }

    pub fn check_installed(python: &mut PythonProcess, app: &str) -> Result<bool, ProcessError> {
        let request = messages::Request {
            command: Some(messages::request::Command::CheckAppInstalled(
                check::AppInstalledRequest {
                    app_name: app.to_string(),
                },
            )),
        };

        let response = python.send(request).map_err(ProcessError::Transport)?;

        match response.result {
            Some(messages::response::Result::CheckAppInstalled(response)) => Ok(response.passed),
            Some(messages::response::Result::Error(e)) => Err(ProcessError::Health(e.message)),
            _ => Err(ProcessError::Response),
        }
    }
}

impl fmt::Display for Apps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for app in &self.0 {
            writeln!(f, "  {}", app)?;
        }
        Ok(())
    }
}
