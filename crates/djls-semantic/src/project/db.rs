//! Project-specific database trait and queries.
//!
//! This module extends the workspace database trait with project-specific
//! functionality including metadata access and Python environment discovery.
//!
//! ## Architecture
//!
//! Following the Salsa pattern established in workspace and templates crates:
//! - `DjangoProject` is a Salsa input representing external project state
//! - Tracked functions compute derived values (Python env, Django config)
//! - Database trait provides stable configuration (metadata, template tags)

use std::sync::Arc;

use camino::Utf8PathBuf;
use salsa::Setter;

use crate::project::introspector::ProjectIntrospector;
use crate::project::Project;

/// Project-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: salsa::Database {
    /// Get the current project (if set)
    fn project(&self) -> Option<Project>;

    /// Get the shared project introspector.
    fn project_introspector(&self) -> Arc<ProjectIntrospector>;

    /// Populate template libraries from the filesystem cache, if available.
    ///
    /// This is a fast, synchronous startup path. It gives completions and
    /// diagnostics previously discovered library data while fresh project
    /// introspection runs in the background.
    fn load_template_library_cache(&mut self) -> bool
    where
        Self: Sized,
    {
        let Some(project) = self.project() else {
            return false;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let dsm = project.django_settings_module(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let Some(response) = super::cache::load_cached_template_library_snapshot(
            &root,
            &interpreter,
            dsm.as_deref(),
            &pythonpath,
        ) else {
            return false;
        };

        let current = project.template_libraries(self).clone();
        let next = current.apply_active_snapshot(Some(response));
        if project.template_libraries(self) != &next {
            project.set_template_libraries(self).to(next);
        }

        true
    }

    /// Refresh active template libraries from the configured project introspector.
    fn refresh_template_libraries(&mut self)
    where
        Self: Sized,
    {
        let Some(project) = self.project() else {
            return;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let dsm = project.django_settings_module(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let response = super::symbols::fetch_template_library_snapshot(self);

        if let Some(ref response) = response {
            super::cache::save_template_library_snapshot(
                &root,
                &interpreter,
                dsm.as_deref(),
                &pythonpath,
                response,
            );
        }

        let current = project.template_libraries(self).clone();
        let next = current.apply_active_snapshot(response);
        if project.template_libraries(self) != &next {
            project.set_template_libraries(self).to(next);
        }
    }

    /// Refresh all external project data.
    ///
    /// Updates active template library data from project introspection, then
    /// scans installed packages for validation rules and model definitions.
    /// Workspace files are handled separately by tracked Salsa queries.
    fn refresh_external_data(&mut self)
    where
        Self: Sized,
    {
        self.refresh_template_libraries();
        self.refresh_external_semantic_data();
    }

    /// Refresh external semantic data for the current project.
    ///
    /// This scans installed packages for template rule extraction data and
    /// model definitions. Workspace files are handled by tracked Salsa queries.
    fn refresh_external_semantic_data(&mut self)
    where
        Self: Sized,
    {
        super::external::refresh_external_semantic_data(self);
    }

    /// Return the current project root or fall back to the current working directory.
    fn project_root_or_cwd(&self) -> Utf8PathBuf {
        if let Some(project) = self.project() {
            project.root(self).clone()
        } else if let Ok(current_dir) = std::env::current_dir() {
            Utf8PathBuf::from_path_buf(current_dir).unwrap_or_else(|_| Utf8PathBuf::from("."))
        } else {
            Utf8PathBuf::from(".")
        }
    }
}
