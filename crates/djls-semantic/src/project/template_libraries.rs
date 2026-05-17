//! Static Django template library facts.
//!
//! This module assembles loadable template libraries and builtin libraries from
//! extracted template backend settings plus the static app registry. It models
//! Django's standard `DjangoTemplates` backend: Django's built-in `{% load %}`
//! libraries, installed-app `templatetags` packages, `OPTIONS.libraries`,
//! Django's default builtins, and `OPTIONS.builtins`.
//!
//! A8 facts are flattened across Django template backends and do not carry
//! backend identity yet. Django defaults and installed-app libraries are
//! assembled once when a standard Django backend is present; explicit
//! `OPTIONS.libraries` entries are then applied in settings order and replace
//! discovered/default libraries, matching Django's `libraries.update(...)`
//! behavior within the flattened model.

#![allow(
    dead_code,
    reason = "Milestone A8 adds template library facts before project facts are assembled."
)]

use std::collections::BTreeMap;
use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;

use crate::project::facts::AppFact;
use crate::project::facts::Fact;
use crate::project::facts::Field;
use crate::project::facts::Reason;
use crate::project::facts::ReasonSource;
use crate::project::facts::TemplateBackendFact;
use crate::project::facts::TemplateLibraryFact;
use crate::project::facts::TemplateLibrarySource;
use crate::project::names::LibraryName;
use crate::project::names::PyModuleName;

const DJANGO_TEMPLATES_BACKEND: &str = "django.template.backends.django.DjangoTemplates";

const DJANGO_DEFAULT_LOADABLE_LIBRARIES: &[(&str, &str)] = &[
    ("cache", "django.templatetags.cache"),
    ("i18n", "django.templatetags.i18n"),
    ("l10n", "django.templatetags.l10n"),
    ("static", "django.templatetags.static"),
    ("tz", "django.templatetags.tz"),
];

