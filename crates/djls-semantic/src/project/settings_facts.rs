//! Django settings fact extraction.
//!
//! This module extracts a narrow Tier 1 subset from a single settings file:
//! literal `INSTALLED_APPS`, literal `TEMPLATES`, and simple template directory
//! path expressions. It reports unsupported settings shapes as partial or
//! unknown facts instead of importing the settings module.

#![allow(
    dead_code,
    reason = "Milestone A4 adds settings facts before project facts are assembled."
)]

use std::collections::BTreeMap;
use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use ruff_python_ast::Expr;
use ruff_python_ast::Number;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;

use crate::project::facts::Fact;
use crate::project::facts::Field;
use crate::project::facts::Reason;
use crate::project::facts::SettingsFacts;
use crate::project::facts::TemplateBackendFact;
use crate::project::facts::TemplateDirFact;
use crate::project::facts::TemplateDirSource;
use crate::project::facts::TemplateLibraryFact;
use crate::project::facts::TemplateLibrarySource;
use crate::project::names::LibraryName;
use crate::project::names::PyModuleName;

const INSTALLED_APPS: &str = "INSTALLED_APPS";
const TEMPLATES: &str = "TEMPLATES";

#[must_use]
pub(crate) fn extract_settings_facts(file: &Utf8Path) -> SettingsFacts {
    let source = match fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) => {
            return unknown_settings_facts(file, format!("failed to read settings file: {error}"));
        }
    };

    let module = match ruff_python_parser::parse_module(&source) {
        Ok(parsed) => parsed.into_syntax(),
        Err(error) => {
            return unknown_settings_facts(file, format!("failed to parse settings file: {error}"));
        }
    };

    let mut path_values = BTreeMap::new();
    let mut installed_apps = None;
    let mut template_backends = None;

    for stmt in &module.body {
        record_path_assignment(stmt, &mut path_values, file);

        if let Some(value) = assigned_value(stmt, INSTALLED_APPS) {
            installed_apps = Some(extract_string_list(
                value,
                Field::SettingsInstalledApps,
                file,
                "INSTALLED_APPS",
            ));
            continue;
        }

        if let Some(value) = assigned_value(stmt, TEMPLATES) {
            template_backends = Some(extract_template_backends(value, &path_values, file));
            continue;
        }

        if is_unsupported_mutation(stmt, INSTALLED_APPS) {
            let mutation_reason = reason(
                Field::SettingsInstalledApps,
                file,
                "unsupported dynamic mutation of INSTALLED_APPS",
            );
            installed_apps = Some(add_reason_or_unknown(installed_apps, mutation_reason));
        }

        if is_unsupported_mutation(stmt, TEMPLATES) {
            let mutation_reason = reason(
                Field::SettingsTemplates,
                file,
                "unsupported dynamic mutation of TEMPLATES",
            );
            template_backends = Some(add_reason_or_unknown(template_backends, mutation_reason));
        }
    }

    SettingsFacts {
        file: file.to_path_buf(),
        installed_apps: installed_apps.unwrap_or_else(|| {
            Fact::unknown(vec![reason(
                Field::SettingsInstalledApps,
                file,
                "INSTALLED_APPS is not assigned in this settings file",
            )])
        }),
        template_backends: template_backends.unwrap_or_else(|| {
            Fact::unknown(vec![reason(
                Field::SettingsTemplates,
                file,
                "TEMPLATES is not assigned in this settings file",
            )])
        }),
    }
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

fn record_path_assignment(
    stmt: &Stmt,
    path_values: &mut BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) {
    let Some((name, value)) = simple_assignment(stmt) else {
        return;
    };
    let Some(path) = evaluate_path(value, path_values, settings_file) else {
        return;
    };
    path_values.insert(name.to_string(), path);
}

