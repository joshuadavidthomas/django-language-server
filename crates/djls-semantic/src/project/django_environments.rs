//! Django environment discovery for project facts.
//!
//! This module turns Django-native settings-module configuration into path-scoped
//! environments. Environment discovery is intentionally separate from settings
//! fact extraction: first find the settings module and file for each path scope,
//! then extract facts from those settings files.

#![allow(
    dead_code,
    reason = "Milestone A3 adds Django environment discovery before project facts are assembled."
)]

use std::collections::BTreeSet;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

use crate::project::facts::Fact;
use crate::project::facts::Field;
use crate::project::facts::ModuleSearchPathEntry;
use crate::project::facts::Reason;
use crate::project::facts::ReasonSource;
use crate::project::input::resolve_django_settings;
use crate::project::module_resolver::module_name_for_file;
use crate::project::module_resolver::resolve_module;
use crate::project::names::PyModuleName;

const DEFAULT_DJANGO_ENVIRONMENT_ROOT: &str = ".";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DjangoEnvironmentResolution {
    pub(crate) root: Utf8PathBuf,
    pub(crate) django_settings_module: Fact<PyModuleName>,
    pub(crate) django_settings_file: Fact<Utf8PathBuf>,
}

/// Resolve Django environments from configuration.
///
/// Precedence is explicit `django_environments`, then discovered
/// `django_settings_file_patterns`, then the legacy/global
/// `django_settings_module` fallback.
#[must_use]
pub(crate) fn discover_django_environments(
    project_root: &Utf8Path,
    settings: &Settings,
    search_paths: &[ModuleSearchPathEntry],
) -> Vec<DjangoEnvironmentResolution> {
    if !settings.django_environments().is_empty() {
        return settings
            .django_environments()
            .iter()
            .map(|environment| {
                let root = normalize_environment_root(project_root, environment.root());
                if let Some(module) = environment.django_settings_module() {
                    environment_from_module(root, module, project_root, search_paths)
                } else {
                    invalid_environment(root, "Django environment must set django_settings_module")
                }
            })
            .collect();
    }

    if !settings.django_settings_file_patterns().is_empty() {
        return environments_from_settings_file_patterns(
            project_root,
            settings.django_settings_file_patterns(),
            search_paths,
        );
    }

    resolve_django_settings(project_root, settings)
        .map(|module| {
            environment_from_module(
                project_root.to_path_buf(),
                &module,
                project_root,
                search_paths,
            )
        })
        .into_iter()
        .collect()
}

fn environment_from_module(
    root: Utf8PathBuf,
    module: &str,
    project_root: &Utf8Path,
    search_paths: &[ModuleSearchPathEntry],
) -> DjangoEnvironmentResolution {
    let Ok(django_settings_module) = PyModuleName::parse(module) else {
        return invalid_environment(
            root,
            format!("django_settings_module is not a valid Python module path: {module}"),
        );
    };

    let resolution = resolve_module(django_settings_module.clone(), search_paths, project_root);
    DjangoEnvironmentResolution {
        root,
        django_settings_module: Fact::known(django_settings_module),
        django_settings_file: resolution.resolved.map(|resolved| resolved.file),
    }
}

fn environments_from_settings_file_patterns(
    project_root: &Utf8Path,
    patterns: &[String],
    search_paths: &[ModuleSearchPathEntry],
) -> Vec<DjangoEnvironmentResolution> {
    let mut builder = OverrideBuilder::new(project_root.as_std_path());
    for pattern in patterns {
        let pattern = pattern.trim();
        if let Err(error) = builder.add(pattern) {
            return vec![invalid_environment(
                project_root.to_path_buf(),
                format!("django_settings_file_patterns contains invalid glob `{pattern}`: {error}"),
            )];
        }
    }

    let matcher = match builder.build() {
        Ok(matcher) => matcher,
        Err(error) => {
            return vec![invalid_environment(
                project_root.to_path_buf(),
                format!("failed to build django_settings_file_patterns matcher: {error}"),
            )];
        }
    };

    let mut settings_files = BTreeSet::new();
    for result in WalkBuilder::new(project_root.as_std_path()).build() {
        let Ok(entry) = result else {
            continue;
        };
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path().to_path_buf()) else {
            continue;
        };
        if matcher.matched(path.as_std_path(), false).is_whitelist() {
            settings_files.insert(path);
        }
    }

    settings_files
        .into_iter()
        .map(|file| environment_from_settings_file(project_root, file, search_paths))
        .collect()
}

