//! Settings context discovery for the static Django project model.
//!
//! This module is intentionally not wired into project validation yet. It turns
//! legacy `django_settings_module` and the new settings-context config shape
//! into context-scoped facts that later static settings extraction can consume.

#![allow(
    dead_code,
    reason = "Milestone A3 adds settings context discovery before wiring static settings extraction."
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SettingsContextResolution {
    pub(crate) label: String,
    pub(crate) settings_module: Fact<PyModuleName>,
    pub(crate) settings_file: Fact<Utf8PathBuf>,
}

#[must_use]
pub(crate) fn discover_settings_contexts(
    project_root: &Utf8Path,
    settings: &Settings,
    import_roots: &[ImportRoot],
) -> Vec<SettingsContextResolution> {
    if !settings.settings_contexts().is_empty() {
        return settings
            .settings_contexts()
            .iter()
            .map(|context| {
                if let Some(module) = context.module() {
                    context_from_module(context.label(), module, project_root, import_roots)
                } else if let Some(file) = context.file() {
                    context_from_file(context.label(), file, project_root, import_roots)
                } else {
                    invalid_context(context.label(), "settings context must set module or file")
                }
            })
            .collect();
    }

    resolve_django_settings(project_root, settings)
        .map(|module| context_from_module("default", &module, project_root, import_roots))
        .into_iter()
        .collect()
}

fn context_from_module(
    label: &str,
    module: &str,
    project_root: &Utf8Path,
    import_roots: &[ImportRoot],
) -> SettingsContextResolution {
    let Ok(settings_module) = PyModuleName::parse(module) else {
        return invalid_context(
            label,
            format!("settings context module is not a valid Python module path: {module}"),
        );
    };

    let resolution = resolve_module(settings_module.clone(), import_roots, project_root);
    SettingsContextResolution {
        label: label.to_string(),
        settings_module: Fact::known(settings_module),
        settings_file: resolution.resolved.map(|resolved| resolved.file),
    }
}

fn context_from_file(
    label: &str,
    file: &str,
    project_root: &Utf8Path,
    import_roots: &[ImportRoot],
) -> SettingsContextResolution {
    let file = normalize_context_file(project_root, file);
    let settings_file = if file.is_file() {
        Fact::known(file.clone())
    } else {
        Fact::unknown(vec![Reason::file(
            Field::SettingsContext,
            file.clone(),
            "settings context file does not exist or is not a file",
        )])
    };
    let settings_module = resolve_file_module(&file, import_roots);

    SettingsContextResolution {
        label: label.to_string(),
        settings_module,
        settings_file,
    }
}

fn invalid_context(label: &str, message: impl Into<String>) -> SettingsContextResolution {
    let reason = Reason::new(
        Field::SettingsContext,
        ReasonSource::SettingsContext(label.to_string()),
        message,
    );

    SettingsContextResolution {
        label: label.to_string(),
        settings_module: Fact::unknown(vec![reason.clone()]),
        settings_file: Fact::unknown(vec![reason]),
    }
}

#[must_use]
fn normalize_context_file(project_root: &Utf8Path, file: &str) -> Utf8PathBuf {
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
            panic!("expected known settings module, got {fact:?}");
        };
        value.clone()
    }

    fn known_file(fact: &Fact<Utf8PathBuf>) -> Utf8PathBuf {
        let Fact::Known { value } = fact else {
            panic!("expected known settings file, got {fact:?}");
        };
        value.clone()
    }

    #[test]
    fn legacy_django_settings_module_becomes_default_context() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(&root.join("project/settings.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_module = "project.settings""#,
        );

        let contexts = discover_settings_contexts(&root, &settings(&root), &import_roots(&root));

        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].label, "default");
        assert_eq!(
            known_module(&contexts[0].settings_module),
            module("project.settings")
        );
        assert_eq!(
            known_file(&contexts[0].settings_file),
            root.join("project/settings.py")
        );
    }

    #[test]
    fn explicit_module_contexts_remain_separate() {
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
[[settings_contexts]]
label = "site1"
module = "projects.site1.settings"

[[settings_contexts]]
label = "site2"
module = "projects.site2.settings"
"#,
        );

        let contexts = discover_settings_contexts(&root, &settings(&root), &import_roots(&root));

        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].label, "site1");
        assert_eq!(
            known_module(&contexts[0].settings_module),
            module("projects.site1.settings")
        );
        assert_eq!(
            known_file(&contexts[0].settings_file),
            root.join("projects/site1/settings.py")
        );
        assert_eq!(contexts[1].label, "site2");
        assert_eq!(
            known_module(&contexts[1].settings_module),
            module("projects.site2.settings")
        );
        assert_eq!(
            known_file(&contexts[1].settings_file),
            root.join("projects/site2/settings.py")
        );
    }

    #[test]
    fn file_context_resolves_module_through_import_roots() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("src/project/__init__.py"), "");
        write_file(&root.join("src/project/settings/__init__.py"), "");
        write_file(&root.join("src/project/settings/dev.py"), "");
        write_file(
            &root.join("djls.toml"),
            r#"
[[settings_contexts]]
label = "dev"
file = "src/project/settings/dev.py"
"#,
        );

        let roots = import_roots(&root);
        assert!(roots
            .iter()
            .any(|root| root.kind == ImportRootKind::AutoSrc));

        let contexts = discover_settings_contexts(&root, &settings(&root), &roots);

        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].label, "dev");
        assert_eq!(
            known_module(&contexts[0].settings_module),
            module("project.settings.dev")
        );
        assert_eq!(
            known_file(&contexts[0].settings_file),
            root.join("src/project/settings/dev.py")
        );
    }

    #[test]
    fn file_context_outside_import_roots_keeps_file_but_unknown_module() {
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
[[settings_contexts]]
label = "external"
file = "{settings_file}"
"#,
            ),
        );

        let contexts = discover_settings_contexts(&root, &settings(&root), &import_roots(&root));

        assert_eq!(known_file(&contexts[0].settings_file), settings_file);
        match &contexts[0].settings_module {
            Fact::Unknown { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.field == Field::ResolverModule));
            }
            other => panic!("expected unknown module for file outside import roots, got {other:?}"),
        }
    }
}
