//! Source-derived Django template directory facts.
//!
//! This module assembles Django template search directories from extracted
//! settings facts and static app registry facts. It follows Django's loader
//! order for the standard Django template backend: configured `DIRS` first,
//! then existing `<app>/templates` directories when `APP_DIRS` is true.
//! Exact duplicate app-template facts are emitted once at their first flattened
//! backend occurrence because these facts do not carry backend identity yet.

#![allow(
    dead_code,
    reason = "Milestone A7 adds template directory facts before project facts are assembled."
)]

use crate::project::facts::AppFact;
use crate::project::facts::Fact;
use crate::project::facts::Reason;
use crate::project::facts::ReasonSource;
use crate::project::facts::TemplateBackendFact;
use crate::project::facts::TemplateDirFact;
use crate::project::facts::TemplateDirSource;

const DJANGO_TEMPLATES_BACKEND: &str = "django.template.backends.django.DjangoTemplates";

#[must_use]
pub(crate) fn assemble_template_dirs(
    template_backends: &Fact<Vec<TemplateBackendFact>>,
    app_registry: &Fact<Vec<AppFact>>,
) -> Fact<Vec<TemplateDirFact>> {
    match template_backends {
        Fact::Known { value } => assemble_template_backend_dirs(value, Vec::new(), app_registry),
        Fact::Partial { value, reasons } => {
            assemble_template_backend_dirs(value, reasons.clone(), app_registry)
        }
        Fact::Unknown { reasons } | Fact::Ambiguous { reasons, .. } => {
            Fact::unknown(reasons.clone())
        }
    }
}

fn assemble_template_backend_dirs(
    backends: &[TemplateBackendFact],
    mut reasons: Vec<Reason>,
    app_registry: &Fact<Vec<AppFact>>,
) -> Fact<Vec<TemplateDirFact>> {
    let mut dirs = Vec::new();

    for backend in backends {
        if !is_django_template_backend(backend, &mut reasons) {
            continue;
        }

        append_backend_dirs(&mut dirs, &mut reasons, &backend.dirs);
        append_app_dirs(&mut dirs, &mut reasons, &backend.app_dirs, app_registry);
    }

    known_or_partial(dirs, reasons)
}

fn is_django_template_backend(backend: &TemplateBackendFact, reasons: &mut Vec<Reason>) -> bool {
    let Some(backend) = backend.backend.as_deref() else {
        reasons.push(Reason::new(
            ReasonSource::Unknown,
            "TEMPLATES BACKEND is not known; skipped template directory assembly for this backend",
        ));
        return false;
    };

    backend == DJANGO_TEMPLATES_BACKEND
}

fn append_backend_dirs(
    dirs: &mut Vec<TemplateDirFact>,
    reasons: &mut Vec<Reason>,
    backend_dirs: &Fact<Vec<TemplateDirFact>>,
) {
    match backend_dirs {
        Fact::Known { value } => dirs.extend(value.iter().cloned()),
        Fact::Partial {
            value,
            reasons: dir_reasons,
        } => {
            dirs.extend(value.iter().cloned());
            extend_unique_reasons(reasons, dir_reasons.iter().cloned());
        }
        Fact::Unknown {
            reasons: dir_reasons,
        }
        | Fact::Ambiguous {
            reasons: dir_reasons,
            ..
        } => {
            extend_unique_reasons(reasons, dir_reasons.iter().cloned());
        }
    }
}

fn append_app_dirs(
    dirs: &mut Vec<TemplateDirFact>,
    reasons: &mut Vec<Reason>,
    app_dirs: &Fact<bool>,
    app_registry: &Fact<Vec<AppFact>>,
) {
    match app_dirs {
        Fact::Known { value: true } => append_app_template_dirs(dirs, reasons, app_registry),
        Fact::Partial {
            value: true,
            reasons: app_dir_reasons,
        } => {
            extend_unique_reasons(reasons, app_dir_reasons.iter().cloned());
            append_app_template_dirs(dirs, reasons, app_registry);
        }
        Fact::Known { value: false } => {}
        Fact::Partial {
            value: false,
            reasons: app_dir_reasons,
        }
        | Fact::Unknown {
            reasons: app_dir_reasons,
        }
        | Fact::Ambiguous {
            reasons: app_dir_reasons,
            ..
        } => {
            extend_unique_reasons(reasons, app_dir_reasons.iter().cloned());
        }
    }
}