fn simple_assignment(stmt: &Stmt) -> Option<(&str, &Expr)> {
    match stmt {
        Stmt::Assign(assign) => {
            let target = assign.targets.first()?;
            let Expr::Name(name) = target else {
                return None;
            };
            Some((name.id.as_str(), assign.value.as_ref()))
        }
        Stmt::AnnAssign(assign) => {
            let Expr::Name(name) = assign.target.as_ref() else {
                return None;
            };
            Some((name.id.as_str(), assign.value.as_deref()?))
        }
        _ => None,
    }
}

fn is_unsupported_mutation(stmt: &Stmt, target_name: &str) -> bool {
    match stmt {
        Stmt::AugAssign(assign) => is_name(assign.target.as_ref(), target_name),
        Stmt::Expr(expr_stmt) => {
            let Expr::Call(call) = expr_stmt.value.as_ref() else {
                return false;
            };
            let Expr::Attribute(attribute) = call.func.as_ref() else {
                return false;
            };
            matches!(
                attribute.attr.as_str(),
                "append" | "extend" | "insert" | "update"
            ) && is_name(attribute.value.as_ref(), target_name)
        }
        _ => false,
    }
}

fn is_name(expr: &Expr, expected: &str) -> bool {
    matches!(expr, Expr::Name(name) if name.id.as_str() == expected)
}

fn extract_string_list(
    expr: &Expr,
    field: Field,
    file: &Utf8Path,
    setting_name: &str,
) -> Fact<Vec<String>> {
    let Some(elements) = collection_elements(expr) else {
        return Fact::unknown(vec![reason(
            field,
            file,
            format!("{setting_name} must be assigned a literal list or tuple of strings"),
        )]);
    };

    let mut values = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        if let Some(value) = string_literal(element) {
            values.push(value);
        } else {
            reasons.push(reason(
                field,
                file,
                format!("{setting_name} contains a non-string entry"),
            ));
        }
    }

    known_or_partial(values, reasons)
}

fn extract_template_backends(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
) -> Fact<Vec<TemplateBackendFact>> {
    let Some(elements) = collection_elements(expr) else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplates,
            file,
            "TEMPLATES must be assigned a literal list or tuple of dictionaries",
        )]);
    };

    let mut backends = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        let Expr::Dict(dict) = element else {
            reasons.push(reason(
                Field::SettingsTemplates,
                file,
                "TEMPLATES contains a non-dictionary backend entry",
            ));
            continue;
        };
        backends.push(extract_template_backend(
            dict,
            path_values,
            file,
            &mut reasons,
        ));
    }

    known_or_partial(backends, reasons)
}

fn extract_template_backend(
    dict: &ruff_python_ast::ExprDict,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
    reasons: &mut Vec<Reason>,
) -> TemplateBackendFact {
    let backend = match dict_value(dict, "BACKEND") {
        Some(value) => {
            if let Some(backend) = string_literal(value) {
                Some(backend)
            } else {
                reasons.push(reason(
                    Field::SettingsTemplates,
                    file,
                    "TEMPLATES BACKEND must be a string literal",
                ));
                None
            }
        }
        None => None,
    };

    let dirs = match dict_value(dict, "DIRS") {
        Some(value) => extract_template_dirs(value, path_values, file),
        None => Fact::known(Vec::new()),
    };

    let app_dirs = match dict_value(dict, "APP_DIRS") {
        Some(Expr::BooleanLiteral(boolean)) => Fact::known(boolean.value),
        Some(_) => Fact::unknown(vec![reason(
            Field::SettingsTemplates,
            file,
            "TEMPLATES APP_DIRS must be a boolean literal",
        )]),
        None => Fact::known(false),
    };

    let (option_libraries, option_builtins) = match dict_value(dict, "OPTIONS") {
        Some(Expr::Dict(options)) => (
            extract_option_libraries(options, file),
            extract_option_builtins(options, file),
        ),
        Some(_) => {
            let option_reason = reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS must be a dictionary literal",
            );
            (
                Fact::unknown(vec![option_reason.clone()]),
                Fact::unknown(vec![option_reason]),
            )
        }
        None => (Fact::known(Vec::new()), Fact::known(Vec::new())),
    };

    TemplateBackendFact {
        backend,
        dirs,
        app_dirs,
        option_libraries,
        option_builtins,
    }
}

