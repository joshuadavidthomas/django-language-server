use crate::django::Apps;
use crate::gis::{gdal_is_installed, has_geodjango, GISError};
use djls_python::Python;
use std::fmt;
use std::sync::Arc;

#[derive(Debug)]
pub struct DjangoProject {
    python_env: Arc<Python>,
    settings_module: String,
    installed_apps: Apps,
}

impl DjangoProject {
    fn new(python_env: Arc<Python>, settings_module: String, installed_apps: Apps) -> Self {
        Self {
            python_env,
            settings_module,
            installed_apps,
        }
    }

    pub fn setup(python_env: Arc<Python>) -> Result<Self, ProjectError> {
        let settings_module =
            std::env::var("DJANGO_SETTINGS_MODULE").expect("DJANGO_SETTINGS_MODULE must be set");

        python_env.run_python("import django")?;

        if has_geodjango(Arc::clone(&python_env))? && !gdal_is_installed() {
            eprintln!("Warning: GeoDjango detected but GDAL is not available.");
            eprintln!("Django initialization will be skipped. Some features may be limited.");
            eprintln!("To enable full functionality, please install GDAL and other GeoDjango prerequisites.");

            return Ok(Self {
                python_env,
                settings_module,
                installed_apps: Apps::default(),
            });
        }

        python_env.run_python(
            r#"
import django
django.setup()
        "#,
        )?;

        let apps_json = python_env.run_python(
            r#"
import json
from django.conf import settings
print(json.dumps(list(settings.INSTALLED_APPS)))
            "#,
        )?;

        let apps: Vec<String> = serde_json::from_str(&apps_json)?;
        let installed_apps = Apps::from_strings(apps);

        Ok(Self::new(python_env, settings_module, installed_apps))
    }

    pub fn python_env(&self) -> &Python {
        &self.python_env
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
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("GIS error: {0}")]
    Gis(#[from] GISError),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),
}
