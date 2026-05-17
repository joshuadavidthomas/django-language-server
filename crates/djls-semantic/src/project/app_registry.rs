//! Static Django app registry facts.
//!
//! This module resolves `INSTALLED_APPS` entries into app package facts without
//! importing Django or project code. It supports direct app package entries and
//! simple `AppConfig` classes with literal `name`, `label`, and `path`
//! assignments.

#![allow(
    dead_code,
    reason = "Milestone A6 adds app registry facts before project facts are assembled."
)]

use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtClassDef;

use crate::project::facts::AppConfigFact;
use crate::project::facts::AppFact;
use crate::project::facts::Fact;
use crate::project::facts::Field;
use crate::project::facts::InstalledAppFact;
use crate::project::facts::ModuleResolution;
use crate::project::facts::ModuleSearchPathEntry;
use crate::project::facts::Reason;
use crate::project::facts::ReasonSource;
use crate::project::facts::ResolvedModule;
use crate::project::module_resolver::resolve_module;
use crate::project::names::PyModuleName;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppRegistryFacts {
    pub(crate) installed_apps: Fact<Vec<InstalledAppFact>>,
    pub(crate) app_registry: Fact<Vec<AppFact>>,
}

struct AppConfigAssignments<'a> {
    name: Option<&'a Expr>,
    label: Option<&'a Expr>,
    path: Option<&'a Expr>,
}

#[must_use]
pub(crate) fn resolve_app_registry(
    installed_apps: &Fact<Vec<String>>,
    project_root: &Utf8Path,
    search_paths: &[ModuleSearchPathEntry],
) -> AppRegistryFacts {
    match installed_apps {
        Fact::Known { value } => {
            resolve_installed_app_entries(value, Vec::new(), project_root, search_paths)
        }
        Fact::Partial { value, reasons } => {
            resolve_installed_app_entries(value, reasons.clone(), project_root, search_paths)
        }
        Fact::Unknown { reasons } | Fact::Ambiguous { reasons, .. } => {
            unknown_app_registry(reasons.clone())
        }
    }
}

fn resolve_installed_app_entries(
    entries: &[String],
    input_reasons: Vec<Reason>,
    project_root: &Utf8Path,
    search_paths: &[ModuleSearchPathEntry],
) -> AppRegistryFacts {
    let installed_apps = entries
        .iter()
        .map(|entry| resolve_installed_app_entry(entry, project_root, search_paths))
        .collect::<Vec<_>>();

    let mut reasons = input_reasons;
    for app in &installed_apps {
        reasons.extend(installed_app_reasons(app));
    }

    let app_registry = installed_apps
        .iter()
        .filter_map(promote_installed_app)
        .collect::<Vec<_>>();

    AppRegistryFacts {
        installed_apps: known_or_partial(installed_apps, reasons.clone()),
        app_registry: known_or_partial(app_registry, reasons),
    }
}

fn resolve_installed_app_entry(
    entry: &str,
    project_root: &Utf8Path,
    search_paths: &[ModuleSearchPathEntry],
) -> InstalledAppFact {
    let entry_module = match PyModuleName::parse(entry) {
        Ok(module) => module,
        Err(error) => {
            return unknown_installed_app(
                entry,
                vec![Reason::new(
                    Field::AppsInstalled,
                    ReasonSource::Unknown,
                    format!("installed app entry is not a valid Python module path: {error}"),
                )],
            );
        }
    };

    let direct_resolution = resolve_module(entry_module.clone(), search_paths, project_root);
    // Match Django's creation order: import the entry as a module first, then
    // fall back to treating the last segment as an AppConfig class.
    match &direct_resolution.resolved {
        Fact::Known { .. } | Fact::Partial { .. } | Fact::Ambiguous { .. } => {
            direct_installed_app(entry, entry_module, &direct_resolution)
        }
        Fact::Unknown {
            reasons: direct_reasons,
        } => resolve_app_config_entry(entry, direct_reasons.clone(), project_root, search_paths)
            .unwrap_or_else(|reasons| unknown_installed_app(entry, reasons)),
    }
}

