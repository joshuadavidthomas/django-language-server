//! Tag Roles extracted from the parser APIs a compile function calls.

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprContext;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::visitor;
use ruff_python_ast::visitor::Visitor;
use rustc_hash::FxHashSet;

use crate::ast::ExprExt;
use crate::templates::tags::blocks::is_parser_receiver;
use crate::templates::tags::meaning::TagRole;

/// Key under which Django's `partialdef` and `partial` share definitions in `parser.extra_data`.
const PARTIALS_KEY: &str = "partials";

/// Extract a role from parser sinks in a compile function.
///
/// - `parser.add_library(...)` makes later Tags and Filters available, so the tag loads Template
///   Libraries.
/// - Writing `parser.extra_data["partials"]` defines a Template Partial; reading it renders one.
#[must_use]
pub(crate) fn extract_tag_role(func: &StmtFunctionDef) -> Option<TagRole> {
    let parser_var = func.parameters.args.first()?.parameter.name.as_str();
    let mut visitor = ParserSinkVisitor {
        parser_var,
        extra_data_aliases: extra_data_aliases(&func.body, parser_var),
        adds_library: false,
        writes_partials: false,
        reads_partials: false,
    };
    visitor.visit_body(&func.body);
    if visitor.adds_library {
        Some(TagRole::TemplateLibraryLoader)
    } else if visitor.writes_partials {
        Some(TagRole::TemplatePartial)
    } else if visitor.reads_partials {
        Some(TagRole::TemplatePartialReference)
    } else {
        None
    }
}

fn is_extra_data_attribute(expr: &Expr, parser_var: &str) -> bool {
    matches!(
        expr,
        Expr::Attribute(ExprAttribute { attr, value, .. })
            if attr.as_str() == "extra_data" && is_parser_receiver(value, parser_var)
    )
}

/// Names bound to `parser.extra_data` anywhere in the function body.
fn extra_data_aliases(body: &[Stmt], parser_var: &str) -> FxHashSet<String> {
    struct AliasVisitor<'p> {
        parser_var: &'p str,
        aliases: FxHashSet<String>,
    }

    impl<'a> Visitor<'a> for AliasVisitor<'_> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
                return;
            }
            if let Stmt::Assign(assign) = stmt
                && let [target] = assign.targets.as_slice()
                && let Some(name) = target.name_target()
                && is_extra_data_attribute(&assign.value, self.parser_var)
            {
                self.aliases.insert(name.to_string());
            }
            visitor::walk_stmt(self, stmt);
        }
    }

    let mut visitor = AliasVisitor {
        parser_var,
        aliases: FxHashSet::default(),
    };
    visitor.visit_body(body);
    visitor.aliases
}

struct ParserSinkVisitor<'p> {
    parser_var: &'p str,
    extra_data_aliases: FxHashSet<String>,
    adds_library: bool,
    writes_partials: bool,
    reads_partials: bool,
}

impl ParserSinkVisitor<'_> {
    fn is_extra_data(&self, expr: &Expr) -> bool {
        is_extra_data_attribute(expr, self.parser_var)
            || expr
                .name_target()
                .is_some_and(|name| self.extra_data_aliases.contains(name))
    }

    fn visit_call(&mut self, call: &ExprCall) {
        let args = &call.arguments.args;
        if let Expr::Attribute(ExprAttribute { attr, value, .. }) = call.func.as_ref() {
            match attr.as_str() {
                "add_library" if is_parser_receiver(value, self.parser_var) => {
                    self.adds_library = true;
                }
                "setdefault" if self.is_extra_data(value) && is_partials_key(args.first()) => {
                    self.writes_partials = true;
                }
                "get" if self.is_extra_data(value) && is_partials_key(args.first()) => {
                    self.reads_partials = true;
                }
                _ => {}
            }
        }
        // Django's `partial` hands both the mapping and the key to a lazy view.
        if args.iter().any(|arg| self.is_extra_data(arg))
            && args.iter().any(|arg| is_partials_key(Some(arg)))
        {
            self.reads_partials = true;
        }
    }

    fn visit_subscript(&mut self, subscript: &ExprSubscript) {
        if self.is_extra_data(&subscript.value) && is_partials_key(Some(&subscript.slice)) {
            match subscript.ctx {
                ExprContext::Store | ExprContext::Del => self.writes_partials = true,
                ExprContext::Load | ExprContext::Invalid => self.reads_partials = true,
            }
        }
    }
}

fn is_partials_key(expr: Option<&Expr>) -> bool {
    expr.and_then(ExprExt::string_literal) == Some(PARTIALS_KEY)
}

impl<'a> Visitor<'a> for ParserSinkVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if !matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
            visitor::walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Lambda(_) = expr {
            return;
        }
        if let Expr::Call(call) = expr {
            self.visit_call(call);
        } else if let Expr::Subscript(subscript) = expr {
            self.visit_subscript(subscript);
        }
        visitor::walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;

    use super::*;

    fn role(source: &str) -> Option<TagRole> {
        let module = ruff_python_parser::parse_module(source)
            .expect("valid Python")
            .into_syntax();
        let Some(Stmt::FunctionDef(func)) = module.body.first() else {
            panic!("fixture should start with a function");
        };
        extract_tag_role(func)
    }

    #[test]
    fn add_library_on_the_parser_loads_libraries() {
        assert_eq!(
            role(
                "def load(parser, token):\n    for name in token.split_contents()[1:]:\n        parser.add_library(find(parser, name))\n"
            ),
            Some(TagRole::TemplateLibraryLoader)
        );
        assert_eq!(
            role("def other(parser, token):\n    registry.add_library(token)\n"),
            None
        );
    }

    #[test]
    fn partials_in_extra_data_define_or_render_partials() {
        assert_eq!(
            role(
                "def define(parser, token):\n    partials = parser.extra_data.setdefault('partials', {})\n    partials['x'] = 1\n"
            ),
            Some(TagRole::TemplatePartial)
        );
        assert_eq!(
            role(
                "def render(parser, token):\n    match token.split_contents():\n        case _, name:\n            extra_data = parser.extra_data\n            return Node(DeferredSubDict(extra_data, 'partials'))\n"
            ),
            Some(TagRole::TemplatePartialReference)
        );
        assert_eq!(
            role("def other(parser, token):\n    return parser.extra_data.get('cache')\n"),
            None
        );
    }

    #[test]
    fn nested_functions_do_not_lend_their_sinks() {
        assert_eq!(
            role(
                "def outer(parser, token):\n    def inner(parser):\n        parser.add_library(x)\n    return Node()\n"
            ),
            None
        );
    }
}
