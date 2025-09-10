mod db;
pub mod inspector;
mod meta;
pub mod python;
mod system;
mod templatetags;

use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
pub use db::find_python_environment;
pub use db::Db;
use inspector::pool::InspectorPool;
use inspector::DjlsRequest;
use inspector::Query;
pub use meta::ProjectMetadata;
pub use python::PythonEnvironment;
pub use templatetags::TemplateTags;

#[derive(Debug)]
pub struct DjangoProject {
    path: PathBuf,
    env: Option<PythonEnvironment>,
    template_tags: Option<TemplateTags>,
    inspector: Arc<InspectorPool>,
}

impl DjangoProject {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            env: None,
            template_tags: None,
            inspector: Arc::new(InspectorPool::new()),
        }
    }

    pub fn initialize(&mut self, db: &dyn Db) -> Result<()> {
        // Use the database to find the Python environment
        self.env = find_python_environment(db);
        let env = self
            .env
            .as_ref()
            .context("Could not find Python environment")?;

        // Initialize Django
        let request = DjlsRequest {
            query: Query::DjangoInit,
        };
        let response = self.inspector.query(env, &self.path, &request)?;

        if !response.ok {
            anyhow::bail!("Failed to initialize Django: {:?}", response.error);
        }

        // Get template tags
        let request = DjlsRequest {
            query: Query::Templatetags,
        };
        let response = self.inspector.query(env, &self.path, &request)?;

        if let Some(data) = response.data {
            self.template_tags = Some(TemplateTags::from_json(&data)?);
        }

        Ok(())
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
