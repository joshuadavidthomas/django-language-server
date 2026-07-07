use djls_source::Spanned;
use ruff_python_ast as ast;

use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::settings::extraction::AssignmentCompleteness;
use crate::settings::extraction::bindings::ExtractedList;
use crate::settings::extraction::env::EvalEnv;
use crate::settings::types::TemplateDirPath;

pub(super) fn evaluate_static_url_assignment(
    value: &ast::Expr,
) -> Option<(Spanned<String>, AssignmentCompleteness)> {
    let value_string = value.string_literal()?;

    Some((
        Spanned::new(value_string.to_string(), value.span()),
        AssignmentCompleteness::Full,
    ))
}

pub(super) fn evaluate_static_root_assignment(
    value: &ast::Expr,
    env: &EvalEnv<'_>,
) -> (Spanned<TemplateDirPath>, AssignmentCompleteness) {
    let path = env.evaluate_template_dir_path(value);
    let completeness = if path == TemplateDirPath::Unknown {
        AssignmentCompleteness::Partial
    } else {
        AssignmentCompleteness::Full
    };

    (Spanned::new(path, value.span()), completeness)
}

pub(super) fn evaluate_staticfiles_dirs_assignment(
    value: &ast::Expr,
    env: &EvalEnv<'_>,
) -> Option<ExtractedList<Spanned<TemplateDirPath>>> {
    match value {
        ast::Expr::List(list) => Some(evaluate_path_elements(&list.elts, env)),
        ast::Expr::Tuple(tuple) => Some(evaluate_path_elements(&tuple.elts, env)),
        _ => None,
    }
}

fn evaluate_path_elements(
    elements: &[ast::Expr],
    env: &EvalEnv<'_>,
) -> ExtractedList<Spanned<TemplateDirPath>> {
    let mut extracted = ExtractedList::complete(Vec::new());
    for element in elements {
        let path = env.evaluate_template_dir_path(element);
        if path == TemplateDirPath::Unknown {
            extracted.status.mark_incomplete();
        }
        extracted.values.push(Spanned::new(path, element.span()));
    }
    extracted
}
