use ruff_python_ast as ast;

use crate::ast::ExprExt;
use crate::python::PythonModuleName;
use crate::settings::extraction::bindings::ExtractedList;
use crate::settings::extraction::env::EvalEnv;
use crate::settings::types::TemplateBackend;
use crate::settings::types::TemplateDirPath;

pub(super) enum AssignmentEffect {
    Assign(Vec<TemplateBackend>, AssignmentCompleteness),
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AssignmentCompleteness {
    Full,
    Partial,
}

pub(super) enum DirsExtensionEffect {
    Extend(Vec<TemplateDirPath>),
    Partial,
}

pub(super) fn evaluate_assignment(value: &ast::Expr, env: &EvalEnv<'_>) -> AssignmentEffect {
    let ast::Expr::List(list) = value else {
        return AssignmentEffect::Unsupported;
    };

    let mut backends = Vec::new();
    let mut completeness = AssignmentCompleteness::Full;
    for element in &list.elts {
        let ast::Expr::Dict(dict) = element else {
            completeness = AssignmentCompleteness::Partial;
            continue;
        };
        let backend = evaluate_template_backend(dict, env);
        if !backend.is_fully_extracted() {
            completeness = AssignmentCompleteness::Partial;
        }
        backends.push(backend);
    }

    AssignmentEffect::Assign(backends, completeness)
}

pub(super) fn evaluate_dirs_extension(value: &ast::Expr, env: &EvalEnv<'_>) -> DirsExtensionEffect {
    let ast::Expr::List(list) = value else {
        return DirsExtensionEffect::Partial;
    };

    DirsExtensionEffect::Extend(
        list.elts
            .iter()
            .map(|element| env.evaluate_template_dir_path(element))
            .collect(),
    )
}

fn evaluate_template_backend(dict: &ast::ExprDict, env: &EvalEnv<'_>) -> TemplateBackend {
    let mut backend = TemplateBackend::default();
    for item in &dict.items {
        let Some(key_expr) = &item.key else {
            backend.mark_partial();
            continue;
        };
        let Some(key) = key_expr.string_literal() else {
            backend.mark_partial();
            continue;
        };
        match key {
            "BACKEND" => match item.value.string_literal() {
                Some(value) => backend.backend = Some(value.to_string()),
                None => backend.mark_partial(),
            },
            "DIRS" => evaluate_template_dirs(&item.value, env, &mut backend),
            "APP_DIRS" => match item.value.bool_literal() {
                Some(value) => backend.app_dirs = Some(value),
                None => backend.mark_partial(),
            },
            "OPTIONS" => evaluate_template_options(&item.value, &mut backend),
            _ => {}
        }
    }
    backend
}

fn evaluate_template_dirs(value: &ast::Expr, env: &EvalEnv<'_>, backend: &mut TemplateBackend) {
    let ast::Expr::List(list) = value else {
        backend.mark_partial();
        return;
    };
    for element in &list.elts {
        let path = env.evaluate_template_dir_path(element);
        if path == TemplateDirPath::Unknown {
            backend.mark_partial();
        }
        backend.dirs.push(path);
    }
}

fn evaluate_template_options(value: &ast::Expr, backend: &mut TemplateBackend) {
    let ast::Expr::Dict(dict) = value else {
        backend.mark_partial();
        return;
    };

    for item in &dict.items {
        let Some(key_expr) = &item.key else {
            backend.mark_partial();
            continue;
        };
        let Some(key) = key_expr.string_literal() else {
            backend.mark_partial();
            continue;
        };
        match key {
            "libraries" => {
                let libraries = extract_template_library_dict(&item.value);
                backend.libraries.extend(libraries.values);
                if !libraries.status.is_complete() {
                    backend.mark_partial();
                }
            }
            "builtins" => {
                let builtins = extract_python_module_name_list(&item.value);
                backend.builtins.extend(builtins.values);
                if !builtins.status.is_complete() {
                    backend.mark_partial();
                }
            }
            _ => {}
        }
    }
}

fn extract_template_library_dict(value: &ast::Expr) -> ExtractedList<(String, PythonModuleName)> {
    let ast::Expr::Dict(dict) = value else {
        return ExtractedList::incomplete(Vec::new());
    };

    let mut extracted = ExtractedList::complete(Vec::new());
    for item in &dict.items {
        match (
            item.key.as_ref().and_then(ExprExt::string_literal),
            item.value.string_literal(),
        ) {
            (Some(key), Some(value)) => match PythonModuleName::parse(value) {
                Ok(module_name) => extracted.values.push((key.to_string(), module_name)),
                Err(_) => extracted.status.mark_incomplete(),
            },
            _ => extracted.status.mark_incomplete(),
        }
    }
    extracted
}

fn extract_python_module_name_list(value: &ast::Expr) -> ExtractedList<PythonModuleName> {
    let ast::Expr::List(list) = value else {
        return ExtractedList::incomplete(Vec::new());
    };

    let mut extracted = ExtractedList::complete(Vec::new());
    for element in &list.elts {
        let Some(value) = element.string_literal() else {
            extracted.status.mark_incomplete();
            continue;
        };
        match PythonModuleName::parse(value) {
            Ok(module_name) => extracted.values.push(module_name),
            Err(_) => extracted.status.mark_incomplete(),
        }
    }
    extracted
}
