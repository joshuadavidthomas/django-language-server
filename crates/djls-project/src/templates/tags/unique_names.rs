//! Unique names extracted from a compile function's membership check on parser state.

use std::ops::ControlFlow;

use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::ExprList;
use ruff_python_ast::ExprSet;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::visitor;
use ruff_python_ast::visitor::Visitor;
use ruff_text_size::Ranged;
use ruff_text_size::TextRange;
use rustc_hash::FxHashMap;

use crate::ast::ExprExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::templates::tags::analysis::AbstractValue;
use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::Env;
use crate::templates::tags::analysis::constants::StaticBindings;
use crate::templates::tags::analysis::constants::seed_static_bindings;
use crate::templates::tags::analysis::exceptions::direct_raise_exception;
use crate::templates::tags::analysis::exceptions::extract_exception_message;
use crate::templates::tags::analysis::expressions::eval_expr;
use crate::templates::tags::analysis::process_statements;
use crate::templates::tags::blocks::is_parser_receiver;
use crate::templates::tags::meaning::NameRegistry;
use crate::templates::tags::meaning::UniqueName;
use crate::templates::tags::types::SplitPosition;

/// Extract an opener argument the compile function rejects when parser state already holds it.
///
/// Recognizes Django's `do_block` and `partialdef_func`:
///
/// ```python
/// if block_name in parser.__loaded_blocks:
///     raise TemplateSyntaxError(...)
/// parser.__loaded_blocks.append(block_name)
/// ```
///
/// The function must both reject the argument and record it in the same registry, so a second
/// Tag with that name fails while compiling the same template. When another function in the
/// module writes the registry, as Tetra's component tag resets `parser.__loaded_slots` around
/// each component body, names are unique only within that scope, so no fact is extracted.
pub(crate) fn extract_unique_name(
    func: &StmtFunctionDef,
    module: &[Stmt],
    static_bindings: Option<&StaticBindings>,
) -> Option<UniqueName> {
    let parser_var = func.parameters.args.first()?.parameter.name.as_str();
    let token_var = func.parameters.args.get(1)?.parameter.name.as_str();
    let registries = Registries {
        parser_var,
        aliases: registry_aliases(&func.body, parser_var),
    };

    let mut guards = Vec::new();
    let mut records = Vec::new();
    walk_stmts(&func.body, Recurse::ControlFlow, |stmt| {
        if let Stmt::If(if_stmt) = stmt {
            let clauses = std::iter::once((Some(if_stmt.test.as_ref()), &if_stmt.body)).chain(
                if_stmt
                    .elif_else_clauses
                    .iter()
                    .map(|clause| (clause.test.as_ref(), &clause.body)),
            );
            for (test, body) in clauses {
                if let Some(test) = test
                    && let Some(exception) = direct_raise_exception(body)
                    && let Some((argument, registry)) = registries.membership(test)
                {
                    guards.push((argument, registry, exception));
                }
            }
        } else if let Stmt::Expr(expr_stmt) = stmt {
            records.extend(registries.recorded_by_call(&expr_stmt.value));
        } else if let Stmt::Assign(assign) = stmt {
            records.extend(
                assign
                    .targets
                    .iter()
                    .filter_map(|target| registries.recorded_by_assignment(target, &assign.value)),
            );
        }
        ControlFlow::Continue(())
    });
    if guards.is_empty() || records.is_empty() {
        return None;
    }

    let mut env = Env::for_compile_function(parser_var, token_var);
    if let Some(static_bindings) = static_bindings {
        seed_static_bindings(static_bindings, func, &mut env);
    }
    let mut ctx = CallContext {
        source: None,
        manual_as_var_strips: &[],
    };
    let _ = process_statements(&func.body, &mut env, &mut ctx);
    let split_position = |expr: &Expr| {
        let AbstractValue::SplitElement {
            index: SplitPosition::Forward(position),
        } = eval_expr(expr, &mut env.clone())
        else {
            return None;
        };
        (position > 0).then_some(position)
    };

    guards
        .into_iter()
        .find_map(|(argument, registry, exception)| {
            let position = split_position(argument)?;
            records
                .iter()
                .any(|(recorded, recorded_registry)| {
                    *recorded_registry == registry && split_position(recorded) == Some(position)
                })
                .then(|| {
                    UniqueName::new(
                        position,
                        registry,
                        extract_exception_message(exception, &env),
                    )
                })
        })
        .filter(|unique_name| !written_elsewhere(module, func, unique_name.registry()))
}