fn direct_installed_app(
    entry: &str,
    module: PyModuleName,
    resolution: &ModuleResolution,
) -> InstalledAppFact {
    let module_fact = module_fact_from_resolution(module, &resolution.resolved);
    let path = app_path_fact_from_resolution(&resolution.resolved);

    InstalledAppFact {
        entry: entry.to_string(),
        module: module_fact,
        path,
        config: None,
    }
}

fn resolve_app_config_entry(
    entry: &str,
    direct_reasons: Vec<Reason>,
    project_root: &Utf8Path,
    search_paths: &[ModuleSearchPathEntry],
) -> Result<InstalledAppFact, Vec<Reason>> {
    let Some((config_module_raw, class_name)) = entry.rsplit_once('.') else {
        return Err(direct_reasons);
    };
    if class_name.trim().is_empty() {
        return Err(direct_reasons);
    }

    let config_module = match PyModuleName::parse(config_module_raw) {
        Ok(module) => module,
        Err(error) => {
            let mut reasons = direct_reasons;
            reasons.push(Reason::new(
                Field::AppsConfig,
                ReasonSource::Unknown,
                format!("AppConfig module path is invalid: {error}"),
            ));
            return Err(reasons);
        }
    };

    let config_resolution = resolve_module(config_module.clone(), search_paths, project_root);
    let (config_file, config_resolution_reasons) = match resolved_config_file(&config_resolution) {
        Ok(resolved) => resolved,
        Err(config_reasons) => {
            let mut reasons = direct_reasons;
            reasons.extend(config_reasons);
            return Err(reasons);
        }
    };

    let source = fs::read_to_string(&config_file).map_err(|error| {
        let mut reasons = direct_reasons.clone();
        reasons.push(Reason::file(
            Field::AppsConfig,
            config_file.clone(),
            format!("failed to read AppConfig module: {error}"),
        ));
        reasons
    })?;
    let module = ruff_python_parser::parse_module(&source)
        .map_err(|error| {
            let mut reasons = direct_reasons.clone();
            reasons.push(Reason::file(
                Field::AppsConfig,
                config_file.clone(),
                format!("failed to parse AppConfig module: {error}"),
            ));
            reasons
        })?
        .into_syntax();

    let Some(class) = module
        .body
        .iter()
        .find_map(|stmt| app_config_class(stmt, class_name))
    else {
        let mut reasons = direct_reasons;
        reasons.push(Reason::file(
            Field::AppsConfig,
            config_file,
            format!(
                "AppConfig class `{class_name}` was not found or does not inherit from AppConfig"
            ),
        ));
        return Err(reasons);
    };

    let default_app_module = default_app_module_from_config_module(&config_module);
    let assignments = app_config_assignments(class);
    let name = app_config_name(assignments.name, default_app_module, &config_file);
    let app_resolution = name
        .value()
        .map(|module| resolve_module(module.clone(), search_paths, project_root));

    let mut app_module = app_module_fact_from_name(&name, app_resolution.as_ref());
    app_module = add_reasons(app_module, config_resolution_reasons.clone());

    let default_path = app_resolution.as_ref().map_or_else(
        || Fact::unknown(name.reasons().to_vec()),
        |resolution| app_path_fact_from_resolution(&resolution.resolved),
    );
    let mut path = app_config_path(assignments.path, default_path, project_root, &config_file);
    path = add_reasons(path, config_resolution_reasons);

    let label = app_config_label(assignments.label, &name, &config_file);
    let config = AppConfigFact {
        module: config_module,
        class_name: class_name.to_string(),
        name,
        label,
        path: path.clone(),
    };

    Ok(InstalledAppFact {
        entry: entry.to_string(),
        module: app_module,
        path,
        config: Some(config),
    })
}