fn extract_template_dirs(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    file: &Utf8Path,
) -> Fact<Vec<TemplateDirFact>> {
    let Some(elements) = collection_elements(expr) else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplateDirs,
            file,
            "TEMPLATES DIRS must be a literal list or tuple",
        )]);
    };

    let mut dirs = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        if let Some(path) = evaluate_path(element, path_values, file) {
            dirs.push(TemplateDirFact {
                path,
                source: TemplateDirSource::SettingsDir,
            });
        } else {
            reasons.push(reason(
                Field::SettingsTemplateDirs,
                file,
                "TEMPLATES DIRS contains an unsupported path expression",
            ));
        }
    }

    known_or_partial(dirs, reasons)
}

fn extract_option_libraries(
    options: &ruff_python_ast::ExprDict,
    file: &Utf8Path,
) -> Fact<Vec<TemplateLibraryFact>> {
    let Some(value) = dict_value(options, "libraries") else {
        return Fact::known(Vec::new());
    };
    let Expr::Dict(libraries) = value else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplateOptions,
            file,
            "TEMPLATES OPTIONS.libraries must be a dictionary literal",
        )]);
    };

    let mut facts = Vec::new();
    let mut reasons = Vec::new();
    for item in &libraries.items {
        let Some(key) = item.key.as_ref() else {
            reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS.libraries contains dictionary unpacking",
            ));
            continue;
        };
        let Some(load_name) = string_literal(key) else {
            reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS.libraries contains a non-string load name",
            ));
            continue;
        };
        let Some(module) = string_literal(&item.value) else {
            reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS.libraries contains a non-string module path",
            ));
            continue;
        };

        match (LibraryName::parse(&load_name), PyModuleName::parse(&module)) {
            (Ok(load_name), Ok(module)) => facts.push(TemplateLibraryFact {
                load_name,
                module,
                source: TemplateLibrarySource::SettingsLibraries,
            }),
            (Err(error), _) => reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                format!("invalid TEMPLATES OPTIONS.libraries load name: {error}"),
            )),
            (_, Err(error)) => reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                format!("invalid TEMPLATES OPTIONS.libraries module path: {error}"),
            )),
        }
    }

    known_or_partial(facts, reasons)
}

fn extract_option_builtins(
    options: &ruff_python_ast::ExprDict,
    file: &Utf8Path,
) -> Fact<Vec<PyModuleName>> {
    let Some(value) = dict_value(options, "builtins") else {
        return Fact::known(Vec::new());
    };
    let Some(elements) = collection_elements(value) else {
        return Fact::unknown(vec![reason(
            Field::SettingsTemplateOptions,
            file,
            "TEMPLATES OPTIONS.builtins must be a literal list or tuple",
        )]);
    };

    let mut builtins = Vec::new();
    let mut reasons = Vec::new();
    for element in elements {
        let Some(module) = string_literal(element) else {
            reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                "TEMPLATES OPTIONS.builtins contains a non-string module path",
            ));
            continue;
        };
        match PyModuleName::parse(&module) {
            Ok(module) => builtins.push(module),
            Err(error) => reasons.push(reason(
                Field::SettingsTemplateOptions,
                file,
                format!("invalid TEMPLATES OPTIONS.builtins module path: {error}"),
            )),
        }
    }

    known_or_partial(builtins, reasons)
}

fn string_literal(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(string) = expr {
        return Some(string.value.to_str().to_string());
    }
    None
}

fn collection_elements(expr: &Expr) -> Option<&[Expr]> {
    match expr {
        Expr::List(list) => Some(list.elts.as_slice()),
        Expr::Tuple(tuple) => Some(tuple.elts.as_slice()),
        _ => None,
    }
}

fn dict_value<'a>(dict: &'a ruff_python_ast::ExprDict, key: &str) -> Option<&'a Expr> {
    dict.items.iter().find_map(|item| {
        let item_key = string_literal(item.key.as_ref()?)?;
        (item_key == key).then_some(&item.value)
    })
}