/// Whether a function other than `func` stores to, deletes, or pops the registry.
fn written_elsewhere(module: &[Stmt], func: &StmtFunctionDef, registry: &NameRegistry) -> bool {
    struct Writes<'r> {
        func: TextRange,
        registry: &'r NameRegistry,
        found: bool,
    }

    impl Writes<'_> {
        fn writes(&self, target: &Expr) -> bool {
            match (self.registry, target) {
                (
                    NameRegistry::ParserAttribute(name),
                    Expr::Attribute(ExprAttribute { attr, .. }),
                ) => attr.as_str() == name,
                (
                    NameRegistry::ParserExtraData(key),
                    Expr::Subscript(ExprSubscript { value, slice, .. }),
                ) => {
                    matches!(value.as_ref(), Expr::Attribute(ExprAttribute { attr, .. }) if attr.as_str() == "extra_data")
                        && slice.string_literal() == Some(key)
                }
                _ => false,
            }
        }

        fn mutates(&self, call: &ExprCall) -> bool {
            let first = call
                .arguments
                .args
                .first()
                .and_then(ExprExt::string_literal);
            match (self.registry, call.func.as_ref()) {
                (NameRegistry::ParserAttribute(name), Expr::Name(function)) => {
                    matches!(function.id.as_str(), "setattr" | "delattr")
                        && call.arguments.args.get(1).and_then(ExprExt::string_literal)
                            == Some(name)
                }
                (
                    NameRegistry::ParserExtraData(key),
                    Expr::Attribute(ExprAttribute { attr, value, .. }),
                ) => {
                    attr.as_str() == "pop"
                        && matches!(value.as_ref(), Expr::Attribute(ExprAttribute { attr, .. }) if attr.as_str() == "extra_data")
                        && first == Some(key)
                }
                _ => false,
            }
        }
    }

    impl<'a> Visitor<'a> for Writes<'_> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.found || stmt.range() == self.func {
                return;
            }
            let targets: &[Expr] = if let Stmt::Assign(assign) = stmt {
                &assign.targets
            } else if let Stmt::AugAssign(assign) = stmt {
                std::slice::from_ref(assign.target.as_ref())
            } else if let Stmt::AnnAssign(assign) = stmt {
                std::slice::from_ref(assign.target.as_ref())
            } else if let Stmt::Delete(delete) = stmt {
                &delete.targets
            } else {
                &[]
            };
            if targets.iter().any(|target| self.writes(target)) {
                self.found = true;
                return;
            }
            visitor::walk_stmt(self, stmt);
        }

        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::Call(call) = expr
                && self.mutates(call)
            {
                self.found = true;
            }
            visitor::walk_expr(self, expr);
        }
    }

    let mut writes = Writes {
        func: func.range(),
        registry,
        found: false,
    };
    writes.visit_body(module);
    writes.found
}

/// Recognizes parser state that persists across the Tags of one template.
struct Registries<'p> {
    parser_var: &'p str,
    aliases: FxHashMap<&'p str, NameRegistry>,
}

impl Registries<'_> {
    fn registry(&self, expr: &Expr) -> Option<NameRegistry> {
        if let Some(name) = expr.name_target() {
            return self.aliases.get(name).cloned();
        }
        direct_registry(expr, self.parser_var)
    }

    /// `name in REGISTRY`
    fn membership<'e>(&self, test: &'e Expr) -> Option<(&'e Expr, NameRegistry)> {
        let Expr::Compare(ExprCompare {
            left,
            ops,
            comparators,
            ..
        }) = test
        else {
            return None;
        };
        let ([CmpOp::In], [registry]) = (&**ops, &**comparators) else {
            return None;
        };
        Some((left.as_ref(), self.registry(registry)?))
    }

    /// `REGISTRY.append(name)` or `REGISTRY.add(name)`
    fn recorded_by_call<'e>(&self, expr: &'e Expr) -> Option<(&'e Expr, NameRegistry)> {
        let Expr::Call(ExprCall {
            func, arguments, ..
        }) = expr
        else {
            return None;
        };
        let Expr::Attribute(ExprAttribute { attr, value, .. }) = func.as_ref() else {
            return None;
        };
        let ([name], []) = (&*arguments.args, &*arguments.keywords) else {
            return None;
        };
        matches!(attr.as_str(), "append" | "add")
            .then(|| self.registry(value))
            .flatten()
            .map(|registry| (name, registry))
    }

    /// `REGISTRY[name] = ...` or `parser.attr = [name]`
    fn recorded_by_assignment<'e>(
        &self,
        target: &'e Expr,
        value: &'e Expr,
    ) -> Option<(&'e Expr, NameRegistry)> {
        if let Expr::Subscript(ExprSubscript {
            value: registry,
            slice,
            ..
        }) = target
        {
            return Some((slice.as_ref(), self.registry(registry)?));
        }
        let (Expr::List(ExprList { elts, .. }) | Expr::Set(ExprSet { elts, .. })) = value else {
            return None;
        };
        let [name] = elts.as_slice() else {
            return None;
        };
        Some((name, direct_registry(target, self.parser_var)?))
    }
}

