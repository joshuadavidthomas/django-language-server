use ruff_python_ast as ast;

use super::mutations::PythonMutationAccess;
use super::values::PythonValue;
use super::values::PythonValueKind;
use crate::ast::ExprExt;

pub(super) struct MutationTarget<'a> {
    pub(super) root: &'a str,
    pub(super) access: Vec<MutationAccess>,
}

impl<'a> MutationTarget<'a> {
    pub(super) fn from_expr(expr: &'a ast::Expr) -> Option<Self> {
        let mut access = Vec::new();
        let root = collect_mutation_target(expr, &mut access)?;
        access.reverse();
        Some(Self { root, access })
    }

    pub(super) fn resolve_mut<'b>(
        &self,
        value: &'b mut PythonValue,
    ) -> Option<&'b mut PythonValue> {
        let mut current = value;
        for access in &self.access {
            match access {
                MutationAccess::Index(index) => {
                    let PythonValueKind::List(values) = &mut current.kind else {
                        return None;
                    };
                    current = values.get_mut(*index)?;
                }
                MutationAccess::Key(key) => {
                    let PythonValueKind::Dict(dict) = &mut current.kind else {
                        return None;
                    };
                    current = dict.get_string_key_mut(key)?;
                }
            }
        }
        Some(current)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MutationAccess {
    Index(usize),
    Key(String),
}

impl MutationAccess {
    pub(super) fn to_public(&self) -> PythonMutationAccess {
        match self {
            Self::Index(index) => PythonMutationAccess::Index(*index),
            Self::Key(key) => PythonMutationAccess::Key(key.clone()),
        }
    }
}

fn collect_mutation_target<'a>(
    expr: &'a ast::Expr,
    access: &mut Vec<MutationAccess>,
) -> Option<&'a str> {
    if let Some(name) = expr.name_target() {
        return Some(name);
    }

    let ast::Expr::Subscript(subscript) = expr else {
        return None;
    };

    if let Some(index) = subscript.slice.non_negative_integer() {
        access.push(MutationAccess::Index(index));
    } else if let Some(key) = subscript.slice.string_literal() {
        access.push(MutationAccess::Key(key.to_string()));
    } else {
        return None;
    }

    collect_mutation_target(&subscript.value, access)
}
