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

use std::collections::BTreeSet;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

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
            import_roots,
        );
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

fn environments_from_settings_file_patterns(
    project_root: &Utf8Path,
    patterns: &[String],
    import_roots: &[ImportRoot],
) -> Vec<ResolvedDjangoEnvironment> {
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

        let Some(path) = Utf8PathBuf::from_path_buf(entry.path().to_path_buf()).ok() else {
            continue;
        };
        if matcher.matched(path.as_std_path(), false).is_whitelist() {
            settings_files.insert(path);
        }
    }

    settings_files
        .into_iter()
        .map(|file| environment_from_settings_file(file, import_roots))
        .collect()
}

fn environment_from_settings_file(
    file: Utf8PathBuf,
    import_roots: &[ImportRoot],
) -> ResolvedDjangoEnvironment {
    let root = infer_environment_root_from_settings_file(&file);
    let django_settings_module = resolve_file_module(&file, import_roots);

    ResolvedDjangoEnvironment {
        root,
        django_settings_module,
        django_settings_file: Fact::known(file),
    }
}

fn infer_environment_root_from_settings_file(file: &Utf8Path) -> Utf8PathBuf {
    let Some(parent) = file.parent() else {
        return file.to_path_buf();
    };

    let mut current = parent;
    loop {
        if current.file_name() == Some("settings") {
            return current.parent().unwrap_or(current).to_path_buf();
        }
        let Some(next) = current.parent() else {
            return parent.to_path_buf();
        };
        current = next;
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
    fn explicit_django_environments_take_precedence_over_settings_file_patterns() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings.py"), "");
        write_file(&root.join("discovered/__init__.py"), "");
        write_file(&root.join("discovered/settings.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"
django_settings_file_patterns = ["discovered/settings.py"]

[[django_environments]]
root = "project"
django_settings_module = "project.settings"
"#,
        );

        let environments =
            discover_django_environments(&root, &settings(&root), &import_roots(&root));

        assert_eq!(environments.len(), 1);
        assert_eq!(environments[0].root, root.join("project"));
        assert_eq!(
            known_module(&environments[0].django_settings_module),
            module("project.settings")
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
            discover_django_environments(&root, &settings(&root), &import_roots(&root));

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
    fn django_settings_file_patterns_map_files_through_auto_src_import_root() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("src/project/__init__.py"), "");
        write_file(&root.join("src/project/settings/__init__.py"), "");
        write_file(&root.join("src/project/settings/dev.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_file_patterns = ["src/*/settings/dev.py"]"#,
        );

        let roots = import_roots(&root);
        assert!(roots
            .iter()
            .any(|root| root.kind == ImportRootKind::AutoSrc));

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