fn append_app_template_dirs(
    dirs: &mut Vec<TemplateDirFact>,
    reasons: &mut Vec<Reason>,
    app_registry: &Fact<Vec<AppFact>>,
) {
    match app_registry {
        Fact::Known { value } => push_existing_app_template_dirs(dirs, value),
        Fact::Partial {
            value,
            reasons: app_reasons,
        } => {
            push_existing_app_template_dirs(dirs, value);
            extend_unique_reasons(reasons, app_reasons.iter().cloned());
        }
        Fact::Unknown {
            reasons: app_reasons,
        }
        | Fact::Ambiguous {
            reasons: app_reasons,
            ..
        } => {
            extend_unique_reasons(reasons, app_reasons.iter().cloned());
        }
    }
}

fn push_existing_app_template_dirs(dirs: &mut Vec<TemplateDirFact>, apps: &[AppFact]) {
    for app in apps {
        let path = app.path.join("templates");
        // Django's get_app_template_dirs() only returns app template directories
        // that exist. Keep that filter here so source-derived facts match runtime dirs.
        if path.is_dir() {
            push_unique_app_template_dir(
                dirs,
                TemplateDirFact {
                    path,
                    source: TemplateDirSource::AppDir {
                        app: app.module.clone(),
                    },
                },
            );
        }
    }
}

