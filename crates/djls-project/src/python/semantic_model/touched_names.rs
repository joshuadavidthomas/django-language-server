use ruff_python_ast as ast;
use rustc_hash::FxHashSet;

use super::mutation_target::MutationTarget;
use crate::ast::ExprExt;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct TouchedNames {
    pub(super) names: FxHashSet<String>,
    pub(super) all: bool,
}

impl TouchedNames {
    pub(super) fn record(&mut self, name: &str) {
        self.names.insert(name.to_string());
    }

    pub(super) fn record_all(&mut self) {
        self.all = true;
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.names.extend(other.names);
        self.all |= other.all;
    }
}

// This must mirror every statement effect that can alter bindings or mutation
// state. Branch joins and loop degradation use it to decide which names lose
// straight-line certainty.
pub(super) fn collect_touched_names(body: &[ast::Stmt]) -> TouchedNames {
    let mut names = TouchedNames::default();
    for stmt in body {
        collect_stmt_touched_names(stmt, &mut names);
    }
    names
}

fn collect_stmt_touched_names(stmt: &ast::Stmt, names: &mut TouchedNames) {
    if collect_control_flow_stmt_touched_names(stmt, names) {
        return;
    }

    match stmt {
        ast::Stmt::Assign(assign) => {
            for target in &assign.targets {
                for name in target_write_names(target) {
                    names.record(name);
                }
            }
        }
        ast::Stmt::AnnAssign(assign) => {
            for name in target_write_names(&assign.target) {
                names.record(name);
            }
        }
        ast::Stmt::AugAssign(assign) => {
            for name in target_write_names(&assign.target) {
                names.record(name);
            }
        }
        ast::Stmt::Delete(delete) => {
            for target in &delete.targets {
                for name in target_write_names(target) {
                    names.record(name);
                }
            }
        }
        ast::Stmt::Expr(expr) => {
            if let ast::Expr::Call(call) = expr.value.as_ref()
                && let ast::Expr::Attribute(attribute) = call.func.as_ref()
                && let Some(target) = MutationTarget::from_expr(&attribute.value)
            {
                names.record(target.root);
            }
            for name in expr_read_names(&expr.value) {
                names.record(&name);
            }
        }
        ast::Stmt::Import(import) => {
            for alias in &import.names {
                let bound_name = alias.asname.as_ref().map_or_else(
                    || first_import_segment(alias.name.as_str()),
                    |asname| asname.as_str(),
                );
                names.record(bound_name);
            }
        }
        ast::Stmt::ImportFrom(import) => {
            if import.names.iter().any(|alias| alias.name.as_str() == "*") {
                names.record_all();
            } else {
                for alias in &import.names {
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                    names.record(bound_name);
                }
            }
        }
        ast::Stmt::FunctionDef(function) => names.record(function.name.as_str()),
        ast::Stmt::ClassDef(class) => names.record(class.name.as_str()),
        ast::Stmt::TypeAlias(type_alias) => {
            for name in target_write_names(&type_alias.name) {
                names.record(name);
            }
        }
        ast::Stmt::For(_)
        | ast::Stmt::While(_)
        | ast::Stmt::With(_)
        | ast::Stmt::Try(_)
        | ast::Stmt::If(_)
        | ast::Stmt::Match(_)
        | ast::Stmt::Return(_)
        | ast::Stmt::Raise(_)
        | ast::Stmt::Assert(_)
        | ast::Stmt::Global(_)
        | ast::Stmt::Nonlocal(_)
        | ast::Stmt::Pass(_)
        | ast::Stmt::Break(_)
        | ast::Stmt::Continue(_)
        | ast::Stmt::IpyEscapeCommand(_) => {}
    }
}

