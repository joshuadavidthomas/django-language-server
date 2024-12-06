use crate::django::Apps;
use djls_python::PythonEnvironment;
use std::fmt;
use std::sync::Arc;

#[derive(Debug)]
pub struct DjangoProject {
    python_env: Arc<PythonEnvironment>,
    settings_module: String,
    installed_apps: Apps,
}

impl DjangoProject {
    fn new(
        python_env: Arc<PythonEnvironment>,
        settings_module: String,
        installed_apps: Apps,
    ) -> Self {
        Self {
            python_env,
            settings_module,
            installed_apps,
        }
    }

    pub fn setup(python_env: Arc<PythonEnvironment>) -> Result<Self, ProjectError> {
        let settings_module =
            std::env::var("DJANGO_SETTINGS_MODULE").expect("DJANGO_SETTINGS_MODULE must be set");

        python_env.py().run_python("import django")?;

        python_env.py().run_python(
            r#"
import django
django.setup()
        "#,
        )?;

        let apps_json = python_env.py().run_python(
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

    pub fn python_env(&self) -> &PythonEnvironment {
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

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),
}
