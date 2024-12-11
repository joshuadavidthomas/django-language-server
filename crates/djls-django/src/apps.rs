use djls_ipc::{parse_json_response, JsonResponse, PythonProcess, TransportError};
use serde::Deserialize;
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

#[derive(Debug, Deserialize)]
struct InstalledAppsCheck {
    has_app: bool,
}

impl TryFrom<JsonResponse> for InstalledAppsCheck {
    type Error = TransportError;

    fn try_from(response: JsonResponse) -> Result<Self, Self::Error> {
        response
            .data()
            .clone()
            .ok_or_else(|| TransportError::Process("No data in response".to_string()))
            .and_then(|data| serde_json::from_value(data).map_err(TransportError::Json))
    }
}

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

    pub fn check_installed(python: &mut PythonProcess, app: &str) -> Result<bool, TransportError> {
        let response = python.send("installed_apps_check", Some(vec![app.to_string()]))?;
        let response = parse_json_response(response)?;
        let result = InstalledAppsCheck::try_from(response)?;
        Ok(result.has_app)
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