fn collect_control_flow_stmt_touched_names(stmt: &ast::Stmt, names: &mut TouchedNames) -> bool {
    match stmt {
        ast::Stmt::For(stmt_for) => {
            for name in target_write_names(&stmt_for.target) {
                names.record(name);
            }
            names.merge(collect_touched_names(&stmt_for.body));
            names.merge(collect_touched_names(&stmt_for.orelse));
        }
        ast::Stmt::While(stmt_while) => {
            names.merge(collect_touched_names(&stmt_while.body));
            names.merge(collect_touched_names(&stmt_while.orelse));
        }
        ast::Stmt::With(stmt_with) => {
            for item in &stmt_with.items {
                if let Some(optional_vars) = &item.optional_vars {
                    for name in target_write_names(optional_vars) {
                        names.record(name);
                    }
                }
            }
            names.merge(collect_touched_names(&stmt_with.body));
        }
        ast::Stmt::Try(stmt_try) => {
            names.merge(collect_touched_names(&stmt_try.body));
            for handler in &stmt_try.handlers {
                let ast::ExceptHandler::ExceptHandler(handler) = handler;
                names.merge(collect_touched_names(&handler.body));
            }
            names.merge(collect_touched_names(&stmt_try.orelse));
            names.merge(collect_touched_names(&stmt_try.finalbody));
        }
        ast::Stmt::If(stmt_if) => {
            names.merge(collect_touched_names(&stmt_if.body));
            for clause in &stmt_if.elif_else_clauses {
                names.merge(collect_touched_names(&clause.body));
            }
        }
        ast::Stmt::Match(stmt_match) => {
            for case in &stmt_match.cases {
                for name in pattern_bound_names(&case.pattern) {
                    names.record(name);
                }
                names.merge(collect_touched_names(&case.body));
            }
        }
        _ => return false,
    }
    true
}

pub(super) fn target_write_names(target: &ast::Expr) -> Vec<&str> {
    let mut names = Vec::new();
    collect_target_write_names(target, &mut names);
    names
}

fn collect_target_write_names<'a>(target: &'a ast::Expr, names: &mut Vec<&'a str>) {
    if let Some(name) = target.name_target() {
        names.push(name);
        return;
    }

    match target {
        ast::Expr::Attribute(attribute) => collect_target_write_names(&attribute.value, names),
        ast::Expr::Subscript(subscript) => collect_target_write_names(&subscript.value, names),
        ast::Expr::Tuple(tuple) => {
            for expr in &tuple.elts {
                collect_target_write_names(expr, names);
            }
        }
        ast::Expr::List(list) => {
            for expr in &list.elts {
                collect_target_write_names(expr, names);
            }
        }
        ast::Expr::Starred(starred) => collect_target_write_names(&starred.value, names),
        _ => {}
    }
}

pub(super) fn expr_read_names(expr: &ast::Expr) -> FxHashSet<String> {
    let mut names = FxHashSet::default();
    collect_expr_read_names(expr, &mut names);
    names
}

