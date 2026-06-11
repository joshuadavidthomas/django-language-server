use camino::Utf8Path;
use camino::Utf8PathBuf;
use ruff_python_ast as ast;

use crate::extraction::settings::PathValue;
use crate::extraction::settings::Reason;
use crate::extraction::settings::SettingsEnv;

pub(crate) fn evaluate_path_expr(
    expr: &ast::Expr,
    module_path: &Utf8Path,
    env: &SettingsEnv,
) -> PathValue {
    match evaluate_path(expr, module_path, env) {
        Some(path) => PathValue::Resolved(path),
        None => PathValue::Unknown(Reason::UnsupportedPathExpression),
    }
}

fn evaluate_path(
    expr: &ast::Expr,
    module_path: &Utf8Path,
    env: &SettingsEnv,
) -> Option<Utf8PathBuf> {
    match expr {
        ast::Expr::Name(name) => env
            .path_value(name.id.as_str())
            .and_then(|path| match path {
                PathValue::Resolved(path) => Some(path.clone()),
                PathValue::Unknown(_) => None,
            }),
        ast::Expr::Attribute(attribute) if attribute.attr.as_str() == "parent" => {
            evaluate_path(&attribute.value, module_path, env).and_then(|path| {
                path.parent().map_or_else(
                    || Some(Utf8PathBuf::from("/")),
                    |parent| Some(parent.to_path_buf()),
                )
            })
        }
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Div => {
            let base = evaluate_path(&bin_op.left, module_path, env)?;
            let segment = string_literal(&bin_op.right)?;
            Some(base.join(segment))
        }
        ast::Expr::Call(call) => evaluate_path_call(call, module_path, env),
        ast::Expr::StringLiteral(literal) => {
            let value = Utf8Path::new(literal.value.to_str());
            if value.is_absolute() {
                Some(value.to_path_buf())
            } else {
                module_path.parent().map(|parent| parent.join(value))
            }
        }
        _ => None,
    }
}

fn evaluate_path_call(
    call: &ast::ExprCall,
    module_path: &Utf8Path,
    env: &SettingsEnv,
) -> Option<Utf8PathBuf> {
    match call.func.as_ref() {
        ast::Expr::Name(name) if name.id.as_str() == "Path" => {
            let argument = single_positional_argument(&call.arguments)?;
            if is_file_name(argument) {
                Some(module_path.to_path_buf())
            } else {
                evaluate_path(argument, module_path, env)
            }
        }
        ast::Expr::Name(name) if name.id.as_str() == "str" => evaluate_path(
            single_positional_argument(&call.arguments)?,
            module_path,
            env,
        ),
        ast::Expr::Attribute(attribute) => match attribute.attr.as_str() {
            "resolve" if call.arguments.is_empty() => {
                evaluate_path(&attribute.value, module_path, env)
            }
            "joinpath" => {
                let mut path = evaluate_path(&attribute.value, module_path, env)?;
                for argument in positional_arguments(&call.arguments) {
                    path = path.join(string_literal(argument)?);
                }
                Some(path)
            }
            "join" if is_os_path_attr(&attribute.value, "path") => {
                let mut arguments = positional_arguments(&call.arguments);
                let first = arguments.next()?;
                let mut path = evaluate_path(first, module_path, env)?;
                for argument in arguments {
                    path = path.join(string_literal(argument)?);
                }
                Some(path)
            }
            "dirname" if is_os_path_attr(&attribute.value, "path") => {
                let path = evaluate_path(
                    single_positional_argument(&call.arguments)?,
                    module_path,
                    env,
                )?;
                path.parent().map(Utf8Path::to_path_buf)
            }
            _ => None,
        },
        _ => None,
    }
}

fn is_os_path_attr(expr: &ast::Expr, attr: &str) -> bool {
    matches!(
        expr,
        ast::Expr::Attribute(attribute)
            if attribute.attr.as_str() == attr && is_name(&attribute.value, "os")
    )
}

fn is_name(expr: &ast::Expr, expected: &str) -> bool {
    matches!(expr, ast::Expr::Name(name) if name.id.as_str() == expected)
}

fn is_file_name(expr: &ast::Expr) -> bool {
    is_name(expr, "__file__")
}

fn single_positional_argument(arguments: &ast::Arguments) -> Option<&ast::Expr> {
    if arguments.keywords.is_empty() && arguments.args.len() == 1 {
        arguments.args.first()
    } else {
        None
    }
}

fn positional_arguments(arguments: &ast::Arguments) -> impl Iterator<Item = &ast::Expr> {
    arguments.args.iter()
}

fn string_literal(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::StringLiteral(literal) => Some(literal.value.to_str()),
        _ => None,
    }
}
