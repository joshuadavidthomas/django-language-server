//! Django environment discovery for the static project model.
//!
//! This module is intentionally not wired into project validation yet. It turns
//! Django-native settings-module configuration into path-scoped environments so
//! later static settings extraction can choose the right settings for a file and
//! still union results for completions.

#![allow(
    dead_code,
    reason = "Milestone A3 adds Django environment discovery before wiring static settings extraction."
)]

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DjangoEnvironmentConfig;
use djls_conf::Settings;

use crate::project::input::resolve_django_settings;
use crate::project::names::PyModuleName;
use crate::project::static_model::Fact;
use crate::project::static_model::Field;
use crate::project::static_model::ImportRoot;
use crate::project::static_model::Reason;
use crate::project::static_model::ReasonSource;
use crate::project::static_model::ResolvedModule;
use crate::project::static_resolver::resolve_module;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedDjangoEnvironment {
    pub(crate) root: Utf8PathBuf,
    pub(crate) django_settings: Fact<ResolvedModule>,
}

#[must_use]
pub(crate) fn discover_django_environments(
    workspace_root: &Utf8Path,
    settings: &Settings,
    module_search_paths: &[ImportRoot],
) -> Vec<ResolvedDjangoEnvironment> {
    if !settings.django_environments().is_empty() {
        return settings
            .django_environments()
            .iter()
            .map(|environment| {
                ResolvedDjangoEnvironment::from_config(
                    environment,
                    workspace_root,
                    module_search_paths,
                )
            })
            .collect();
    }

    resolve_django_settings(workspace_root, settings)
        .map(|module| {
            ResolvedDjangoEnvironment::from_module(
                workspace_root.to_path_buf(),
                &module,
                workspace_root,
                module_search_paths,
            )
        })
        .into_iter()
        .collect()
}

impl ResolvedDjangoEnvironment {
    fn from_config(
        config: &DjangoEnvironmentConfig,
        workspace_root: &Utf8Path,
        module_search_paths: &[ImportRoot],
    ) -> Self {
        let root = Utf8Path::new(config.root());
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else if root.as_str() == "." {
            workspace_root.to_path_buf()
        } else {
            workspace_root.join(root)
        };
        let Some(module) = config.django_settings_module() else {
            return Self::unknown(root, "Django environment must set django_settings_module");
        };

        Self::from_module(root, module, workspace_root, module_search_paths)
    }

    fn from_module(
        root: Utf8PathBuf,
        module: &str,
        workspace_root: &Utf8Path,
        module_search_paths: &[ImportRoot],
    ) -> Self {
        let Ok(module) = PyModuleName::parse(module) else {
            return Self::unknown(
                root,
                format!("django_settings_module is not a valid Python module path: {module}"),
            );
        };

        let resolution = resolve_module(module, module_search_paths, workspace_root);
        Self {
            root,
            django_settings: resolution.resolved,
        }
    }

    fn unknown(root: Utf8PathBuf, message: impl Into<String>) -> Self {
        let reason = Reason::new(
            Field::DjangoEnvironment,
            ReasonSource::DjangoEnvironment(root.clone()),
            message,
        );

        Self {
            root,
            django_settings: Fact::unknown(vec![reason]),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::project::static_resolver::discover_import_roots;

    fn settings(root: &Utf8Path) -> Settings {
        Settings::new(root, None).unwrap()
    }

    fn import_roots(root: &Utf8Path) -> Vec<ImportRoot> {
        discover_import_roots(root, &[], &[])
            .value()
            .cloned()
            .unwrap()
    }

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn known_settings(fact: &Fact<ResolvedModule>) -> ResolvedModule {
        let Fact::Known { value } = fact else {
            panic!("expected known Django settings module, got {fact:?}");
        };
        value.clone()
    }

    #[test]
    fn legacy_django_settings_module_becomes_default_environment() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_module = "project.settings""#,
        );

        let environments =
            discover_django_environments(&root, &settings(&root), &import_roots(&root));

        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].root, root);
        let settings = known_settings(&environments[0].django_settings);
        assert_eq!(settings.module, module("project.settings"));
        assert_eq!(settings.file, root.join("project/settings.py"));
    }

    #[test]
    fn explicit_django_environments_remain_path_scoped() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("projects/__init__.py"), "");
        write_file(&root.join("projects/site1/__init__.py"), "");
        write_file(&root.join("projects/site1/settings.py"), "");
        write_file(&root.join("projects/site2/__init__.py"), "");
        write_file(&root.join("projects/site2/settings.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"
[[django_environments]]
root = "projects/site1"
django_settings_module = "projects.site1.settings"

[[django_environments]]
root = "projects/site2"
django_settings_module = "projects.site2.settings"
"#,
        );

        let environments =
            discover_django_environments(&root, &settings(&root), &import_roots(&root));

        assert_eq!(environments.len(), 2);
        assert_eq!(environments[0].root, root.join("projects/site1"));
        let site1_settings = known_settings(&environments[0].django_settings);
        assert_eq!(site1_settings.module, module("projects.site1.settings"));
        assert_eq!(site1_settings.file, root.join("projects/site1/settings.py"));
        assert_eq!(environments[1].root, root.join("projects/site2"));
        let site2_settings = known_settings(&environments[1].django_settings);
        assert_eq!(site2_settings.module, module("projects.site2.settings"));
        assert_eq!(site2_settings.file, root.join("projects/site2/settings.py"));
    }
}