fn evaluate_path(
    expr: &Expr,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) -> Option<Utf8PathBuf> {
    match expr {
        Expr::StringLiteral(string) => Some(Utf8PathBuf::from(string.value.to_str())),
        Expr::Name(name) if name.id.as_str() == "__file__" => Some(settings_file.to_path_buf()),
        Expr::Name(name) => path_values.get(name.id.as_str()).cloned(),
        Expr::BinOp(binop) if binop.op == Operator::Div => {
            let left = evaluate_path(binop.left.as_ref(), path_values, settings_file)?;
            let right = evaluate_path(binop.right.as_ref(), path_values, settings_file)?;
            Some(left.join(right))
        }
        Expr::Attribute(attribute) if attribute.attr.as_str() == "parent" => {
            let value = evaluate_path(attribute.value.as_ref(), path_values, settings_file)?;
            value.parent().map(Utf8Path::to_path_buf)
        }
        Expr::Subscript(subscript) => {
            evaluate_path_parents_subscript(subscript, path_values, settings_file)
        }
        Expr::Call(call) => evaluate_path_call(call, path_values, settings_file),
        _ => None,
    }
}

fn evaluate_path_call(
    call: &ruff_python_ast::ExprCall,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) -> Option<Utf8PathBuf> {
    match call.func.as_ref() {
        Expr::Name(name) if name.id.as_str() == "Path" => {
            let first = call.arguments.args.first()?;
            evaluate_path(first, path_values, settings_file)
        }
        Expr::Attribute(attribute) if attribute.attr.as_str() == "resolve" => {
            evaluate_path(attribute.value.as_ref(), path_values, settings_file)
        }
        Expr::Attribute(attribute) if attribute.attr.as_str() == "joinpath" => {
            let mut path = evaluate_path(attribute.value.as_ref(), path_values, settings_file)?;
            for arg in &call.arguments.args {
                let part = evaluate_path(arg, path_values, settings_file)?;
                path = path.join(part);
            }
            Some(path)
        }
        _ if dotted_name(call.func.as_ref()).as_deref() == Some("os.path.join") => {
            let mut args = call.arguments.args.iter();
            let mut path = evaluate_path(args.next()?, path_values, settings_file)?;
            for arg in args {
                let part = evaluate_path(arg, path_values, settings_file)?;
                path = path.join(part);
            }
            Some(path)
        }
        _ => None,
    }
}

fn evaluate_path_parents_subscript(
    subscript: &ruff_python_ast::ExprSubscript,
    path_values: &BTreeMap<String, Utf8PathBuf>,
    settings_file: &Utf8Path,
) -> Option<Utf8PathBuf> {
    let Expr::Attribute(attribute) = subscript.value.as_ref() else {
        return None;
    };
    if attribute.attr.as_str() != "parents" {
        return None;
    }

    let mut path = evaluate_path(attribute.value.as_ref(), path_values, settings_file)?;
    let parent_index = integer_literal(subscript.slice.as_ref())?;
    for _ in 0..=parent_index {
        path = path.parent()?.to_path_buf();
    }
    Some(path)
}

fn integer_literal(expr: &Expr) -> Option<usize> {
    let Expr::NumberLiteral(number) = expr else {
        return None;
    };
    let Number::Int(value) = &number.value else {
        return None;
    };
    value.as_usize()
}

fn dotted_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.id.to_string()),
        Expr::Attribute(attribute) => Some(format!(
            "{}.{}",
            dotted_name(attribute.value.as_ref())?,
            attribute.attr.as_str()
        )),
        _ => None,
    }
}

fn known_or_partial<T>(values: Vec<T>, reasons: Vec<Reason>) -> Fact<Vec<T>> {
    if reasons.is_empty() {
        Fact::known(values)
    } else {
        Fact::partial(values, reasons)
    }
}

fn add_reason_or_unknown<T>(fact: Option<Fact<T>>, reason: Reason) -> Fact<T> {
    match fact {
        Some(fact) => fact.with_reason(reason),
        None => Fact::unknown(vec![reason]),
    }
}