fn push_unique_app_template_dir(dirs: &mut Vec<TemplateDirFact>, dir: TemplateDirFact) {
    if !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

fn known_or_partial(
    value: Vec<TemplateDirFact>,
    reasons: Vec<Reason>,
) -> Fact<Vec<TemplateDirFact>> {
    if reasons.is_empty() {
        Fact::known(value)
    } else {
        Fact::partial(value, reasons)
    }
}

fn extend_unique_reasons(reasons: &mut Vec<Reason>, new_reasons: impl Iterator<Item = Reason>) {
    for reason in new_reasons {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    use super::*;
    use crate::project::app_registry::resolve_app_registry;
    use crate::project::facts::TemplateDirSource;
    use crate::project::module_resolver::discover_module_search_paths;
    use crate::project::names::PyModuleName;
    use crate::project::settings_facts::extract_settings_facts;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn mkdir(path: &Utf8Path) {
        std::fs::create_dir_all(path).unwrap();
    }

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            mkdir(parent);
        }
        std::fs::write(path, contents).unwrap();
    }

    fn search_paths(root: &Utf8Path) -> Vec<crate::project::facts::ModuleSearchPathEntry> {
        discover_module_search_paths(root, &[], &[])
            .value()
            .unwrap()
            .clone()
    }

    fn settings_dir(path: Utf8PathBuf) -> TemplateDirFact {
        TemplateDirFact {
            path,
            source: TemplateDirSource::SettingsDir,
        }
    }

    fn backend(dirs: Fact<Vec<TemplateDirFact>>, app_dirs: Fact<bool>) -> TemplateBackendFact {
        TemplateBackendFact {
            backend: Some(DJANGO_TEMPLATES_BACKEND.to_string()),
            dirs,
            app_dirs,
            option_libraries: Fact::known(Vec::new()),
            option_builtins: Fact::known(Vec::new()),
        }
    }

    fn app(root: &Utf8Path, module_name: &str) -> AppFact {
        AppFact {
            entry: module_name.to_string(),
            module: module(module_name),
            path: root.join(module_name.replace('.', "/")),
            config: None,
        }
    }

    fn known_vec<T: Clone + std::fmt::Debug>(fact: &Fact<Vec<T>>) -> Vec<T> {
        let Fact::Known { value } = fact else {
            panic!("expected known fact, got {fact:?}");
        };
        value.clone()
    }

    fn partial_vec<T: Clone + std::fmt::Debug>(fact: &Fact<Vec<T>>) -> (Vec<T>, Vec<Reason>) {
        let Fact::Partial { value, reasons } = fact else {
            panic!("expected partial fact, got {fact:?}");
        };
        (value.clone(), reasons.clone())
    }

    fn unknown_reasons<T: std::fmt::Debug>(fact: &Fact<T>) -> Vec<Reason> {
        let Fact::Unknown { reasons } = fact else {
            panic!("expected unknown fact, got {fact:?}");
        };
        reasons.clone()
    }

    fn app_registry_reason() -> Reason {
        Reason::new(
            ReasonSource::Unknown,
            "some installed apps could not be resolved",
        )
    }

    fn dirs_reason() -> Reason {
        Reason::path(
            "project/settings.py",
            "TEMPLATES DIRS contains an unsupported path expression",
        )
    }

    fn app_dirs_reason() -> Reason {
        Reason::new(
            ReasonSource::Unknown,
            "TEMPLATES APP_DIRS must be a boolean literal",
        )
    }

    #[test]
    fn assembles_settings_dirs_before_app_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        mkdir(&root.join("blog/templates"));
        mkdir(&root.join("shop/templates"));

        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(
                Fact::known(vec![
                    settings_dir(root.join("templates")),
                    settings_dir(root.join("more_templates")),
                ]),
                Fact::known(true),
            )]),
            &Fact::known(vec![app(&root, "blog"), app(&root, "shop")]),
        );

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [
                root.join("templates"),
                root.join("more_templates"),
                root.join("blog/templates"),
                root.join("shop/templates"),
            ]
        );
    }

    #[test]
    fn skips_missing_app_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        mkdir(&root.join("blog/templates"));
        mkdir(&root.join("shop"));

        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(Fact::known(Vec::new()), Fact::known(true))]),
            &Fact::known(vec![app(&root, "blog"), app(&root, "shop")]),
        );

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("blog/templates")]
        );
    }

    #[test]
    fn app_dirs_false_keeps_only_settings_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        mkdir(&root.join("blog/templates"));

        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(
                Fact::known(vec![settings_dir(root.join("templates"))]),
                Fact::known(false),
            )]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("templates")]
        );
    }

    #[test]
    fn app_dirs_unknown_keeps_settings_dirs_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        mkdir(&root.join("blog/templates"));

        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(
                Fact::known(vec![settings_dir(root.join("templates"))]),
                Fact::unknown(vec![app_dirs_reason()]),
            )]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        let (dirs, reasons) = partial_vec(&facts);
        assert_eq!(
            dirs.into_iter().map(|dir| dir.path).collect::<Vec<_>>(),
            [root.join("templates")]
        );
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("APP_DIRS")));
    }

    #[test]
    fn partial_app_registry_keeps_known_app_dirs_and_reasons() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        mkdir(&root.join("blog/templates"));

        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(Fact::known(Vec::new()), Fact::known(true))]),
            &Fact::partial(vec![app(&root, "blog")], vec![app_registry_reason()]),
        );

        let (dirs, reasons) = partial_vec(&facts);
        assert_eq!(
            dirs.into_iter().map(|dir| dir.path).collect::<Vec<_>>(),
            [root.join("blog/templates")]
        );
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("could not be resolved")));
    }

    #[test]
    fn unknown_app_registry_keeps_app_dirs_partial() {
        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(Fact::known(Vec::new()), Fact::known(true))]),
            &Fact::unknown(vec![app_registry_reason()]),
        );

        let (dirs, reasons) = partial_vec(&facts);
        assert!(dirs.is_empty());
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("could not be resolved")));
    }

    #[test]
    fn partial_settings_dirs_preserve_reasons() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let reason = dirs_reason();

        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(
                Fact::partial(
                    vec![settings_dir(root.join("templates"))],
                    vec![reason.clone()],
                ),
                Fact::known(false),
            )]),
            &Fact::known(Vec::new()),
        );

        let (dirs, reasons) = partial_vec(&facts);
        assert_eq!(
            dirs.into_iter().map(|dir| dir.path).collect::<Vec<_>>(),
            [root.join("templates")]
        );
        assert_eq!(reasons, [reason]);
    }

    #[test]
    fn unknown_template_backends_are_unknown_template_dirs() {
        let reason = Reason::new(
            ReasonSource::Unknown,
            "TEMPLATES is not assigned in this settings file",
        );

        let facts = assemble_template_dirs(
            &Fact::unknown(vec![reason.clone()]),
            &Fact::known(Vec::new()),
        );

        assert_eq!(unknown_reasons(&facts), [reason]);
    }

    #[test]
    fn empty_template_backend_list_is_known_empty() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        mkdir(&root.join("blog/templates"));

        let facts = assemble_template_dirs(
            &Fact::known(Vec::new()),
            &Fact::known(vec![app(&root, "blog")]),
        );

        assert!(known_vec(&facts).is_empty());
    }

    #[test]
    fn skips_non_django_backends() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let mut jinja_backend = backend(
            Fact::known(vec![settings_dir(root.join("jinja_templates"))]),
            Fact::known(false),
        );
        jinja_backend.backend = Some("django.template.backends.jinja2.Jinja2".to_string());

        let facts =
            assemble_template_dirs(&Fact::known(vec![jinja_backend]), &Fact::known(Vec::new()));

        assert!(known_vec(&facts).is_empty());
    }

    #[test]
    fn non_django_backend_does_not_block_django_backend_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let mut jinja_backend = backend(
            Fact::known(vec![settings_dir(root.join("jinja_templates"))]),
            Fact::known(false),
        );
        jinja_backend.backend = Some("django.template.backends.jinja2.Jinja2".to_string());

        let facts = assemble_template_dirs(
            &Fact::known(vec![
                jinja_backend,
                backend(
                    Fact::known(vec![settings_dir(root.join("django_templates"))]),
                    Fact::known(false),
                ),
            ]),
            &Fact::known(Vec::new()),
        );

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("django_templates")]
        );
    }

    #[test]
    fn missing_backend_skips_backend_as_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let mut unknown_backend = backend(
            Fact::known(vec![settings_dir(root.join("templates"))]),
            Fact::known(true),
        );
        unknown_backend.backend = None;

        let facts = assemble_template_dirs(
            &Fact::known(vec![unknown_backend]),
            &Fact::known(Vec::new()),
        );

        let (dirs, reasons) = partial_vec(&facts);
        assert!(dirs.is_empty());
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("BACKEND is not known")));
    }

    #[test]
    fn assembles_dirs_from_extracted_settings_and_resolved_apps() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        mkdir(&root.join("blog/templates"));
        write_file(
            &root.join("settings.py"),
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent
INSTALLED_APPS = ["blog"]
TEMPLATES = [{
    "BACKEND": "django.template.backends.django.DjangoTemplates",
    "DIRS": [BASE_DIR / "templates"],
    "APP_DIRS": True,
}]
"#,
        );

        let settings = extract_settings_facts(&root.join("settings.py"));
        let app_registry =
            resolve_app_registry(&settings.installed_apps, &root, &search_paths(&root));
        let facts = assemble_template_dirs(&settings.template_backends, &app_registry.app_registry);

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("templates"), root.join("blog/templates")]
        );
    }

    #[test]
    fn uses_app_config_path_from_resolved_app_registry() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        write_file(
            &root.join("blog/apps.py"),
            r#"
from django.apps import AppConfig

class BlogConfig(AppConfig):
    name = "blog"
    path = "custom_blog"
"#,
        );
        mkdir(&root.join("custom_blog/templates"));

        let app_registry = resolve_app_registry(
            &Fact::known(vec!["blog.apps.BlogConfig".to_string()]),
            &root,
            &search_paths(&root),
        );
        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(Fact::known(Vec::new()), Fact::known(true))]),
            &app_registry.app_registry,
        );

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("custom_blog/templates")]
        );
    }

    #[test]
    fn uses_source_root_app_paths_from_resolved_app_registry() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("src/blog/__init__.py"), "");
        mkdir(&root.join("src/blog/templates"));

        let app_registry = resolve_app_registry(
            &Fact::known(vec!["blog".to_string()]),
            &root,
            &search_paths(&root),
        );
        let facts = assemble_template_dirs(
            &Fact::known(vec![backend(Fact::known(Vec::new()), Fact::known(true))]),
            &app_registry.app_registry,
        );

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("src/blog/templates")]
        );
    }

    #[test]
    fn multiple_app_dirs_backends_deduplicate_app_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        mkdir(&root.join("blog/templates"));

        let facts = assemble_template_dirs(
            &Fact::known(vec![
                backend(
                    Fact::known(vec![settings_dir(root.join("first"))]),
                    Fact::known(true),
                ),
                backend(
                    Fact::known(vec![settings_dir(root.join("second"))]),
                    Fact::known(true),
                ),
            ]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [
                root.join("first"),
                root.join("blog/templates"),
                root.join("second"),
            ]
        );
    }

    #[test]
    fn preserves_backend_order_across_multiple_engines() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        mkdir(&root.join("blog/templates"));

        let facts = assemble_template_dirs(
            &Fact::known(vec![
                backend(
                    Fact::known(vec![settings_dir(root.join("first"))]),
                    Fact::known(false),
                ),
                backend(
                    Fact::known(vec![settings_dir(root.join("second"))]),
                    Fact::known(true),
                ),
            ]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        assert_eq!(
            known_vec(&facts)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [
                root.join("first"),
                root.join("second"),
                root.join("blog/templates")
            ]
        );
    }
}