fn resolved_config_file(
    resolution: &ModuleResolution,
) -> Result<(Utf8PathBuf, Vec<Reason>), Vec<Reason>> {
    match &resolution.resolved {
        Fact::Known { value } => Ok((value.file.clone(), Vec::new())),
        Fact::Partial { value, reasons } => Ok((value.file.clone(), reasons.clone())),
        Fact::Unknown { reasons } | Fact::Ambiguous { reasons, .. } => Err(reasons.clone()),
    }
}

fn app_config_class<'a>(stmt: &'a Stmt, class_name: &str) -> Option<&'a StmtClassDef> {
    let Stmt::ClassDef(class) = stmt else {
        return None;
    };
    if class.name.as_str() != class_name {
        return None;
    }
    let arguments = class.arguments.as_ref()?;
    arguments
        .args
        .iter()
        .any(is_app_config_base)
        .then_some(class)
}

fn is_app_config_base(expr: &Expr) -> bool {
    matches!(base_class_name(expr), Some("AppConfig"))
}

fn base_class_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str()),
        Expr::Attribute(attribute) => Some(attribute.attr.as_str()),
        _ => None,
    }
}

fn app_config_assignments(class: &StmtClassDef) -> AppConfigAssignments<'_> {
    let mut assignments = AppConfigAssignments {
        name: None,
        label: None,
        path: None,
    };

    for stmt in &class.body {
        if let Some(value) = assigned_value(stmt, "name") {
            assignments.name = Some(value);
        }
        if let Some(value) = assigned_value(stmt, "label") {
            assignments.label = Some(value);
        }
        if let Some(value) = assigned_value(stmt, "path") {
            assignments.path = Some(value);
        }
    }

    assignments
}

fn assigned_value<'a>(stmt: &'a Stmt, target_name: &str) -> Option<&'a Expr> {
    match stmt {
        Stmt::Assign(assign) => assign
            .targets
            .iter()
            .any(|target| is_name(target, target_name))
            .then_some(assign.value.as_ref()),
        Stmt::AnnAssign(assign) if is_name(assign.target.as_ref(), target_name) => {
            assign.value.as_deref()
        }
        _ => None,
    }
}

fn is_name(expr: &Expr, expected: &str) -> bool {
    matches!(expr, Expr::Name(name) if name.id.as_str() == expected)
}

fn app_config_name(
    expr: Option<&Expr>,
    default_app_module: Option<PyModuleName>,
    file: &Utf8Path,
) -> Fact<PyModuleName> {
    let Some(expr) = expr else {
        return default_app_module.map_or_else(
            || {
                Fact::unknown(vec![Reason::file(
                    Field::AppsConfig,
                    file,
                    "AppConfig.name is not assigned and no default app module can be inferred",
                )])
            },
            |module| {
                Fact::partial(
                    module,
                    vec![Reason::file(
                        Field::AppsConfig,
                        file,
                        "AppConfig.name is not assigned; using the parent app module",
                    )],
                )
            },
        );
    };

    let Some(name) = string_literal(expr) else {
        return default_app_module.map_or_else(
            || {
                Fact::unknown(vec![Reason::file(
                    Field::AppsConfig,
                    file,
                    "AppConfig.name must be a string literal",
                )])
            },
            |module| {
                Fact::partial(
                    module,
                    vec![Reason::file(
                        Field::AppsConfig,
                        file,
                        "AppConfig.name must be a string literal; using the parent app module",
                    )],
                )
            },
        );
    };

    match PyModuleName::parse(&name) {
        Ok(module) => Fact::known(module),
        Err(error) => Fact::unknown(vec![Reason::file(
            Field::AppsConfig,
            file,
            format!("AppConfig.name is not a valid Python module path: {error}"),
        )]),
    }
}

fn app_config_label(
    expr: Option<&Expr>,
    name: &Fact<PyModuleName>,
    file: &Utf8Path,
) -> Fact<String> {
    if let Some(expr) = expr {
        if let Some(label) = string_literal(expr) {
            return Fact::known(label);
        }

        return add_reasons(
            default_label_from_name(name),
            vec![Reason::file(
                Field::AppsConfig,
                file,
                "AppConfig.label must be a string literal; using the app module basename",
            )],
        );
    }

    default_label_from_name(name)
}

