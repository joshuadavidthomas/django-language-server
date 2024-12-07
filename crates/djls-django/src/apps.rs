use djls_python::{Python, RunnerError, ScriptRunner};
use serde::Deserialize;
use std::fmt;

use crate::scripts;

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

#[derive(Debug, Deserialize)]
struct InstalledAppsCheck {
    has_app: bool,
}

impl ScriptRunner for InstalledAppsCheck {
    const SCRIPT: &'static str = scripts::INSTALLED_APPS_CHECK;
}

impl Apps {
    pub fn from_strings(apps: Vec<String>) -> Self {
        Self(apps.into_iter().map(App).collect())
    }

    pub fn apps(&self) -> &[App] {
        &self.0
    }

    pub fn has_app(&self, name: &str) -> bool {
        self.0.iter().any(|app| app.0 == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &App> {
        self.0.iter()
    }

    pub fn check_installed(py: &Python, app: &str) -> Result<bool, RunnerError> {
        let result = InstalledAppsCheck::run_with_py_args(py, app)?;
        Ok(result.has_app)
    }
}

impl fmt::Display for Apps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Installed Apps:")?;
        for app in &self.0 {
            writeln!(f, "  {}", app)?;
        }
        Ok(())
    }
}
