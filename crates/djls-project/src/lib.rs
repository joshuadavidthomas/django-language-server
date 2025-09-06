mod db;
mod meta;
mod python;
mod system;
mod templatetags;

use std::fmt;
use std::path::Path;
use std::path::PathBuf;

pub use db::find_python_environment;
pub use db::Db;
pub use meta::ProjectMetadata;
use pyo3::prelude::*;
pub use python::PythonEnvironment;
pub use templatetags::TemplateTags;

#[derive(Debug)]
pub struct DjangoProject {
    path: PathBuf,
    env: Option<PythonEnvironment>,
    template_tags: Option<TemplateTags>,
}

impl DjangoProject {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            env: None,
            template_tags: None,
        }
    }

    pub fn initialize(&mut self, db: &dyn Db) -> PyResult<()> {
        // Use the database to find the Python environment
        self.env = find_python_environment(db);
        if self.env.is_none() {
            return Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                "Could not find Python environment",
            ));
        }

        Python::with_gil(|py| {
            let sys = py.import("sys")?;
            let py_path = sys.getattr("path")?;

            if let Some(path_str) = self.path.to_str() {
                py_path.call_method1("insert", (0, path_str))?;
            }

            let env = self.env.as_ref().ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                    "Internal error: Python environment missing after initialization",
                )
            })?;
            env.activate(py)?;

            match py.import("django") {
                Ok(django) => {
                    django.call_method0("setup")?;
                    self.template_tags = Some(TemplateTags::from_python(py)?);
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Failed to import Django: {e}");
                    Err(e)
                }
            }
        })
    }

    #[must_use]
    pub fn template_tags(&self) -> Option<&TemplateTags> {
        self.template_tags.as_ref()
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for DjangoProject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Project path: {}", self.path.display())?;
        if let Some(py_env) = &self.env {
            write!(f, "{py_env}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn create_mock_django_project(dir: &Path) -> PathBuf {
        let project_path = dir.to_path_buf();
        fs::create_dir_all(&project_path).unwrap();

        // Create a mock Django project structure
        fs::create_dir_all(project_path.join("myapp")).unwrap();
        fs::create_dir_all(project_path.join("myapp/templates")).unwrap();
        fs::write(project_path.join("manage.py"), "#!/usr/bin/env python").unwrap();

        project_path
    }

    #[test]
    fn test_django_project_initialization() {
        // This test needs to be run in an environment with Django installed
        // For this test to pass, you would need a real Python environment with Django
        // Here we're just testing the creation of the DjangoProject object
        let project_dir = tempdir().unwrap();
        let project_path = create_mock_django_project(project_dir.path());

        let project = DjangoProject::new(project_path);

        assert!(project.env.is_none()); // Environment not initialized yet
        assert!(project.template_tags.is_none()); // Template tags not loaded yet
    }

    #[test]
    fn test_django_project_path() {
        let project_dir = tempdir().unwrap();
        let project_path = create_mock_django_project(project_dir.path());

        let project = DjangoProject::new(project_path.clone());

        assert_eq!(project.path(), project_path.as_path());
    }

    #[test]
    fn test_django_project_display() {
        let project_dir = tempdir().unwrap();
        let project_path = create_mock_django_project(project_dir.path());

        let project = DjangoProject::new(project_path.clone());

        let display_str = format!("{project}");
        assert!(display_str.contains(&format!("Project path: {}", project_path.display())));
    }
}