fn unknown_settings_facts(file: &Utf8Path, message: impl Into<String>) -> SettingsFacts {
    let message = message.into();
    SettingsFacts {
        file: file.to_path_buf(),
        installed_apps: Fact::unknown(vec![reason(
            Field::SettingsInstalledApps,
            file,
            message.clone(),
        )]),
        template_backends: Fact::unknown(vec![reason(Field::SettingsTemplates, file, message)]),
    }
}

fn reason(field: Field, file: &Utf8Path, message: impl Into<String>) -> Reason {
    Reason::file(field, file, message)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use djls_conf::Settings;
    use tempfile::tempdir;

    use super::*;
    use crate::project::django_environments::discover_django_environments;
    use crate::project::module_resolver::discover_module_search_paths;
    use crate::project::names::LibraryName;

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn write_settings(root: &Utf8Path, source: &str) -> Utf8PathBuf {
        let path = root.join("project/settings.py");
        write_file(&path, source);
        path
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

    fn known_bool(fact: &Fact<bool>) -> bool {
        let Fact::Known { value } = fact else {
            panic!("expected known bool, got {fact:?}");
        };
        *value
    }

    fn unknown_reasons<T: std::fmt::Debug>(fact: &Fact<T>) -> Vec<Reason> {
        let Fact::Unknown { reasons } = fact else {
            panic!("expected unknown fact, got {fact:?}");
        };
        reasons.clone()
    }

    #[test]
    fn extracts_literal_installed_apps_and_templates() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = [
    "django.contrib.auth",
    "project.app",
]
TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates", BASE_DIR.joinpath("more_templates")],
        "APP_DIRS": True,
        "OPTIONS": {
            "libraries": {
                "custom_tags": "project.templatetags.custom_tags",
            },
            "builtins": ["project.templatetags.builtins"],
        },
    }
]
"#,
        );

        let facts = extract_settings_facts(&settings);

        assert_eq!(
            known_vec(&facts.installed_apps),
            ["django.contrib.auth", "project.app"]
        );
        let backends = known_vec(&facts.template_backends);
        assert_eq!(backends.len(), 1);
        assert_eq!(
            backends[0].backend.as_deref(),
            Some("django.template.backends.django.DjangoTemplates")
        );
        assert!(known_bool(&backends[0].app_dirs));
        assert_eq!(
            known_vec(&backends[0].dirs)
                .into_iter()
                .map(|dir| dir.path)
                .collect::<Vec<_>>(),
            [root.join("templates"), root.join("more_templates")]
        );
        assert_eq!(
            known_vec(&backends[0].option_libraries)[0].load_name,
            LibraryName::parse("custom_tags").unwrap()
        );
        assert_eq!(
            known_vec(&backends[0].option_builtins),
            [PyModuleName::parse("project.templatetags.builtins").unwrap()]
        );
    }

    #[test]
    fn extracts_typed_settings_assignments() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
INSTALLED_APPS: list[str] = ["django.contrib.auth"]
TEMPLATES: list[dict] = []
"#,
        );

        let facts = extract_settings_facts(&settings);

        assert_eq!(known_vec(&facts.installed_apps), ["django.contrib.auth"]);
        assert!(known_vec(&facts.template_backends).is_empty());
    }

    #[test]
    fn extracts_pathlib_parents_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parents[1]
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [BASE_DIR / "templates"]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(known_vec(&backends[0].dirs)[0].path, root.join("templates"));
    }

    #[test]
    fn extracts_pathlib_parents_zero_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path