fn app_config_path(
    expr: Option<&Expr>,
    default_path: Fact<Utf8PathBuf>,
    project_root: &Utf8Path,
    file: &Utf8Path,
) -> Fact<Utf8PathBuf> {
    let Some(expr) = expr else {
        return default_path;
    };

    let Some(path) = string_literal(expr) else {
        return default_path.with_reason(Reason::file(
            Field::AppsPath,
            file,
            "AppConfig.path must be a string literal; using the resolved app package path",
        ));
    };

    let path = normalize_app_config_path(project_root, Utf8Path::new(&path));
    if path.is_dir() {
        Fact::known(path)
    } else {
        default_path.with_reason(Reason::path(
            Field::AppsPath,
            path,
            "AppConfig.path does not exist or is not a directory; using the resolved app package path",
        ))
    }
}

fn normalize_app_config_path(project_root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn default_label_from_name(name: &Fact<PyModuleName>) -> Fact<String> {
    match name {
        Fact::Known { value } => Fact::known(module_basename(value).to_string()),
        Fact::Partial { value, reasons } => {
            Fact::partial(module_basename(value).to_string(), reasons.clone())
        }
        Fact::Unknown { reasons } => Fact::unknown(reasons.clone()),
        Fact::Ambiguous {
            candidates,
            reasons,
        } => Fact::ambiguous(
            candidates
                .iter()
                .map(|candidate| module_basename(candidate).to_string())
                .collect(),
            reasons.clone(),
        ),
    }
}

fn module_basename(module: &PyModuleName) -> &str {
    module
        .as_str()
        .rsplit('.')
        .next()
        .unwrap_or(module.as_str())
}

fn default_app_module_from_config_module(module: &PyModuleName) -> Option<PyModuleName> {
    let mut parts = module.as_str().split('.').collect::<Vec<_>>();
    parts.pop();

    if parts.is_empty() {
        return None;
    }

    PyModuleName::parse(&parts.join(".")).ok()
}

fn app_module_fact_from_name(
    name: &Fact<PyModuleName>,
    resolution: Option<&ModuleResolution>,
) -> Fact<PyModuleName> {
    let Some(module) = name.value().cloned() else {
        return Fact::unknown(name.reasons().to_vec());
    };

    let Some(resolution) = resolution else {
        return Fact::unknown(name.reasons().to_vec());
    };

    let mut fact = module_fact_from_resolution(module, &resolution.resolved);
    fact = add_reasons(fact, name.reasons().to_vec());
    fact
}

fn module_fact_from_resolution(
    module: PyModuleName,
    resolution: &Fact<ResolvedModule>,
) -> Fact<PyModuleName> {
    match resolution {
        Fact::Known { .. } => Fact::known(module),
        Fact::Partial { reasons, .. } | Fact::Ambiguous { reasons, .. } => {
            Fact::partial(module, reasons.clone())
        }
        Fact::Unknown { reasons } => Fact::unknown(reasons.clone()),
    }
}

fn app_path_fact_from_resolution(resolution: &Fact<ResolvedModule>) -> Fact<Utf8PathBuf> {
    match resolution {
        Fact::Known { value } => Fact::known(app_path_from_resolved(value)),
        Fact::Partial { value, reasons } => {
            Fact::partial(app_path_from_resolved(value), reasons.clone())
        }
        Fact::Unknown { reasons } => Fact::unknown(reasons.clone()),
        Fact::Ambiguous {
            candidates,
            reasons,
        } => Fact::ambiguous(
            unique_app_paths(candidates.iter().map(app_path_from_resolved)),
            reasons.clone(),
        ),
    }
}

fn app_path_from_resolved(resolved: &ResolvedModule) -> Utf8PathBuf {
    resolved
        .file
        .parent()
        .map_or_else(|| resolved.file.clone(), Utf8Path::to_path_buf)
}

fn unique_app_paths(paths: impl Iterator<Item = Utf8PathBuf>) -> Vec<Utf8PathBuf> {
    paths.fold(Vec::new(), |mut unique, path| {
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
        unique
    })
}

fn promote_installed_app(app: &InstalledAppFact) -> Option<AppFact> {
    Some(AppFact {
        entry: app.entry.clone(),
        module: app.module.value()?.clone(),
        path: app.path.value()?.clone(),
        config: app.config.clone(),
    })
}

fn installed_app_reasons(app: &InstalledAppFact) -> Vec<Reason> {
    let mut reasons = Vec::new();
    extend_unique_reasons(&mut reasons, app.module.reasons().iter().cloned());
    extend_unique_reasons(&mut reasons, app.path.reasons().iter().cloned());
    if let Some(config) = &app.config {
        extend_unique_reasons(&mut reasons, config.name.reasons().iter().cloned());
        extend_unique_reasons(&mut reasons, config.label.reasons().iter().cloned());
        extend_unique_reasons(&mut reasons, config.path.reasons().iter().cloned());
    }
    reasons
}

fn extend_unique_reasons(reasons: &mut Vec<Reason>, new_reasons: impl Iterator<Item = Reason>) {
    for reason in new_reasons {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
}

fn add_reasons<T>(mut fact: Fact<T>, reasons: Vec<Reason>) -> Fact<T> {
    for reason in reasons {
        fact = fact.with_reason(reason);
    }
    fact
}

fn known_or_partial<T>(value: Vec<T>, reasons: Vec<Reason>) -> Fact<Vec<T>> {
    if reasons.is_empty() {
        Fact::known(value)
    } else {
        Fact::partial(value, reasons)
    }
}

fn unknown_app_registry(reasons: Vec<Reason>) -> AppRegistryFacts {
    AppRegistryFacts {
        installed_apps: Fact::unknown(reasons.clone()),
        app_registry: Fact::unknown(reasons),
    }
}

fn unknown_installed_app(entry: &str, reasons: Vec<Reason>) -> InstalledAppFact {
    InstalledAppFact {
        entry: entry.to_string(),
        module: Fact::unknown(reasons.clone()),
        path: Fact::unknown(reasons),
        config: None,
    }
}

fn string_literal(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(string) = expr {
        return Some(string.value.to_str().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::project::facts::ModuleSearchPathKind;
    use crate::project::module_resolver::discover_module_search_paths;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn search_paths(root: &Utf8Path) -> Vec<ModuleSearchPathEntry> {
        discover_module_search_paths(root, &[], &[])
            .value()
            .cloned()
            .unwrap()
    }

    fn search_paths_with_explicit(
        root: &Utf8Path,
        explicit: &[Utf8PathBuf],
    ) -> Vec<ModuleSearchPathEntry> {
        discover_module_search_paths(root, explicit, &[])
            .value()
            .cloned()
            .unwrap()
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

    fn known_module(fact: &Fact<PyModuleName>) -> PyModuleName {
        let Fact::Known { value } = fact else {
            panic!("expected known module, got {fact:?}");
        };
        value.clone()
    }

    fn known_path(fact: &Fact<Utf8PathBuf>) -> Utf8PathBuf {
        let Fact::Known { value } = fact else {
            panic!("expected known path, got {fact:?}");
        };
        value.clone()
    }

    fn partial_module(fact: &Fact<PyModuleName>) -> (PyModuleName, Vec<Reason>) {
        let Fact::Partial { value, reasons } = fact else {
            panic!("expected partial module, got {fact:?}");
        };
        (value.clone(), reasons.clone())
    }

    #[test]
    fn resolves_direct_app_package_entries() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");

        let facts = resolve_app_registry(
            &Fact::known(vec!["blog".to_string()]),
            &root,
            &search_paths(&root),
        );

        let installed_apps = known_vec(&facts.installed_apps);
        assert_eq!(installed_apps.len(), 1);
        assert_eq!(installed_apps[0].entry, "blog");
        assert_eq!(known_module(&installed_apps[0].module), module("blog"));
        assert_eq!(known_path(&installed_apps[0].path), root.join("blog"));
        assert!(installed_apps[0].config.is_none());

        let app_registry = known_vec(&facts.app_registry);
        assert_eq!(app_registry.len(), 1);
        assert_eq!(app_registry[0].module, module("blog"));
        assert_eq!(app_registry[0].path, root.join("blog"));
    }

    #[test]
    fn resolves_app_config_entries() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        write_file(
            &root.join("blog/apps.py"),
            r#"
from django.apps import AppConfig

class BlogConfig(AppConfig):
    name = "blog"
    label = "weblog"
"#,
        );

        let facts = resolve_app_registry(
            &Fact::known(vec!["blog.apps.BlogConfig".to_string()]),
            &root,
            &search_paths(&root),
        );

        let installed_apps = known_vec(&facts.installed_apps);
        assert_eq!(known_module(&installed_apps[0].module), module("blog"));
        assert_eq!(known_path(&installed_apps[0].path), root.join("blog"));
        let config = installed_apps[0].config.as_ref().unwrap();
        assert_eq!(config.module, module("blog.apps"));
        assert_eq!(config.class_name, "BlogConfig");
        assert_eq!(known_module(&config.name), module("blog"));
        assert_eq!(config.label.value().unwrap(), "weblog");
        assert_eq!(known_path(&config.path), root.join("blog"));

        let app_registry = known_vec(&facts.app_registry);
        assert_eq!(app_registry[0].module, module("blog"));
        assert!(app_registry[0].config.is_some());
    }

    #[test]
    fn defaults_missing_app_config_name_and_label_as_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        write_file(
            &root.join("blog/apps.py"),
            r"
from django.apps import AppConfig

class BlogConfig(AppConfig):
    pass
",
        );

        let facts = resolve_app_registry(
            &Fact::known(vec!["blog.apps.BlogConfig".to_string()]),
            &root,
            &search_paths(&root),
        );

        let (installed_apps, reasons) = partial_vec(&facts.installed_apps);
        assert_eq!(installed_apps.len(), 1);
        let (app_module, module_reasons) = partial_module(&installed_apps[0].module);
        assert_eq!(app_module, module("blog"));
        assert!(module_reasons[0]
            .message
            .contains("AppConfig.name is not assigned"));
        let config = installed_apps[0].config.as_ref().unwrap();
        assert_eq!(config.label.value().unwrap(), "blog");
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("AppConfig.name is not assigned")));

        let (app_registry, _) = partial_vec(&facts.app_registry);
        assert_eq!(app_registry[0].module, module("blog"));
    }

    #[test]
    fn falls_back_when_app_config_name_is_not_literal() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        write_file(
            &root.join("blog/apps.py"),
            r#"
from django.apps import AppConfig

BLOG_NAME = "blog"

class BlogConfig(AppConfig):
    name = BLOG_NAME
"#,
        );

        let facts = resolve_app_registry(
            &Fact::known(vec!["blog.apps.BlogConfig".to_string()]),
            &root,
            &search_paths(&root),
        );

        let (installed_apps, reasons) = partial_vec(&facts.installed_apps);
        let (app_module, module_reasons) = partial_module(&installed_apps[0].module);
        assert_eq!(app_module, module("blog"));
        assert!(module_reasons[0]
            .message
            .contains("AppConfig.name must be a string literal"));
        assert!(reasons.iter().any(|reason| reason
            .message
            .contains("AppConfig.name must be a string literal")));
    }

    #[test]
    fn uses_existing_app_config_path_override() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let app_path = root.join("custom_blog_path");
        fs::create_dir_all(&app_path).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        write_file(
            &root.join("blog/apps.py"),
            &format!(
                r#"
from django.apps import AppConfig

class BlogConfig(AppConfig):
    name = "blog"
    path = "{app_path}"
"#,
            ),
        );

        let facts = resolve_app_registry(
            &Fact::known(vec!["blog.apps.BlogConfig".to_string()]),
            &root,
            &search_paths(&root),
        );

        let installed_apps = known_vec(&facts.installed_apps);
        assert_eq!(known_path(&installed_apps[0].path), app_path);
        assert_eq!(
            known_path(&installed_apps[0].config.as_ref().unwrap().path),
            app_path
        );
    }

    #[test]
    fn falls_back_when_app_config_path_override_does_not_exist() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        write_file(
            &root.join("blog/apps.py"),
            r#"
from django.apps import AppConfig

class BlogConfig(AppConfig):
    name = "blog"
    path = "missing-blog-path"
"#,
        );

        let facts = resolve_app_registry(
            &Fact::known(vec!["blog.apps.BlogConfig".to_string()]),
            &root,
            &search_paths(&root),
        );

        let (installed_apps, reasons) = partial_vec(&facts.installed_apps);
        assert_eq!(installed_apps[0].path.value().unwrap(), &root.join("blog"));
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("AppConfig.path does not exist")));

        let (app_registry, _) = partial_vec(&facts.app_registry);
        assert_eq!(app_registry[0].path, root.join("blog"));
    }

    #[test]
    fn unresolved_entries_do_not_abort_the_registry() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");

        let facts = resolve_app_registry(
            &Fact::known(vec!["blog".to_string(), "missing.app".to_string()]),
            &root,
            &search_paths(&root),
        );

        let (installed_apps, reasons) = partial_vec(&facts.installed_apps);
        assert_eq!(installed_apps.len(), 2);
        assert!(matches!(installed_apps[1].module, Fact::Unknown { .. }));
        assert!(reasons
            .iter()
            .any(|reason| reason.field == Field::ResolverModule));

        let (app_registry, reasons) = partial_vec(&facts.app_registry);
        assert_eq!(app_registry.len(), 1);
        assert_eq!(app_registry[0].module, module("blog"));
        assert!(reasons
            .iter()
            .any(|reason| reason.field == Field::ResolverModule));
    }

    #[test]
    fn ambiguous_direct_app_entries_are_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("shop/__init__.py"), "");
        write_file(&root.join("src/shop/__init__.py"), "");

        let facts = resolve_app_registry(
            &Fact::known(vec!["shop".to_string()]),
            &root,
            &search_paths(&root),
        );

        let (installed_apps, reasons) = partial_vec(&facts.installed_apps);
        let (app_module, _) = partial_module(&installed_apps[0].module);
        assert_eq!(app_module, module("shop"));
        assert!(matches!(installed_apps[0].path, Fact::Ambiguous { .. }));
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("more than one module search path")));

        let (app_registry, _) = partial_vec(&facts.app_registry);
        assert!(app_registry.is_empty());
    }

    #[test]
    fn uses_configured_source_roots_for_app_packages() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let source_root = root.join("backend/python/apps");
        write_file(&source_root.join("catalog/__init__.py"), "");

        let search_paths =
            search_paths_with_explicit(&root, &[Utf8PathBuf::from("backend/python/apps")]);
        assert!(search_paths.iter().any(|search_path| {
            search_path.kind == ModuleSearchPathKind::ExplicitPythonPath
                && search_path.path == source_root
        }));

        let facts = resolve_app_registry(
            &Fact::known(vec!["catalog".to_string()]),
            &root,
            &search_paths,
        );

        let app_registry = known_vec(&facts.app_registry);
        assert_eq!(app_registry[0].module, module("catalog"));
        assert_eq!(app_registry[0].path, source_root.join("catalog"));
    }

    #[test]
    fn rejects_app_config_entries_without_app_config_base() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        write_file(
            &root.join("blog/apps.py"),
            r#"
class BlogConfig:
    name = "blog"
"#,
        );

        let facts = resolve_app_registry(
            &Fact::known(vec!["blog.apps.BlogConfig".to_string()]),
            &root,
            &search_paths(&root),
        );

        let (installed_apps, reasons) = partial_vec(&facts.installed_apps);
        assert!(matches!(installed_apps[0].module, Fact::Unknown { .. }));
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("does not inherit from AppConfig")));
    }
}