fn environment_from_settings_file(
    project_root: &Utf8Path,
    file: Utf8PathBuf,
    search_paths: &[ModuleSearchPathEntry],
) -> DjangoEnvironmentResolution {
    let root = infer_environment_root_from_settings_file(project_root, &file);
    let django_settings_module = module_name_for_file(&file, search_paths);

    DjangoEnvironmentResolution {
        root,
        django_settings_module,
        django_settings_file: Fact::known(file),
    }
}

fn infer_environment_root_from_settings_file(
    project_root: &Utf8Path,
    file: &Utf8Path,
) -> Utf8PathBuf {
    let Some(parent) = file.parent() else {
        return project_root.to_path_buf();
    };

    let mut current = parent;
    loop {
        if current == project_root {
            return project_root.to_path_buf();
        }
        if current.file_name() == Some("settings") {
            return current.parent().unwrap_or(project_root).to_path_buf();
        }
        let Some(next) = current.parent() else {
            return project_root.to_path_buf();
        };
        current = next;
    }
}

fn invalid_environment(
    root: Utf8PathBuf,
    message: impl Into<String>,
) -> DjangoEnvironmentResolution {
    let reason = Reason::new(
        Field::DjangoEnvironmentDiscovery,
        ReasonSource::DjangoEnvironmentRoot(root.clone()),
        message,
    );

    DjangoEnvironmentResolution {
        root,
        django_settings_module: Fact::unknown(vec![reason.clone()]),
        django_settings_file: Fact::unknown(vec![reason]),
    }
}