PROJECT_DIR = Path(__file__).resolve().parents[0]
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [PROJECT_DIR / "templates"]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(
            known_vec(&backends[0].dirs)[0].path,
            root.join("project/templates")
        );
    }

    #[test]
    fn marks_out_of_range_pathlib_parents_template_dirs_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parents[100]
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [BASE_DIR / "templates"]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);
        let (dirs, reasons) = partial_vec(&backends[0].dirs);

        assert!(dirs.is_empty());
        assert!(reasons[0].message.contains("unsupported path expression"));
    }

    #[test]
    fn marks_unreadable_settings_file_unknown() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let facts = extract_settings_facts(&root.join("project/missing.py"));

        let reasons = unknown_reasons(&facts.installed_apps);
        assert!(reasons[0].message.contains("failed to read settings file"));
        let reasons = unknown_reasons(&facts.template_backends);
        assert!(reasons[0].message.contains("failed to read settings file"));
    }

    #[test]
    fn marks_parse_error_settings_file_unknown() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(&root, "INSTALLED_APPS = [\n");

        let facts = extract_settings_facts(&settings);

        let reasons = unknown_reasons(&facts.installed_apps);
        assert!(reasons[0].message.contains("failed to parse settings file"));
        let reasons = unknown_reasons(&facts.template_backends);
        assert!(reasons[0].message.contains("failed to parse settings file"));
    }

    #[test]
    fn extracts_settings_facts_from_resolved_settings_file() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&root.join("project/__init__.py"), "");
        write_file(
            &root.join("project/settings.py"),
            r#"
INSTALLED_APPS = ["project.app"]
TEMPLATES = [{"DIRS": ["templates"]}]
"#,
        );
        write_file(
            &root.join("djls.toml"),
            r#"django_settings_module = "project.settings""#,
        );

        let settings = Settings::new(&root, None).unwrap();
        let search_paths = discover_module_search_paths(&root, &[], &[])
            .value()
            .cloned()
            .unwrap();
        let environments = discover_django_environments(&root, &settings, &search_paths);
        let Fact::Known {
            value: django_settings,
        } = &environments[0].django_settings
        else {
            panic!(
                "expected resolved settings module, got {:?}",
                environments[0].django_settings
            );
        };

        let facts = extract_settings_facts(&django_settings.file);

        assert_eq!(known_vec(&facts.installed_apps), ["project.app"]);
        assert_eq!(known_vec(&facts.template_backends).len(), 1);
    }

    #[test]
    fn marks_unsupported_installed_apps_entries_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
LOCAL_APPS = ["project.app"]
INSTALLED_APPS = ["django.contrib.auth", *LOCAL_APPS]
TEMPLATES = []
"#,
        );

        let facts = extract_settings_facts(&settings);

        let (apps, reasons) = partial_vec(&facts.installed_apps);
        assert_eq!(apps, ["django.contrib.auth"]);
        assert_eq!(reasons[0].field, Field::SettingsInstalledApps);
        assert!(reasons[0].message.contains("non-string"));
    }

    #[test]
    fn marks_dynamic_settings_mutations_partial() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
INSTALLED_APPS = ["django.contrib.auth"]
INSTALLED_APPS.append("project.dynamic")
TEMPLATES = []
TEMPLATES += [{"APP_DIRS": True}]
"#,
        );

        let facts = extract_settings_facts(&settings);

        let (apps, app_reasons) = partial_vec(&facts.installed_apps);
        assert_eq!(apps, ["django.contrib.auth"]);
        assert!(app_reasons[0]
            .message
            .contains("unsupported dynamic mutation"));

        let (templates, template_reasons) = partial_vec(&facts.template_backends);
        assert!(templates.is_empty());
        assert!(template_reasons[0]
            .message
            .contains("unsupported dynamic mutation"));
    }

    #[test]
    fn extracts_os_path_join_template_dirs() {
        let tmp = tempdir().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let settings = write_settings(
            &root,
            r#"
from pathlib import Path
import os

BASE_DIR = Path(__file__).resolve().parent.parent
INSTALLED_APPS = []
TEMPLATES = [{"DIRS": [os.path.join(BASE_DIR, "templates")]}]
"#,
        );

        let facts = extract_settings_facts(&settings);
        let backends = known_vec(&facts.template_backends);

        assert_eq!(known_vec(&backends[0].dirs)[0].path, root.join("templates"));
    }
}
