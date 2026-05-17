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
use djls_conf::Settings;

use crate::project::input::resolve_django_settings;
use crate::project::names::PyModuleName;
use crate::project::static_model::Fact;
use crate::project::static_model::Field;
use crate::project::static_model::ImportRoot;
use crate::project::static_model::Reason;
use crate::project::static_model::ReasonSource;
use crate::project::static_resolver::resolve_file_module;
use crate::project::static_resolver::resolve_module;

const DEFAULT_DJANGO_ENVIRONMENT_ROOT: &str = ".";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedDjangoEnvironment {
    pub(crate) root: Utf8PathBuf,
    pub(crate) django_settings_module: Fact<PyModuleName>,
    pub(crate) django_settings_file: Fact<Utf8PathBuf>,
}

#[must_use]
pub(crate) fn discover_django_environments(
    project_root: &Utf8Path,
    settings: &Settings,
    import_roots: &[ImportRoot],
) -> Vec<ResolvedDjangoEnvironment> {
    if !settings.django_environments().is_empty() {
        return settings
            .django_environments()
            .iter()
            .map(|environment| {
                let root = normalize_environment_root(project_root, environment.root());
                if let Some(module) = environment.django_settings_module() {
                    environment_from_module(root, module, project_root, import_roots)
                } else if let Some(file) = environment.django_settings_file() {
                    environment_from_file(root, file, project_root, import_roots)
                } else {
                    invalid_environment(root, "Django environment must set django_settings_module or django_settings_file")
                }
            })
            .collect();
    }

    resolve_django_settings(project_root, settings)
        .map(|module| {
            environment_from_module(
                project_root.to_path_buf(),
                &module,
                project_root,
                import_roots,
            )
        })
        .into_iter()
        .collect()
}

fn environment_from_module(
    root: Utf8PathBuf,
    module: &str,
    project_root: &Utf8Path,
    import_roots: &[ImportRoot],
) -> ResolvedDjangoEnvironment {
    let Ok(django_settings_module) = PyModuleName::parse(module) else {
        return invalid_environment(
            root,
            format!("django_settings_module is not a valid Python module path: {module}"),
        );
    };

    let resolution = resolve_module(django_settings_module.clone(), import_roots, project_root);
    ResolvedDjangoEnvironment {
        root,
        django_settings_module: Fact::known(django_settings_module),
        django_settings_file: resolution.resolved.map(|resolved| resolved.file),
    }
}

fn environment_from_file(
    root: Utf8PathBuf,
    file: &str,
    project_root: &Utf8Path,
    import_roots: &[ImportRoot],
) -> ResolvedDjangoEnvironment {
    let file = normalize_settings_file(project_root, file);
    let django_settings_file = if file.is_file() {
        Fact::known(file.clone())
    } else {
        Fact::unknown(vec![Reason::file(
            Field::DjangoEnvironment,
            file.clone(),
            "django_settings_file does not exist or is not a file",
        )])
    };
    let django_settings_module = resolve_file_module(&file, import_roots);

    ResolvedDjangoEnvironment {
        root,
        django_settings_module,
        django_settings_file,
    }
}

fn invalid_environment(root: Utf8PathBuf, message: impl Into<String>) -> ResolvedDjangoEnvironment {
    let reason = Reason::new(
        Field::DjangoEnvironment,
        ReasonSource::DjangoEnvironment(root.clone()),
        message,
    );

    ResolvedDjangoEnvironment {
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

#[must_use]
fn normalize_settings_file(project_root: &Utf8Path, file: &str) -> Utf8PathBuf {
    let file = Utf8Path::new(file);
    if file.is_absolute() {
        file.to_path_buf()
    } else {
        project_root.join(file)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::project::static_model::ImportRootKind;
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
            discover_django_environments(&root, &settings(&root), &import_roots(&root));

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
    fn django_settings_file_resolves_module_through_import_roots() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("src/project/__init__.py"), "");
        write_file(&root.join("src/project/settings/__init__.py"), "");
        write_file(&root.join("src/project/settings/dev.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"
[[django_environments]]
root = "."
django_settings_file = "src/project/settings/dev.py"
"#,
        );

        let roots = import_roots(&root);
        assert!(roots
            .iter()
            .any(|root| root.kind == ImportRootKind::AutoSrc));

        let environments = discover_django_environments(&root, &settings(&root), &roots);

        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].root, root);
        assert_eq!(
            known_module(&environments[0].django_settings_module),
            module("project.settings.dev")
        );
        assert_eq!(
            known_file(&environments[0].django_settings_file),
            root.join("src/project/settings/dev.py")
        );
    }

    #[test]
    fn django_settings_file_outside_import_roots_keeps_file_but_unknown_module() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let external_tmp = tempdir().unwrap();
        let external_root = Utf8PathBuf::try_from(external_tmp.path().to_path_buf()).unwrap();
        let settings_file = external_root.join("project/settings.py");
        write_file(&settings_file, "");
        write_file(
            &root.join("djls.toml"),
            &format!(
                r#"
[[django_environments]]
root = "."
django_settings_file = "{settings_file}"
"#,
            ),
        );

        let environments =
            discover_django_environments(&root, &settings(&root), &import_roots(&root));

        assert_eq!(
            known_file(&environments[0].django_settings_file),
            settings_file
        );
        match &environments[0].django_settings_module {
            Fact::Unknown { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.field == Field::ResolverModule));
            }
            other => panic!("expected unknown module for file outside import roots, got {other:?}"),
        }
    }
}
