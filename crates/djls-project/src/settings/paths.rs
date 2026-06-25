use camino::Utf8Path;
use camino::Utf8PathBuf;
use ruff_python_ast as ast;

use crate::ast::ExprExt;
use crate::settings::types::LocalBindings;
use crate::settings::types::TemplateDirPath;

pub(crate) fn evaluate_path_expr(
    expr: &ast::Expr,
    module_path: &Utf8Path,
    locals: &LocalBindings,
) -> TemplateDirPath {
    match evaluate_path(expr, module_path, locals) {
        Some(path) => TemplateDirPath::Resolved(path),
        None => TemplateDirPath::Unknown,
    }
}

fn evaluate_path(
    expr: &ast::Expr,
    module_path: &Utf8Path,
    locals: &LocalBindings,
) -> Option<Utf8PathBuf> {
    if let Some(name) = expr.name_target() {
        return locals.path_value(name).cloned();
    }

    match expr {
        ast::Expr::Attribute(attribute) if attribute.attr.as_str() == "parent" => {
            evaluate_path(&attribute.value, module_path, locals).and_then(|path| {
                path.parent().map_or_else(
                    || Some(Utf8PathBuf::from("/")),
                    |parent| Some(parent.to_path_buf()),
                )
            })
        }
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Div => {
            let base = evaluate_path(&bin_op.left, module_path, locals)?;
            let segment = bin_op.right.string_literal()?;
            Some(base.join(segment))
        }
        ast::Expr::Call(call) => evaluate_path_call(call, module_path, locals),
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
    locals: &LocalBindings,
) -> Option<Utf8PathBuf> {
    match call.func.as_ref() {
        func if func.name_target() == Some("Path") => {
            let argument = single_positional_argument(&call.arguments)?;
            if is_file_name(argument) {
                Some(module_path.to_path_buf())
            } else {
                evaluate_path(argument, module_path, locals)
            }
        }
        func if func.name_target() == Some("str") => evaluate_path(
            single_positional_argument(&call.arguments)?,
            module_path,
            locals,
        ),
        ast::Expr::Attribute(attribute) => match attribute.attr.as_str() {
            "resolve" if call.arguments.is_empty() => {
                evaluate_path(&attribute.value, module_path, locals)
            }
            "joinpath" => {
                let mut path = evaluate_path(&attribute.value, module_path, locals)?;
                for argument in positional_arguments(&call.arguments) {
                    path = path.join(argument.string_literal()?);
                }
                Some(path)
            }
            "join" if is_os_path_attr(&attribute.value, "path") => {
                let mut arguments = positional_arguments(&call.arguments);
                let first = arguments.next()?;
                let mut path = evaluate_path(first, module_path, locals)?;
                for argument in arguments {
                    path = path.join(argument.string_literal()?);
                }
                Some(path)
            }
            "dirname" if is_os_path_attr(&attribute.value, "path") => {
                let path = evaluate_path(
                    single_positional_argument(&call.arguments)?,
                    module_path,
                    locals,
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
            if attribute.attr.as_str() == attr && attribute.value.name_target() == Some("os")
    )
}

fn is_file_name(expr: &ast::Expr) -> bool {
    expr.name_target() == Some("__file__")
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
