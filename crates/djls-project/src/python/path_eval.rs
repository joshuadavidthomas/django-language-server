use camino::Utf8Path;
use camino::Utf8PathBuf;
use ruff_python_ast as ast;
use rustc_hash::FxHashMap;

use crate::ast::ExprExt;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PythonPathBindings {
    paths: FxHashMap<String, Utf8PathBuf>,
}

impl PythonPathBindings {
    pub(crate) fn set(&mut self, name: impl Into<String>, value: Utf8PathBuf) {
        self.paths.insert(name.into(), value);
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Utf8PathBuf> {
        self.paths.get(name)
    }
}

pub(crate) fn evaluate_path(
    expr: &ast::Expr,
    file_path: &Utf8Path,
    bindings: &PythonPathBindings,
) -> Option<Utf8PathBuf> {
    if let Some(name) = expr.name_target() {
        return bindings.get(name).cloned();
    }

    match expr {
        ast::Expr::Attribute(attribute) if attribute.attr.as_str() == "parent" => {
            evaluate_path(&attribute.value, file_path, bindings).and_then(|path| {
                path.parent().map_or_else(
                    || Some(Utf8PathBuf::from("/")),
                    |parent| Some(parent.to_path_buf()),
                )
            })
        }
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Div => {
            let base = evaluate_path(&bin_op.left, file_path, bindings)?;
            let segment = bin_op.right.string_literal()?;
            Some(base.join(segment))
        }
        ast::Expr::Call(call) => evaluate_path_call(call, file_path, bindings),
        ast::Expr::StringLiteral(literal) => {
            let value = Utf8Path::new(literal.value.to_str());
            if value.is_absolute() {
                Some(value.to_path_buf())
            } else {
                file_path.parent().map(|parent| parent.join(value))
            }
        }
        _ => None,
    }
}

fn evaluate_path_call(
    call: &ast::ExprCall,
    file_path: &Utf8Path,
    bindings: &PythonPathBindings,
) -> Option<Utf8PathBuf> {
    match call.func.as_ref() {
        func if func.name_target() == Some("Path") => {
            let argument = single_positional_argument(&call.arguments)?;
            if argument.name_target() == Some("__file__") {
                Some(file_path.to_path_buf())
            } else {
                evaluate_path(argument, file_path, bindings)
            }
        }
        func if func.name_target() == Some("str") => evaluate_path(
            single_positional_argument(&call.arguments)?,
            file_path,
            bindings,
        ),
        ast::Expr::Attribute(attribute) => match attribute.attr.as_str() {
            "resolve" if call.arguments.is_empty() => {
                evaluate_path(&attribute.value, file_path, bindings)
            }
            "joinpath" => {
                let mut path = evaluate_path(&attribute.value, file_path, bindings)?;
                for argument in positional_arguments(&call.arguments) {
                    path = path.join(argument.string_literal()?);
                }
                Some(path)
            }
            "join" if is_os_path_attr(&attribute.value, "path") => {
                let mut arguments = positional_arguments(&call.arguments);
                let first = arguments.next()?;
                let mut path = evaluate_path(first, file_path, bindings)?;
                for argument in arguments {
                    path = path.join(argument.string_literal()?);
                }
                Some(path)
            }
            "dirname" if is_os_path_attr(&attribute.value, "path") => {
                let path = evaluate_path(
                    single_positional_argument(&call.arguments)?,
                    file_path,
                    bindings,
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