/// `parser.attr` or `parser.extra_data["key"]`.
fn direct_registry(expr: &Expr, parser_var: &str) -> Option<NameRegistry> {
    if let Expr::Attribute(ExprAttribute { attr, value, .. }) = expr
        && attr.as_str() != "extra_data"
        && is_parser_receiver(value, parser_var)
    {
        return Some(NameRegistry::ParserAttribute(attr.to_string()));
    }
    if let Expr::Subscript(ExprSubscript { value, slice, .. }) = expr
        && is_extra_data(value, parser_var)
    {
        return Some(NameRegistry::ParserExtraData(
            slice.string_literal()?.to_string(),
        ));
    }
    None
}

fn is_extra_data(expr: &Expr, parser_var: &str) -> bool {
    matches!(
        expr,
        Expr::Attribute(ExprAttribute { attr, value, .. })
            if attr.as_str() == "extra_data" && is_parser_receiver(value, parser_var)
    )
}

/// Names every assignment binds to the same registry, including
/// `parser.extra_data.setdefault("key", ...)`.
fn registry_aliases<'a>(body: &'a [Stmt], parser_var: &str) -> FxHashMap<&'a str, NameRegistry> {
    let mut bindings: FxHashMap<&str, Option<NameRegistry>> = FxHashMap::default();
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        if let Stmt::Assign(assign) = stmt {
            for target in &assign.targets {
                if let Some(name) = target.name_target() {
                    let registry = bound_registry(&assign.value, parser_var);
                    bindings
                        .entry(name)
                        .and_modify(|bound| {
                            if *bound != registry {
                                *bound = None;
                            }
                        })
                        .or_insert(registry);
                }
            }
        }
        ControlFlow::Continue(())
    });
    bindings
        .into_iter()
        .filter_map(|(name, registry)| Some((name, registry?)))
        .collect()
}

