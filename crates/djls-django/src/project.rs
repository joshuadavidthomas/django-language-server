use crate::django::Apps;
use crate::gis::{check_gis_setup, GISError};
use crate::scripts;
use crate::templates::TemplateTag;
use djls_python::{ImportCheck, Python, RunnerError, ScriptRunner};
use serde::Deserialize;
use std::fmt;

#[derive(Debug)]
pub struct DjangoProject {
    py: Python,
    settings_module: String,
    installed_apps: Apps,
    templatetags: Vec<TemplateTag>,
}

#[derive(Debug, Deserialize)]
pub struct DjangoSetup {
    apps: Vec<String>,
    tags: Vec<TemplateTag>,
}

impl ScriptRunner for DjangoSetup {
    const SCRIPT: &'static str = scripts::DJANGO_SETUP;
}

impl DjangoSetup {
    pub fn apps(&self) -> &[String] {
        &self.apps
    }

    pub fn tags(&self) -> &[TemplateTag] {
        &self.tags
    }
}

impl DjangoProject {
    fn new(
        py: Python,
        settings_module: String,
        installed_apps: Apps,
        templatetags: Vec<TemplateTag>,
    ) -> Self {
        Self {
            py,
            settings_module,
            installed_apps,
            templatetags,
        }
    }

    pub fn setup() -> Result<Self, ProjectError> {
        let settings_module =
            std::env::var("DJANGO_SETTINGS_MODULE").expect("DJANGO_SETTINGS_MODULE must be set");

        let py = Python::initialize()?;

        let has_django = ImportCheck::check(&py, "django")?;

        if !has_django {
            return Err(ProjectError::DjangoNotFound);
        }

        if !check_gis_setup(&py)? {
            eprintln!("Warning: GeoDjango detected but GDAL is not available.");
            eprintln!("Django initialization will be skipped. Some features may be limited.");
            eprintln!("To enable full functionality, please install GDAL and other GeoDjango prerequisites.");

            return Ok(Self {
                py,
                settings_module,
                installed_apps: Apps::default(),
                templatetags: Vec::new(),
            });
        }

        let setup = DjangoSetup::run_with_py(&py)?;

        Ok(Self::new(
            py,
            settings_module,
            Apps::from_strings(setup.apps().to_vec()),
            setup.tags().to_vec(),
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
        write!(f, "{}", self.installed_apps)?;
        write!(f, "{:?}", self.templatetags)?;
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
    Python(#[from] djls_python::PythonError),

    #[error(transparent)]
    Runner(#[from] RunnerError),
}