fn collect_expr_read_names(expr: &ast::Expr, names: &mut FxHashSet<String>) {
    if let Some(name) = expr.name_target() {
        names.insert(name.to_string());
    }
    if collect_simple_expr_read_names(expr, names) {
        return;
    }

    match expr {
        ast::Expr::If(if_expr) => {
            collect_expr_read_names(&if_expr.test, names);
            collect_expr_read_names(&if_expr.body, names);
            collect_expr_read_names(&if_expr.orelse, names);
        }
        ast::Expr::Lambda(lambda) => collect_expr_read_names(&lambda.body, names),
        ast::Expr::ListComp(comp) => collect_expr_read_names(&comp.elt, names),
        ast::Expr::SetComp(comp) => collect_expr_read_names(&comp.elt, names),
        ast::Expr::DictComp(comp) => {
            collect_expr_read_names(&comp.key, names);
            collect_expr_read_names(&comp.value, names);
        }
        ast::Expr::Generator(generator) => collect_expr_read_names(&generator.elt, names),
        ast::Expr::Await(await_expr) => collect_expr_read_names(&await_expr.value, names),
        ast::Expr::Yield(yield_expr) => {
            if let Some(value) = &yield_expr.value {
                collect_expr_read_names(value, names);
            }
        }
        ast::Expr::YieldFrom(yield_from) => collect_expr_read_names(&yield_from.value, names),
        ast::Expr::Named(named_expr) => {
            collect_target_write_names(&named_expr.target, &mut Vec::new());
            collect_expr_read_names(&named_expr.value, names);
        }
        ast::Expr::Slice(slice) => collect_slice_read_names(slice, names),
        ast::Expr::Attribute(_)
        | ast::Expr::Subscript(_)
        | ast::Expr::Call(_)
        | ast::Expr::BinOp(_)
        | ast::Expr::UnaryOp(_)
        | ast::Expr::BoolOp(_)
        | ast::Expr::Compare(_)
        | ast::Expr::Tuple(_)
        | ast::Expr::List(_)
        | ast::Expr::Set(_)
        | ast::Expr::Dict(_)
        | ast::Expr::Starred(_)
        | ast::Expr::FString(_)
        | ast::Expr::TString(_)
        | ast::Expr::Name(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::BytesLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NoneLiteral(_)
        | ast::Expr::EllipsisLiteral(_)
        | ast::Expr::IpyEscapeCommand(_) => {}
    }
}

fn collect_simple_expr_read_names(expr: &ast::Expr, names: &mut FxHashSet<String>) -> bool {
    match expr {
        ast::Expr::Attribute(attribute) => collect_expr_read_names(&attribute.value, names),
        ast::Expr::Subscript(subscript) => {
            collect_expr_read_names(&subscript.value, names);
            collect_expr_read_names(&subscript.slice, names);
        }
        ast::Expr::Call(call) => {
            collect_expr_read_names(&call.func, names);
            for arg in &call.arguments.args {
                collect_expr_read_names(arg, names);
            }
            for keyword in &call.arguments.keywords {
                collect_expr_read_names(&keyword.value, names);
            }
        }
        ast::Expr::BinOp(bin_op) => {
            collect_expr_read_names(&bin_op.left, names);
            collect_expr_read_names(&bin_op.right, names);
        }
        ast::Expr::UnaryOp(unary) => collect_expr_read_names(&unary.operand, names),
        ast::Expr::BoolOp(bool_op) => {
            for value in &bool_op.values {
                collect_expr_read_names(value, names);
            }
        }
        ast::Expr::Compare(compare) => {
            collect_expr_read_names(&compare.left, names);
            for comparator in &compare.comparators {
                collect_expr_read_names(comparator, names);
            }
        }
        ast::Expr::Tuple(tuple) => collect_elements_read_names(&tuple.elts, names),
        ast::Expr::List(list) => collect_elements_read_names(&list.elts, names),
        ast::Expr::Set(set) => collect_elements_read_names(&set.elts, names),
        ast::Expr::Dict(dict) => collect_dict_read_names(dict, names),
        ast::Expr::Starred(starred) => collect_expr_read_names(&starred.value, names),
        _ => return false,
    }
    true
}

fn collect_elements_read_names(elements: &[ast::Expr], names: &mut FxHashSet<String>) {
    for expr in elements {
        collect_expr_read_names(expr, names);
    }
}

fn collect_dict_read_names(dict: &ast::ExprDict, names: &mut FxHashSet<String>) {
    for item in &dict.items {
        if let Some(key) = &item.key {
            collect_expr_read_names(key, names);
        }
        collect_expr_read_names(&item.value, names);
    }
}

fn collect_slice_read_names(slice: &ast::ExprSlice, names: &mut FxHashSet<String>) {
    if let Some(lower) = &slice.lower {
        collect_expr_read_names(lower, names);
    }
    if let Some(upper) = &slice.upper {
        collect_expr_read_names(upper, names);
    }
    if let Some(step) = &slice.step {
        collect_expr_read_names(step, names);
    }
}

pub(super) fn pattern_bound_names(pattern: &ast::Pattern) -> Vec<&str> {
    let mut names = Vec::new();
    collect_pattern_bound_names(pattern, &mut names);
    names
}

fn collect_pattern_bound_names<'a>(pattern: &'a ast::Pattern, names: &mut Vec<&'a str>) {
    match pattern {
        ast::Pattern::MatchValue(_) | ast::Pattern::MatchSingleton(_) => {}
        ast::Pattern::MatchSequence(sequence) => {
            for pattern in &sequence.patterns {
                collect_pattern_bound_names(pattern, names);
            }
        }
        ast::Pattern::MatchMapping(mapping) => {
            for pattern in &mapping.patterns {
                collect_pattern_bound_names(pattern, names);
            }
            if let Some(rest) = &mapping.rest {
                names.push(rest.as_str());
            }
        }
        ast::Pattern::MatchClass(class) => {
            for pattern in &class.arguments.patterns {
                collect_pattern_bound_names(pattern, names);
            }
            for keyword in &class.arguments.keywords {
                collect_pattern_bound_names(&keyword.pattern, names);
            }
        }
        ast::Pattern::MatchStar(star) => {
            if let Some(name) = &star.name {
                names.push(name.as_str());
            }
        }
        ast::Pattern::MatchAs(match_as) => {
            if let Some(pattern) = &match_as.pattern {
                collect_pattern_bound_names(pattern, names);
            }
            if let Some(name) = &match_as.name {
                names.push(name.as_str());
            }
        }
        ast::Pattern::MatchOr(match_or) => {
            for pattern in &match_or.patterns {
                collect_pattern_bound_names(pattern, names);
            }
        }
    }
}

pub(super) fn first_import_segment(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}
