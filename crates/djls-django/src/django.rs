use crate::apps::Apps;
use crate::gis::{check_gis_setup, GISError};
use crate::templates::TemplateTags;
use djls_ipc::{JsonResponse, PythonProcess, TransportError, TransportMessage, TransportResponse};
use djls_python::{ImportCheck, Python};
use serde::Deserialize;
use std::fmt;

#[derive(Debug)]
pub struct DjangoProject {
    py: Python,
    python: PythonProcess,
    settings_module: String,
    installed_apps: Apps,
    templatetags: TemplateTags,
}

#[derive(Debug, Deserialize)]
struct DjangoSetup {
    installed_apps: Vec<String>,
    templatetags: TemplateTags,
}

impl DjangoSetup {
    pub fn setup(python: &mut PythonProcess) -> Result<JsonResponse, ProjectError> {
        let message = TransportMessage::Json("django_setup".to_string());
        let response = python.send(message, None)?;
        match response {
            TransportResponse::Json(json_str) => {
                let json_response: JsonResponse = serde_json::from_str(&json_str)?;
                Ok(json_response)
            }
            _ => Err(ProjectError::Transport(TransportError::Process(
                "Unexpected response type".to_string(),
            ))),
        }
    }
}

impl DjangoProject {
    fn new(
        py: Python,
        python: PythonProcess,
        settings_module: String,
        installed_apps: Apps,
        templatetags: TemplateTags,
    ) -> Self {
        Self {
            py,
            python,
            settings_module,
            installed_apps,
            templatetags,
        }
    }

    pub fn setup(mut python: PythonProcess) -> Result<Self, ProjectError> {
        let settings_module =
            std::env::var("DJANGO_SETTINGS_MODULE").expect("DJANGO_SETTINGS_MODULE must be set");

        let py = Python::setup(&mut python)?;

        let has_django = ImportCheck::check(&mut python, Some(vec!["django".to_string()]))?;

        if !has_django {
            return Err(ProjectError::DjangoNotFound);
        }

        if !check_gis_setup(&mut python)? {
            eprintln!("Warning: GeoDjango detected but GDAL is not available.");
            eprintln!("Django initialization will be skipped. Some features may be limited.");
            eprintln!("To enable full functionality, please install GDAL and other GeoDjango prerequisites.");

            return Ok(Self {
                py,
                python,
                settings_module,
                installed_apps: Apps::default(),
                templatetags: TemplateTags::default(),
            });
        }

        let response = DjangoSetup::setup(&mut python)?;
        let setup: DjangoSetup = response
            .data()
            .clone()
            .ok_or_else(|| TransportError::Process("No data in response".to_string()))
            .and_then(|data| serde_json::from_value(data).map_err(TransportError::Json))?;

        Ok(Self::new(
            py,
            python,
            settings_module,
            Apps::from_strings(setup.installed_apps.to_vec()),
            setup.templatetags,
        ))
    }

    pub fn py(&self) -> &Python {
        &self.py
    }

    fn settings_module(&self) -> &String {
        &self.settings_module
    }
}

impl fmt::Display for DjangoProject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Django Project")?;
        writeln!(f, "Settings Module: {}", self.settings_module)?;
        writeln!(f, "Installed Apps:")?;
        write!(f, "{}", self.installed_apps)?;
        writeln!(f, "Template Tags:")?;
        write!(f, "{}", self.templatetags)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("Django is not installed or cannot be imported")]
    DjangoNotFound,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("GIS error: {0}")]
    Gis(#[from] GISError),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Packaging(#[from] djls_python::PackagingError),

    #[error(transparent)]
    Python(#[from] djls_python::PythonError),

    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
}
