use ruff_python_ast::statement_visitor::walk_stmt;
use ruff_python_ast::statement_visitor::StatementVisitor;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBinOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprFString;
use ruff_python_ast::FStringPart;
use ruff_python_ast::InterpolatedStringElement;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtReturn;

use super::is_parser_receiver;
use crate::ext::ExprExt;
use crate::types::BlockSpec;

/// Detect dynamic end-tag patterns: `parser.parse((f"end{tag_name}",))`.
///
/// Returns a `BlockSpec` with `end_tag: None` (dynamic, not statically known)
/// when the function uses f-string or format-string patterns for the end tag.
pub(super) fn detect(body: &[Stmt], parser_var: &str) -> Option<BlockSpec> {
    if !has_dynamic_end_in_body(body, parser_var) {
        return None;
    }
    Some(BlockSpec {
        end_tag: None,
        intermediates: Vec::new(),
        opaque: false,
    })
}

fn has_dynamic_end_in_body(body: &[Stmt], parser_var: &str) -> bool {
    let mut visitor = DynamicEndFinder::new(parser_var);
    visitor.visit_body(body);
    visitor.found
}

struct DynamicEndFinder<'a> {
    parser_var: &'a str,
    found: bool,
}

impl<'a> DynamicEndFinder<'a> {
    fn new(parser_var: &'a str) -> Self {
        Self {
            parser_var,
            found: false,
        }
    }
}

impl StatementVisitor<'_> for DynamicEndFinder<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if self.found {
            return;
        }

        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.found = is_dynamic_end_parse_call(&expr_stmt.value, self.parser_var);
            }
            Stmt::Assign(StmtAssign { value, .. }) => {
                self.found = is_dynamic_end_parse_call(value, self.parser_var);
            }
            Stmt::Return(StmtReturn {
                value: Some(val), ..
            }) => {
                self.found = is_dynamic_end_parse_call(val, self.parser_var);
            }
            // Recurse into control flow to find all possible parse calls.
            Stmt::If(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::Try(_)
            | Stmt::With(_)
            | Stmt::Match(_) => {
                walk_stmt(self, stmt);
            }
            _ => {}
        }
    }
}

/// Check if an expression is `parser.parse((f"end{...}",))`.
fn is_dynamic_end_parse_call(expr: &Expr, parser_var: &str) -> bool {
    let Expr::Call(ExprCall {
        func, arguments, ..
    }) = expr
    else {
        return false;
    };
    let Expr::Attribute(ExprAttribute {
        attr, value: obj, ..
    }) = func.as_ref()
    else {
        return false;
    };
    if attr.as_str() != "parse" {
        return false;
    }
    if !is_parser_receiver(obj, parser_var) {
        return false;
    }
    if arguments.args.is_empty() {
        return false;
    }

    // Check if the argument is a tuple/list containing an f-string with "end" prefix
    let seq = &arguments.args[0];
    let elements = match seq {
        Expr::Tuple(t) => &t.elts,
        Expr::List(l) => &l.elts,
        _ => return false,
    };

    for elt in elements {
        if is_end_fstring(elt) {
            return true;
        }
    }
    false
}

/// Check if an expression is an f-string starting with "end".
pub(super) fn is_end_fstring(expr: &Expr) -> bool {
    let Expr::FString(ExprFString { value, .. }) = expr else {
        return false;
    };

    for part in value {
        match part {
            FStringPart::FString(fstr) => {
                let Some(first) = fstr.elements.first() else {
                    continue;
                };

                let has_end_prefix = matches!(
                    first,
                    InterpolatedStringElement::Literal(lit) if lit.value.starts_with("end")
                );
                if !has_end_prefix {
                    continue;
                }

                let has_interpolation = fstr
                    .elements
                    .iter()
                    .any(|e| matches!(e, InterpolatedStringElement::Interpolation(_)));

                if has_interpolation {
                    return true;
                }
            }
            FStringPart::Literal(_) => {}
        }
    }

    false
}

/// Check for dynamic end-tag format strings: `"end%s" % bits[0]` or `f"end{bits[0]}"`.
pub(super) fn has_dynamic_end_tag_format(body: &[Stmt]) -> bool {
    let mut visitor = DynamicEndFormatFinder::default();
    visitor.visit_body(body);
    visitor.found
}

#[derive(Default)]
struct DynamicEndFormatFinder {
    found: bool,
}

impl StatementVisitor<'_> for DynamicEndFormatFinder {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if self.found {
            return;
        }

        match stmt {
            Stmt::Assign(StmtAssign { value, .. }) => {
                self.found = is_end_format_expr(value);
            }
            // Recurse into control flow to find all possible parse calls.
            Stmt::If(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::Try(_)
            | Stmt::With(_)
            | Stmt::Match(_) => {
                walk_stmt(self, stmt);
            }
            _ => {}
        }
    }
}

/// Check if an expression is `"end%s" % something` or similar end-tag format.
fn is_end_format_expr(expr: &Expr) -> bool {
    // `"end%s" % bits[0]`
    if let Expr::BinOp(ExprBinOp {
        left,
        op: Operator::Mod,
        ..
    }) = expr
    {
        if let Some(s) = left.string_literal() {
            if s.starts_with("end") && s.contains('%') {
                return true;
            }
        }
    }
    // f"end{...}" patterns
    if is_end_fstring(expr) {
        return true;
    }
    false
}