fn bound_registry(value: &Expr, parser_var: &str) -> Option<NameRegistry> {
    if let Expr::Call(ExprCall {
        func, arguments, ..
    }) = value
        && let Expr::Attribute(ExprAttribute { attr, value, .. }) = func.as_ref()
        && attr.as_str() == "setdefault"
        && is_extra_data(value, parser_var)
    {
        return Some(NameRegistry::ParserExtraData(
            arguments.args.first()?.string_literal()?.to_string(),
        ));
    }
    direct_registry(value, parser_var)
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::templates::tags::testing::find_function_in_source;
    use crate::templates::tags::testing::fixture_source;
    use crate::templates::tags::types::ExtractedMessageArg;
    use crate::templates::tags::types::ExtractedMessageTemplate;

    /// Extract with the module's name-resolution facts, as production does.
    fn unique_name_in(source: &str, name: &str) -> Option<UniqueName> {
        let module = parse_module(source).expect("valid Python").into_syntax();
        let Some(func) = find_function_in_source(source, name) else {
            panic!("fixture should define {name}");
        };
        extract_unique_name(
            &func,
            &module.body,
            Some(&StaticBindings::from_module(&module.body)),
        )
    }

    fn unique_name(source: &str) -> Option<UniqueName> {
        unique_name_in(source, "compile")
    }

    #[test]
    fn django_block_names_are_unique_per_parser_attribute() {
        let source = fixture_source("django/template/loader_tags.py").expect("do_block fixture");
        assert_eq!(
            unique_name_in(source, "do_block"),
            Some(UniqueName::new(
                1,
                NameRegistry::ParserAttribute("__loaded_blocks".to_string()),
                Some(ExtractedMessageTemplate::PercentFormat {
                    template: "'%s' tag with name '%s' appears more than once".to_string(),
                    args: vec![
                        ExtractedMessageArg::SplitElement(SplitPosition::Forward(0)),
                        ExtractedMessageArg::SplitElement(SplitPosition::Forward(1)),
                    ],
                }),
            ))
        );
    }

    #[test]
    fn django_partial_names_are_unique_per_extra_data_key() {
        let source = fixture_source("django/template/defaulttags.py").expect("partialdef fixture");
        assert_eq!(
            unique_name_in(source, "partialdef_func"),
            Some(UniqueName::new(
                1,
                NameRegistry::ParserExtraData("partials".to_string()),
                Some(ExtractedMessageTemplate::PercentFormat {
                    template: "Partial '%s' is already defined in the '%s' template.".to_string(),
                    args: vec![
                        ExtractedMessageArg::SplitElement(SplitPosition::Forward(1)),
                        ExtractedMessageArg::OriginName,
                    ],
                }),
            ))
        );
    }

    #[test]
    fn records_the_checked_argument_position_and_lowers_its_fstring_message() {
        assert_eq!(
            unique_name(
                "def compile(parser, token):\n    bits = token.split_contents()\n    label = bits[2]\n    if label in parser.extra_data['sections']:\n        raise TemplateSyntaxError(f'100% sure {label!r} repeats in {parser.origin.name}')\n    parser.extra_data['sections'][label] = 1\n"
            ),
            Some(UniqueName::new(
                2,
                NameRegistry::ParserExtraData("sections".to_string()),
                Some(ExtractedMessageTemplate::PercentFormat {
                    template: "100%% sure %r repeats in %s".to_string(),
                    args: vec![
                        ExtractedMessageArg::SplitElement(SplitPosition::Forward(2)),
                        ExtractedMessageArg::OriginName,
                    ],
                }),
            ))
        );
    }

    #[test]
    fn check_without_record_is_not_unique() {
        assert_eq!(
            unique_name(
                "def compile(parser, token):\n    name = token.split_contents()[1]\n    if name in parser.seen:\n        raise TemplateSyntaxError('dup')\n    return Node(name)\n"
            ),
            None
        );
    }

    #[test]
    fn record_of_another_argument_is_not_unique() {
        assert_eq!(
            unique_name(
                "def compile(parser, token):\n    bits = token.split_contents()\n    if bits[1] in parser.seen:\n        raise TemplateSyntaxError('dup')\n    parser.seen.append(bits[2])\n"
            ),
            None
        );
    }

    #[test]
    fn check_without_raise_is_not_unique() {
        assert_eq!(
            unique_name(
                "def compile(parser, token):\n    name = token.split_contents()[1]\n    if name in parser.seen:\n        warn('dup')\n    parser.seen.append(name)\n"
            ),
            None
        );
    }

    #[test]
    fn registry_reset_by_another_function_is_not_unique() {
        // Tetra's component tag scopes `{% slot %}` names to each component body.
        let source = "def compile(parser, token):\n    name = token.split_contents()[1]\n    if name in parser.slots:\n        raise TemplateSyntaxError('dup')\n    parser.slots.append(name)\n\ndef component(parser, token):\n    outer = getattr(parser, 'slots', None)\n    parser.slots = []\n    nodelist = parser.parse(('endcomponent',))\n    parser.slots = outer\n";
        assert_eq!(unique_name(source), None);
        let read_only = "def compile(parser, token):\n    name = token.split_contents()[1]\n    if name in parser.slots:\n        raise TemplateSyntaxError('dup')\n    parser.slots.append(name)\n\ndef other(parser, token):\n    return Node(parser.slots)\n";
        assert_eq!(
            unique_name(read_only),
            Some(UniqueName::new(
                1,
                NameRegistry::ParserAttribute("slots".to_string()),
                Some(ExtractedMessageTemplate::Static("dup".to_string())),
            ))
        );
    }

    #[test]
    fn check_against_local_state_is_not_unique() {
        assert_eq!(
            unique_name(
                "def compile(parser, token):\n    seen = []\n    name = token.split_contents()[1]\n    if name in seen:\n        raise TemplateSyntaxError('dup')\n    seen.append(name)\n"
            ),
            None
        );
    }
}
