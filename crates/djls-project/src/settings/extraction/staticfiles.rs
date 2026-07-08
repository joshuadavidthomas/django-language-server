use djls_source::File;
use ruff_python_ast as ast;

use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::settings::extraction::AssignmentCompleteness;
use crate::settings::extraction::bindings::ExtractedList;
use crate::settings::extraction::env::EvalEnv;
use crate::settings::types::EvaluatedPath;
use crate::settings::types::Origin;
use crate::settings::types::Originated;

pub(super) fn evaluate_static_url_assignment(
    value: &ast::Expr,
    file: File,
) -> Option<(Originated<String>, AssignmentCompleteness)> {
    let value_string = value.string_literal()?;

    Some((
        Originated::new(value_string.to_string(), Origin::new(file, value.span())),
        AssignmentCompleteness::Full,
    ))
}

pub(super) fn evaluate_static_root_assignment(
    value: &ast::Expr,
    env: &EvalEnv<'_>,
    file: File,
) -> (Originated<EvaluatedPath>, AssignmentCompleteness) {
    let path = env.evaluate_template_dir_path(value);
    let completeness = if path == EvaluatedPath::Unknown {
        AssignmentCompleteness::Partial
    } else {
        AssignmentCompleteness::Full
    };

    (
        Originated::new(path, Origin::new(file, value.span())),
        completeness,
    )
}

pub(super) fn evaluate_staticfiles_dirs_assignment(
    value: &ast::Expr,
    env: &EvalEnv<'_>,
    file: File,
) -> Option<ExtractedList<Originated<EvaluatedPath>>> {
    match value {
        ast::Expr::List(list) => Some(evaluate_path_elements(&list.elts, env, file)),
        ast::Expr::Tuple(tuple) => Some(evaluate_path_elements(&tuple.elts, env, file)),
        _ => None,
    }
}

fn evaluate_path_elements(
    elements: &[ast::Expr],
    env: &EvalEnv<'_>,
    file: File,
) -> ExtractedList<Originated<EvaluatedPath>> {
    let mut extracted = ExtractedList::complete(Vec::new());
    for element in elements {
        let path = env.evaluate_template_dir_path(element);
        if path == EvaluatedPath::Unknown {
            extracted.status.mark_incomplete();
        }
        extracted
            .values
            .push(Originated::new(path, Origin::new(file, element.span())));
    }
    extracted
}