const DJANGO_DEFAULT_BUILTINS: &[&str] = &[
    "django.template.defaulttags",
    "django.template.defaultfilters",
    "django.template.loader_tags",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendKind {
    Django,
    NonDjango,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DuplicatePolicy {
    Report,
    SilentOverride,
}

#[must_use]
pub(crate) fn assemble_template_libraries(
    template_backends: &Fact<Vec<TemplateBackendFact>>,
    app_registry: &Fact<Vec<AppFact>>,
) -> Fact<Vec<TemplateLibraryFact>> {
    match template_backends {
        Fact::Known { value } => {
            assemble_template_backend_libraries(value, Vec::new(), app_registry)
        }
        Fact::Partial { value, reasons } => {
            assemble_template_backend_libraries(value, reasons.clone(), app_registry)
        }
        Fact::Unknown { reasons } | Fact::Ambiguous { reasons, .. } => {
            Fact::unknown(reasons.clone())
        }
    }
}

fn assemble_template_backend_libraries(
    backends: &[TemplateBackendFact],
    mut reasons: Vec<Reason>,
    app_registry: &Fact<Vec<AppFact>>,
) -> Fact<Vec<TemplateLibraryFact>> {
    let mut loadable = BTreeMap::new();
    let mut builtins = Vec::new();
    let mut has_django_backend = false;
    let mut option_backends = Vec::new();

    for backend in backends {
        match classify_template_backend(backend, &mut reasons) {
            BackendKind::Django => {
                has_django_backend = true;
                option_backends.push(backend);
            }
            BackendKind::Unknown => option_backends.push(backend),
            BackendKind::NonDjango => {}
        }
    }

    if has_django_backend {
        append_default_loadable_libraries(&mut loadable);
        append_app_template_tag_libraries(&mut loadable, &mut reasons, app_registry);
        append_default_builtins(&mut builtins);
    }

    for backend in option_backends {
        append_option_libraries(&mut loadable, &mut reasons, &backend.option_libraries);
        append_option_builtins(&mut builtins, &mut reasons, &backend.option_builtins);
    }

    let mut libraries = loadable.into_values().collect::<Vec<_>>();
    libraries.extend(builtins);
    known_or_partial(libraries, reasons)
}

fn classify_template_backend(
    backend: &TemplateBackendFact,
    reasons: &mut Vec<Reason>,
) -> BackendKind {
    let Some(backend) = backend.backend.as_deref() else {
        reasons.push(Reason::new(
            Field::TemplateLibraries,
            ReasonSource::Unknown,
            "TEMPLATES BACKEND is not known; only statically known template library options were assembled for this backend",
        ));
        return BackendKind::Unknown;
    };

    if backend == DJANGO_TEMPLATES_BACKEND {
        BackendKind::Django
    } else {
        BackendKind::NonDjango
    }
}

fn append_default_loadable_libraries(loadable: &mut BTreeMap<LibraryName, TemplateLibraryFact>) {
    for (load_name, module) in DJANGO_DEFAULT_LOADABLE_LIBRARIES {
        let fact = TemplateLibraryFact {
            load_name: LibraryName::parse(load_name).expect("Django default library name is valid"),
            module: PyModuleName::parse(module).expect("Django default library module is valid"),
            source: TemplateLibrarySource::DjangoDefaultLibrary,
        };
        loadable.entry(fact.load_name.clone()).or_insert(fact);
    }
}

fn append_app_template_tag_libraries(
    loadable: &mut BTreeMap<LibraryName, TemplateLibraryFact>,
    reasons: &mut Vec<Reason>,
    app_registry: &Fact<Vec<AppFact>>,
) {
    match app_registry {
        Fact::Known { value } => push_app_template_tag_libraries(loadable, reasons, value),
        Fact::Partial {
            value,
            reasons: app_reasons,
        } => {
            push_app_template_tag_libraries(loadable, reasons, value);
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

fn push_app_template_tag_libraries(
    loadable: &mut BTreeMap<LibraryName, TemplateLibraryFact>,
    reasons: &mut Vec<Reason>,
    apps: &[AppFact],
) {
    for app in apps {
        for fact in discover_app_template_tag_libraries(app, reasons) {
            upsert_loadable_library(loadable, reasons, fact, DuplicatePolicy::Report);
        }
    }
}

fn discover_app_template_tag_libraries(
    app: &AppFact,
    reasons: &mut Vec<Reason>,
) -> Vec<TemplateLibraryFact> {
    let templatetags_dir = app.path.join("templatetags");
    if !templatetags_dir.is_dir() {
        return Vec::new();
    }

    let mut files = Vec::new();
    collect_python_files(&templatetags_dir, &mut files, reasons);
    files.sort();

    files
        .into_iter()
        .filter_map(|file| app_template_tag_library(app, &templatetags_dir, &file, reasons))
        .collect()
}

fn collect_python_files(dir: &Utf8Path, files: &mut Vec<Utf8PathBuf>, reasons: &mut Vec<Reason>) {
    let Ok(entries) = fs::read_dir(dir.as_std_path()) else {
        reasons.push(Reason::path(
            Field::TemplateLibraries,
            dir,
            "failed to read templatetags package directory",
        ));
        return;
    };

    let mut paths = entries
        .filter_map(Result::ok)
        .filter_map(|entry| Utf8PathBuf::try_from(entry.path()).ok())
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        if path.is_dir() {
            // Django discovers nested libraries through pkgutil.walk_packages(),
            // which only descends into package directories.
            if path.join("__init__.py").is_file() {
                collect_python_files(&path, files, reasons);
            }
        } else if path.extension() == Some("py") {
            files.push(path);
        }
    }
}

fn app_template_tag_library(
    app: &AppFact,
    templatetags_dir: &Utf8Path,
    file: &Utf8Path,
    reasons: &mut Vec<Reason>,
) -> Option<TemplateLibraryFact> {
    let load_name = template_tag_load_name(templatetags_dir, file, reasons)?;
    if !defines_template_register(file, reasons) {
        return None;
    }

    let module = PyModuleName::parse(&format!(
        "{}.templatetags.{}",
        app.module.as_str(),
        load_name.as_str()
    ))
    .ok()?;

    Some(TemplateLibraryFact {
        load_name,
        module,
        source: TemplateLibrarySource::AppTemplateTags {
            app: app.module.clone(),
        },
    })
}

fn template_tag_load_name(
    templatetags_dir: &Utf8Path,
    file: &Utf8Path,
    reasons: &mut Vec<Reason>,
) -> Option<LibraryName> {
    let relative = file.strip_prefix(templatetags_dir).ok()?;
    let module_path = if relative.file_name() == Some("__init__.py") {
        let parent = relative.parent()?;
        if parent.as_str().is_empty() {
            return None;
        }
        parent.to_path_buf()
    } else {
        relative.with_extension("")
    };

    let dotted = dotted_path(&module_path);
    match LibraryName::parse(&dotted) {
        Ok(load_name) => Some(load_name),
        Err(error) => {
            reasons.push(Reason::file(
                Field::TemplateLibraries,
                file,
                format!("template tag module has an invalid load name: {error}"),
            ));
            None
        }
    }
}

fn dotted_path(path: &Utf8Path) -> String {
    path.components()
        .map(|component| component.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn defines_template_register(file: &Utf8Path, reasons: &mut Vec<Reason>) -> bool {
    let source = match fs::read_to_string(file.as_std_path()) {
        Ok(source) => source,
        Err(error) => {
            reasons.push(Reason::file(
                Field::TemplateLibraries,
                file,
                format!("failed to read template tag module: {error}"),
            ));
            return false;
        }
    };

    let module = match ruff_python_parser::parse_module(&source) {
        Ok(parsed) => parsed.into_syntax(),
        Err(error) => {
            reasons.push(Reason::file(
                Field::TemplateLibraries,
                file,
                format!("failed to parse template tag module: {error}"),
            ));
            return false;
        }
    };

    module.body.iter().any(stmt_defines_template_register)
}

fn stmt_defines_template_register(stmt: &Stmt) -> bool {
    if imported_as_register(stmt) {
        return true;
    }

    match stmt {
        Stmt::FunctionDef(function) if function.name.as_str() == "register" => true,
        Stmt::ClassDef(class) if class.name.as_str() == "register" => true,
        _ => {
            // Django's discovery accepts any module with a top-level `register`
            // attribute. Keep the static filter at the same boundary instead of
            // trying to prove that the value is specifically a
            // `django.template.Library`.
            assigned_value(stmt, "register").is_some()
        }
    }
}

fn imported_as_register(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Import(import) => import.names.iter().any(alias_binds_register),
        Stmt::ImportFrom(import_from) => import_from.names.iter().any(alias_binds_register),
        _ => false,
    }
}

fn alias_binds_register(alias: &ruff_python_ast::Alias) -> bool {
    alias
        .asname
        .as_ref()
        .map_or(alias.name.as_str(), |asname| asname.as_str())
        == "register"
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

fn append_option_libraries(
    loadable: &mut BTreeMap<LibraryName, TemplateLibraryFact>,
    reasons: &mut Vec<Reason>,
    option_libraries: &Fact<Vec<TemplateLibraryFact>>,
) {
    match option_libraries {
        Fact::Known { value } => push_option_libraries(loadable, reasons, value),
        Fact::Partial {
            value,
            reasons: library_reasons,
        } => {
            push_option_libraries(loadable, reasons, value);
            extend_unique_reasons(reasons, library_reasons.iter().cloned());
        }
        Fact::Unknown {
            reasons: library_reasons,
        }
        | Fact::Ambiguous {
            reasons: library_reasons,
            ..
        } => {
            extend_unique_reasons(reasons, library_reasons.iter().cloned());
        }
    }
}

fn push_option_libraries(
    loadable: &mut BTreeMap<LibraryName, TemplateLibraryFact>,
    reasons: &mut Vec<Reason>,
    libraries: &[TemplateLibraryFact],
) {
    for library in libraries {
        upsert_loadable_library(
            loadable,
            reasons,
            library.clone(),
            DuplicatePolicy::SilentOverride,
        );
    }
}

fn append_default_builtins(builtins: &mut Vec<TemplateLibraryFact>) {
    for module in DJANGO_DEFAULT_BUILTINS {
        push_unique_builtin(
            builtins,
            builtin_fact(
                PyModuleName::parse(module).expect("Django default builtin module is valid"),
                TemplateLibrarySource::DjangoDefaultBuiltin,
            ),
        );
    }
}

fn append_option_builtins(
    builtins: &mut Vec<TemplateLibraryFact>,
    reasons: &mut Vec<Reason>,
    option_builtins: &Fact<Vec<PyModuleName>>,
) {
    match option_builtins {
        Fact::Known { value } => push_option_builtins(builtins, value),
        Fact::Partial {
            value,
            reasons: builtin_reasons,
        } => {
            push_option_builtins(builtins, value);
            extend_unique_reasons(reasons, builtin_reasons.iter().cloned());
        }
        Fact::Unknown {
            reasons: builtin_reasons,
        }
        | Fact::Ambiguous {
            reasons: builtin_reasons,
            ..
        } => {
            extend_unique_reasons(reasons, builtin_reasons.iter().cloned());
        }
    }
}

fn push_option_builtins(builtins: &mut Vec<TemplateLibraryFact>, modules: &[PyModuleName]) {
    for module in modules {
        push_unique_builtin(
            builtins,
            builtin_fact(module.clone(), TemplateLibrarySource::SettingsBuiltins),
        );
    }
}

fn builtin_fact(module: PyModuleName, source: TemplateLibrarySource) -> TemplateLibraryFact {
    let load_name = module
        .as_str()
        .split('.')
        .next_back()
        .and_then(|name| LibraryName::parse(name).ok())
        .expect("builtin module basename is a valid library name");
    TemplateLibraryFact {
        load_name,
        module,
        source,
    }
}

fn push_unique_builtin(builtins: &mut Vec<TemplateLibraryFact>, builtin: TemplateLibraryFact) {
    if builtins
        .iter()
        .any(|existing| existing.module == builtin.module)
    {
        return;
    }
    builtins.push(builtin);
}

fn upsert_loadable_library(
    loadable: &mut BTreeMap<LibraryName, TemplateLibraryFact>,
    reasons: &mut Vec<Reason>,
    library: TemplateLibraryFact,
    duplicate_policy: DuplicatePolicy,
) {
    if duplicate_policy == DuplicatePolicy::Report {
        if let Some(existing) = loadable.get(&library.load_name) {
            if existing.module != library.module {
                let reason = Reason::module(
                    Field::TemplateLibraries,
                    library.module.clone(),
                    format!(
                        "template library load name `{}` is provided by both `{}` and `{}`; the later Django discovery entry wins",
                        library.load_name.as_str(),
                        existing.module.as_str(),
                        library.module.as_str(),
                    ),
                );
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
        }
    }

    loadable.insert(library.load_name.clone(), library);
}

fn known_or_partial(
    value: Vec<TemplateLibraryFact>,
    reasons: Vec<Reason>,
) -> Fact<Vec<TemplateLibraryFact>> {
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
    use crate::project::facts::ModuleSearchPathEntry;
    use crate::project::facts::TemplateDirFact;
    use crate::project::facts::TemplateDirSource;
    use crate::project::module_resolver::discover_module_search_paths;
    use crate::project::settings_facts::extract_settings_facts;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn library(name: &str) -> LibraryName {
        LibraryName::parse(name).unwrap()
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

    fn search_paths(root: &Utf8Path) -> Vec<ModuleSearchPathEntry> {
        discover_module_search_paths(root, &[], &[])
            .value()
            .unwrap()
            .clone()
    }

    fn backend(app_dirs: Fact<bool>) -> TemplateBackendFact {
        TemplateBackendFact {
            backend: Some(DJANGO_TEMPLATES_BACKEND.to_string()),
            dirs: Fact::known(Vec::new()),
            app_dirs,
            option_libraries: Fact::known(Vec::new()),
            option_builtins: Fact::known(Vec::new()),
        }
    }

    fn backend_with_options(
        option_libraries: Fact<Vec<TemplateLibraryFact>>,
        option_builtins: Fact<Vec<PyModuleName>>,
    ) -> TemplateBackendFact {
        TemplateBackendFact {
            option_libraries,
            option_builtins,
            ..backend(Fact::known(false))
        }
    }

    fn settings_library(load_name: &str, module_name: &str) -> TemplateLibraryFact {
        TemplateLibraryFact {
            load_name: library(load_name),
            module: module(module_name),
            source: TemplateLibrarySource::SettingsLibraries,
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

    fn write_template_library(root: &Utf8Path, app_module: &str, load_name: &str) {
        write_file(
            &root.join(format!(
                "{}/templatetags/{load_name}.py",
                app_module.replace('.', "/")
            )),
            r"
from django import template

register = template.Library()
",
        );
    }

    fn known_vec(fact: &Fact<Vec<TemplateLibraryFact>>) -> Vec<TemplateLibraryFact> {
        match fact {
            Fact::Known { value } => value.clone(),
            other => panic!("expected known template libraries, got {other:?}"),
        }
    }

    fn partial_vec(
        fact: &Fact<Vec<TemplateLibraryFact>>,
    ) -> (Vec<TemplateLibraryFact>, Vec<Reason>) {
        match fact {
            Fact::Partial { value, reasons } => (value.clone(), reasons.clone()),
            other => panic!("expected partial template libraries, got {other:?}"),
        }
    }

    fn loadable_map(fact: &Fact<Vec<TemplateLibraryFact>>) -> BTreeMap<String, String> {
        fact.value()
            .unwrap()
            .iter()
            .filter(|library| {
                !matches!(
                    library.source,
                    TemplateLibrarySource::DjangoDefaultBuiltin
                        | TemplateLibrarySource::SettingsBuiltins
                )
            })
            .map(|library| {
                (
                    library.load_name.as_str().to_string(),
                    library.module.as_str().to_string(),
                )
            })
            .collect()
    }

    fn builtin_modules(fact: &Fact<Vec<TemplateLibraryFact>>) -> Vec<String> {
        fact.value()
            .unwrap()
            .iter()
            .filter(|library| {
                matches!(
                    library.source,
                    TemplateLibrarySource::DjangoDefaultBuiltin
                        | TemplateLibrarySource::SettingsBuiltins
                )
            })
            .map(|library| library.module.as_str().to_string())
            .collect()
    }

    fn app_registry_reason() -> Reason {
        Reason::new(
            Field::AppsInstalled,
            ReasonSource::Unknown,
            "INSTALLED_APPS has an unsupported dynamic entry",
        )
    }

    fn option_reason() -> Reason {
        Reason::new(
            Field::SettingsTemplateOptions,
            ReasonSource::Unknown,
            "TEMPLATES OPTIONS.libraries contains an unsupported value",
        )
    }

    #[test]
    fn adds_django_default_libraries_and_builtins() {
        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(false))]),
            &Fact::known(Vec::new()),
        );

        assert_eq!(
            loadable_map(&facts),
            BTreeMap::from([
                ("cache".to_string(), "django.templatetags.cache".to_string(),),
                ("i18n".to_string(), "django.templatetags.i18n".to_string()),
                ("l10n".to_string(), "django.templatetags.l10n".to_string()),
                (
                    "static".to_string(),
                    "django.templatetags.static".to_string(),
                ),
                ("tz".to_string(), "django.templatetags.tz".to_string()),
            ])
        );
        assert_eq!(
            builtin_modules(&facts),
            [
                "django.template.defaulttags",
                "django.template.defaultfilters",
                "django.template.loader_tags",
            ]
        );
    }

    #[test]
    fn settings_libraries_override_default_load_names_and_append_builtins() {
        let facts = assemble_template_libraries(
            &Fact::known(vec![backend_with_options(
                Fact::known(vec![settings_library(
                    "static",
                    "project.templatetags.assets",
                )]),
                Fact::known(vec![module("project.templatetags.extra_builtins")]),
            )]),
            &Fact::known(Vec::new()),
        );

        let loadable = loadable_map(&facts);
        assert_eq!(
            loadable.get("static").map(String::as_str),
            Some("project.templatetags.assets")
        );
        assert_eq!(
            builtin_modules(&facts),
            [
                "django.template.defaulttags",
                "django.template.defaultfilters",
                "django.template.loader_tags",
                "project.templatetags.extra_builtins",
            ]
        );
    }

    #[test]
    fn discovers_app_templatetags_even_when_app_dirs_is_false() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_template_library(&root, "blog", "blog_tags");

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(false))]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        assert_eq!(
            loadable_map(&facts).get("blog_tags").map(String::as_str),
            Some("blog.templatetags.blog_tags")
        );
    }

    #[test]
    fn accepts_modules_with_top_level_register_attribute() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(
            &root.join("blog/templatetags/aliased.py"),
            r"
from django.template import Library as L

register = L()
",
        );
        write_file(
            &root.join("blog/templatetags/indirect.py"),
            r"
from django import template

lib = template.Library()
register = lib
",
        );
        write_file(
            &root.join("blog/templatetags/reexported.py"),
            "from .aliased import register\n",
        );
        write_file(
            &root.join("blog/templatetags/imported.py"),
            "import blog.templatetags.aliased as register\n",
        );
        write_file(
            &root.join("blog/templatetags/function.py"),
            "def register():\n    pass\n",
        );
        write_file(
            &root.join("blog/templatetags/classy.py"),
            "class register:\n    pass\n",
        );

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(true))]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        let loadable = loadable_map(&facts);
        assert_eq!(
            loadable.get("aliased").map(String::as_str),
            Some("blog.templatetags.aliased")
        );
        assert_eq!(
            loadable.get("indirect").map(String::as_str),
            Some("blog.templatetags.indirect")
        );
        assert_eq!(
            loadable.get("reexported").map(String::as_str),
            Some("blog.templatetags.reexported")
        );
        assert_eq!(
            loadable.get("imported").map(String::as_str),
            Some("blog.templatetags.imported")
        );
        assert_eq!(
            loadable.get("function").map(String::as_str),
            Some("blog.templatetags.function")
        );
        assert_eq!(
            loadable.get("classy").map(String::as_str),
            Some("blog.templatetags.classy")
        );
    }

    #[test]
    fn filters_templatetag_modules_without_register() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(
            &root.join("blog/templatetags/base.py"),
            "class Helper: pass\n",
        );
        write_template_library(&root, "blog", "real_tags");

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(true))]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        let loadable = loadable_map(&facts);
        assert!(!loadable.contains_key("base"));
        assert_eq!(
            loadable.get("real_tags").map(String::as_str),
            Some("blog.templatetags.real_tags")
        );
    }

    #[test]
    fn discovers_nested_templatetag_packages() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(
            &root.join("blog/templatetags/nested/__init__.py"),
            r"
from django.template import Library

register = Library()
",
        );
        write_file(
            &root.join("blog/templatetags/nested/deep.py"),
            r"
from django import template

register = template.Library()
",
        );

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(true))]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        let loadable = loadable_map(&facts);
        assert_eq!(
            loadable.get("nested").map(String::as_str),
            Some("blog.templatetags.nested")
        );
        assert_eq!(
            loadable.get("nested.deep").map(String::as_str),
            Some("blog.templatetags.nested.deep")
        );
    }

    #[test]
    fn nested_non_package_directories_are_not_discovered() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(
            &root.join("blog/templatetags/not_a_package/deep.py"),
            r"
from django import template

register = template.Library()
",
        );

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(true))]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        assert!(!loadable_map(&facts).contains_key("not_a_package.deep"));
    }

    #[test]
    fn duplicate_app_load_names_choose_later_app_and_record_reason() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_template_library(&root, "app1", "shared_tags");
        write_template_library(&root, "app2", "shared_tags");

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(true))]),
            &Fact::known(vec![app(&root, "app1"), app(&root, "app2")]),
        );

        let (libraries, reasons) = partial_vec(&facts);
        let loadable = Fact::known(libraries);
        assert_eq!(
            loadable_map(&loadable)
                .get("shared_tags")
                .map(String::as_str),
            Some("app2.templatetags.shared_tags")
        );
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("shared_tags")));
    }

    #[test]
    fn app_load_names_can_override_django_defaults_with_reason() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_template_library(&root, "blog", "static");

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(true))]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        let (libraries, reasons) = partial_vec(&facts);
        let loadable = Fact::known(libraries);
        assert_eq!(
            loadable_map(&loadable).get("static").map(String::as_str),
            Some("blog.templatetags.static")
        );
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("django.templatetags.static")));
    }

    #[test]
    fn partial_app_registry_keeps_known_libraries_and_reasons() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_template_library(&root, "blog", "blog_tags");
        let reason = app_registry_reason();

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(true))]),
            &Fact::partial(vec![app(&root, "blog")], vec![reason.clone()]),
        );

        let (_libraries, reasons) = partial_vec(&facts);
        assert_eq!(
            loadable_map(&facts).get("blog_tags").map(String::as_str),
            Some("blog.templatetags.blog_tags")
        );
        assert_eq!(reasons, [reason]);
    }

    #[test]
    fn unknown_app_registry_keeps_default_libraries_partial() {
        let reason = app_registry_reason();

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(true))]),
            &Fact::unknown(vec![reason.clone()]),
        );

        let (_libraries, reasons) = partial_vec(&facts);
        assert_eq!(
            loadable_map(&facts).get("cache").map(String::as_str),
            Some("django.templatetags.cache")
        );
        assert_eq!(reasons, [reason]);
    }

    #[test]
    fn partial_option_libraries_preserve_values_and_reasons() {
        let reason = option_reason();

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend_with_options(
                Fact::partial(
                    vec![settings_library("custom", "project.templatetags.custom")],
                    vec![reason.clone()],
                ),
                Fact::known(Vec::new()),
            )]),
            &Fact::known(Vec::new()),
        );

        let (_libraries, reasons) = partial_vec(&facts);
        assert_eq!(
            loadable_map(&facts).get("custom").map(String::as_str),
            Some("project.templatetags.custom")
        );
        assert_eq!(reasons, [reason]);
    }

    #[test]
    fn unknown_template_backends_are_unknown_template_libraries() {
        let reason = Reason::new(
            Field::SettingsTemplates,
            ReasonSource::Unknown,
            "TEMPLATES is not assigned in this settings file",
        );

        let facts = assemble_template_libraries(
            &Fact::unknown(vec![reason.clone()]),
            &Fact::known(Vec::new()),
        );

        assert_eq!(facts.reasons(), &[reason]);
        assert!(matches!(facts, Fact::Unknown { .. }));
    }

    #[test]
    fn skips_non_django_backends_without_blocking_django_backend() {
        let mut jinja_backend = backend(Fact::known(false));
        jinja_backend.backend = Some("django.template.backends.jinja2.Jinja2".to_string());
        let facts = assemble_template_libraries(
            &Fact::known(vec![
                jinja_backend,
                backend_with_options(
                    Fact::known(vec![settings_library(
                        "custom",
                        "project.templatetags.custom",
                    )]),
                    Fact::known(Vec::new()),
                ),
            ]),
            &Fact::known(Vec::new()),
        );

        assert_eq!(
            loadable_map(&facts).get("custom").map(String::as_str),
            Some("project.templatetags.custom")
        );
    }

    #[test]
    fn missing_backend_preserves_statically_known_options_as_partial() {
        let mut unknown_backend = backend_with_options(
            Fact::known(vec![settings_library(
                "custom",
                "project.templatetags.custom",
            )]),
            Fact::known(vec![module("project.templatetags.custom_builtins")]),
        );
        unknown_backend.backend = None;

        let facts = assemble_template_libraries(
            &Fact::known(vec![unknown_backend]),
            &Fact::known(Vec::new()),
        );

        let (_libraries, reasons) = partial_vec(&facts);
        let loadable = loadable_map(&facts);
        assert_eq!(
            loadable.get("custom").map(String::as_str),
            Some("project.templatetags.custom")
        );
        assert!(!loadable.contains_key("cache"));
        assert_eq!(
            builtin_modules(&facts),
            ["project.templatetags.custom_builtins"]
        );
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("BACKEND is not known")));
    }

    #[test]
    fn multiple_django_backends_do_not_let_later_defaults_override_settings_libraries() {
        let facts = assemble_template_libraries(
            &Fact::known(vec![
                backend_with_options(
                    Fact::known(vec![settings_library(
                        "static",
                        "project.templatetags.assets",
                    )]),
                    Fact::known(Vec::new()),
                ),
                backend(Fact::known(false)),
            ]),
            &Fact::known(Vec::new()),
        );

        assert_eq!(
            loadable_map(&facts).get("static").map(String::as_str),
            Some("project.templatetags.assets")
        );
    }

    #[test]
    fn later_backend_options_override_earlier_backend_options() {
        let facts = assemble_template_libraries(
            &Fact::known(vec![
                backend_with_options(
                    Fact::known(vec![settings_library("custom", "project.templatetags.one")]),
                    Fact::known(Vec::new()),
                ),
                backend_with_options(
                    Fact::known(vec![settings_library("custom", "project.templatetags.two")]),
                    Fact::known(Vec::new()),
                ),
            ]),
            &Fact::known(Vec::new()),
        );

        assert_eq!(
            loadable_map(&facts).get("custom").map(String::as_str),
            Some("project.templatetags.two")
        );
    }

    #[test]
    fn multiple_django_backends_deduplicate_default_builtins() {
        let facts = assemble_template_libraries(
            &Fact::known(vec![
                backend(Fact::known(false)),
                backend(Fact::known(false)),
            ]),
            &Fact::known(Vec::new()),
        );

        assert_eq!(
            builtin_modules(&facts),
            [
                "django.template.defaulttags",
                "django.template.defaultfilters",
                "django.template.loader_tags",
            ]
        );
    }

    #[test]
    fn assembles_libraries_from_extracted_settings_and_resolved_apps() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("blog/__init__.py"), "");
        write_template_library(&root, "blog", "blog_tags");
        write_file(
            &root.join("settings.py"),
            r#"
INSTALLED_APPS = ["blog"]
TEMPLATES = [{
    "BACKEND": "django.template.backends.django.DjangoTemplates",
    "APP_DIRS": False,
    "OPTIONS": {
        "libraries": {"alias": "blog.templatetags.blog_tags"},
        "builtins": ["blog.templatetags.blog_tags"],
    },
}]
"#,
        );

        let settings = extract_settings_facts(&root.join("settings.py"));
        let app_registry =
            resolve_app_registry(&settings.installed_apps, &root, &search_paths(&root));
        let facts =
            assemble_template_libraries(&settings.template_backends, &app_registry.app_registry);

        let loadable = loadable_map(&facts);
        assert_eq!(
            loadable.get("blog_tags").map(String::as_str),
            Some("blog.templatetags.blog_tags")
        );
        assert_eq!(
            loadable.get("alias").map(String::as_str),
            Some("blog.templatetags.blog_tags")
        );
        assert_eq!(
            builtin_modules(&facts).last().map(String::as_str),
            Some("blog.templatetags.blog_tags")
        );
    }

    #[test]
    fn uses_app_config_path_when_discovering_templatetags() {
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
        write_file(
            &root.join("custom_blog/templatetags/custom_tags.py"),
            r"
from django import template

register = template.Library()
",
        );

        let app_registry = resolve_app_registry(
            &Fact::known(vec!["blog.apps.BlogConfig".to_string()]),
            &root,
            &search_paths(&root),
        );
        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(false))]),
            &app_registry.app_registry,
        );

        assert_eq!(
            loadable_map(&facts).get("custom_tags").map(String::as_str),
            Some("blog.templatetags.custom_tags")
        );
    }

    #[test]
    fn settings_builtins_duplicate_default_modules_keep_first_builtin() {
        let facts = assemble_template_libraries(
            &Fact::known(vec![backend_with_options(
                Fact::known(Vec::new()),
                Fact::known(vec![module("django.template.defaulttags")]),
            )]),
            &Fact::known(Vec::new()),
        );

        let libraries = known_vec(&facts);
        let defaulttags = libraries
            .iter()
            .find(|library| library.module == module("django.template.defaulttags"))
            .unwrap();
        assert!(matches!(
            defaulttags.source,
            TemplateLibrarySource::DjangoDefaultBuiltin
        ));
    }

    #[test]
    fn malformed_templatetag_module_makes_fact_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(
            &root.join("blog/templatetags/broken.py"),
            "from django import template\nregister = template.Library(\n",
        );

        let facts = assemble_template_libraries(
            &Fact::known(vec![backend(Fact::known(false))]),
            &Fact::known(vec![app(&root, "blog")]),
        );

        let (_libraries, reasons) = partial_vec(&facts);
        assert!(reasons
            .iter()
            .any(|reason| reason.message.contains("failed to parse")));
    }

    #[test]
    fn template_dirs_field_is_not_required_for_library_assembly() {
        let facts = assemble_template_libraries(
            &Fact::known(vec![TemplateBackendFact {
                backend: Some(DJANGO_TEMPLATES_BACKEND.to_string()),
                dirs: Fact::partial(
                    vec![TemplateDirFact {
                        path: Utf8PathBuf::from("templates"),
                        source: TemplateDirSource::SettingsDir,
                    }],
                    vec![Reason::path(
                        Field::TemplateDirs,
                        "templates",
                        "template dir uncertainty should not affect libraries",
                    )],
                ),
                app_dirs: Fact::unknown(vec![Reason::new(
                    Field::SettingsTemplates,
                    ReasonSource::Unknown,
                    "APP_DIRS is dynamic but irrelevant for libraries",
                )]),
                option_libraries: Fact::known(Vec::new()),
                option_builtins: Fact::known(Vec::new()),
            }]),
            &Fact::known(Vec::new()),
        );

        assert!(matches!(facts, Fact::Known { .. }));
    }
}