#[must_use]
fn normalize_environment_root(project_root: &Utf8Path, root: &str) -> Utf8PathBuf {
    let root = Utf8Path::new(root);
    if root.is_absolute() {
        root.to_path_buf()
    } else if root.as_str() == DEFAULT_DJANGO_ENVIRONMENT_ROOT {
        project_root.to_path_buf()
    } else {
        project_root.join(root)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::project::facts::ModuleSearchPathKind;
    use crate::project::module_resolver::discover_module_search_paths;

    fn settings(root: &Utf8Path) -> Settings {
        Settings::new(root, None).unwrap()
    }

    fn search_paths(root: &Utf8Path) -> Vec<ModuleSearchPathEntry> {
        discover_module_search_paths(root, &[], &[])
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

    fn known_module(fact: &Fact<PyModuleName>) -> PyModuleName {
        let Fact::Known { value } = fact else {
            panic!("expected known Django settings module, got {fact:?}");
        };
        value.clone()
    }

    fn known_file(fact: &Fact<Utf8PathBuf>) -> Utf8PathBuf {
        let Fact::Known { value } = fact else {
            panic!("expected known Django settings file, got {fact:?}");
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
            discover_django_environments(&root, &settings(&root), &search_paths(&root));

        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].root, root);
        assert_eq!(
            known_module(&environments[0].django_settings_module),
            module("project.settings")
        );
        assert_eq!(
            known_file(&environments[0].django_settings_file),
            root.join("project/settings.py")
        );
    }

    #[test]
    fn django_settings_file_patterns_keep_flat_settings_at_workspace_root() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_file_patterns = ["project/settings.py"]"#,
        );

        let environments =
            discover_django_environments(&root, &settings(&root), &search_paths(&root));

        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].root, root);
        assert_eq!(
            known_module(&environments[0].django_settings_module),
            module("project.settings")
        );
        assert_eq!(
            known_file(&environments[0].django_settings_file),
            environments[0].root.join("project/settings.py")
        );
    }

    #[test]
    fn settings_file_patterns_without_settings_package_fall_back_to_workspace_root() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/conf.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_file_patterns = ["project/conf.py"]"#,
        );

        let environments =
            discover_django_environments(&root, &settings(&root), &search_paths(&root));

        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].root, root);
        assert_eq!(
            known_module(&environments[0].django_settings_module),
            module("project.conf")
        );
    }

    #[test]
    fn invalid_explicit_django_environment_module_is_unknown() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(
            &root.join("djls.toml"),
            r#"
[[django_environments]]
root = "."
django_settings_module = "project..settings"
"#,
        );

        let environments =
            discover_django_environments(&root, &settings(&root), &search_paths(&root));

        assert_eq!(environments.len(), 1);
        assert!(matches!(
            &environments[0].django_settings_module,
            Fact::Unknown { .. }
        ));
        assert!(matches!(
            &environments[0].django_settings_file,
            Fact::Unknown { .. }
        ));
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
            discover_django_environments(&root, &settings(&root), &search_paths(&root));

        assert_eq!(environments.len(), 2);
        assert_eq!(environments[0].root, root.join("projects/site1"));
        assert_eq!(
            known_module(&environments[0].django_settings_module),
            module("projects.site1.settings")
        );
        assert_eq!(
            known_file(&environments[0].django_settings_file),
            root.join("projects/site1/settings.py")
        );
        assert_eq!(environments[1].root, root.join("projects/site2"));
        assert_eq!(
            known_module(&environments[1].django_settings_module),
            module("projects.site2.settings")
        );
        assert_eq!(
            known_file(&environments[1].django_settings_file),
            root.join("projects/site2/settings.py")
        );
    }

    #[test]
    fn django_settings_file_patterns_discover_split_settings_environments() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("projects/__init__.py"), "");
        write_file(&root.join("projects/site1/__init__.py"), "");
        write_file(&root.join("projects/site1/settings/__init__.py"), "");
        write_file(&root.join("projects/site1/settings/dev.py"), "");
        write_file(&root.join("projects/site1/settings/production.py"), "");
        write_file(&root.join("projects/site2/__init__.py"), "");
        write_file(&root.join("projects/site2/settings/__init__.py"), "");
        write_file(&root.join("projects/site2/settings/dev.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_file_patterns = ["projects/*/settings/dev.py"]"#,
        );

        let environments =
            discover_django_environments(&root, &settings(&root), &search_paths(&root));

        assert_eq!(environments.len(), 2);
        assert_eq!(environments[0].root, root.join("projects/site1"));
        assert_eq!(
            known_module(&environments[0].django_settings_module),
            module("projects.site1.settings.dev")
        );
        assert_eq!(
            known_file(&environments[0].django_settings_file),
            root.join("projects/site1/settings/dev.py")
        );
        assert_eq!(environments[1].root, root.join("projects/site2"));
        assert_eq!(
            known_module(&environments[1].django_settings_module),
            module("projects.site2.settings.dev")
        );
        assert_eq!(
            known_file(&environments[1].django_settings_file),
            root.join("projects/site2/settings/dev.py")
        );
    }

    #[test]
    fn django_settings_file_patterns_map_files_through_auto_src_search_path() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("src/project/__init__.py"), "");
        write_file(&root.join("src/project/settings/__init__.py"), "");
        write_file(&root.join("src/project/settings/dev.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_file_patterns = ["src/*/settings/dev.py"]"#,
        );

        let roots = search_paths(&root);
        assert!(roots
            .iter()
            .any(|root| root.kind == ModuleSearchPathKind::AutoSrc));

        let environments = discover_django_environments(&root, &settings(&root), &roots);

        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].root, root.join("src/project"));
        assert_eq!(
            known_module(&environments[0].django_settings_module),
            module("project.settings.dev")
        );
        assert_eq!(
            known_file(&environments[0].django_settings_file),
            root.join("src/project/settings/dev.py")
        );
    }
}
