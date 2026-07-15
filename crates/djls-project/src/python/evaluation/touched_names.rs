use std::collections::BTreeSet;

use ruff_python_ast as ast;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use super::PythonSyntaxImpact;
use super::mutation::MutationTarget;
use super::truthiness::Truthiness;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::python::PythonSyntaxError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TouchedNames {
    names: FxHashSet<String>,
    all: bool,
}

impl TouchedNames {
    fn record(&mut self, name: &str) {
        self.names.insert(name.to_string());
    }

    fn record_all(&mut self) {
        self.all = true;
    }

    fn merge(&mut self, other: Self) {
        self.names.extend(other.names);
        self.all |= other.all;
    }
}

// Recovered syntax impacts intentionally over-approximate source effects without
// depending on evaluator reachability.
fn collect_touched_names(body: &[ast::Stmt]) -> TouchedNames {
    let mut names = TouchedNames::default();
    for stmt in body {
        collect_stmt_touched_names(stmt, &mut names);
    }
    names
}

pub(super) fn collect_syntax_impacts(
    body: &[ast::Stmt],
    errors: &[PythonSyntaxError],
) -> Vec<PythonSyntaxImpact> {
    errors
        .iter()
        .filter_map(|error| {
            let error_start = error.span.start();
            let boundary_stmt = (error.span.length() == 0)
                .then(|| {
                    body.iter()
                        .enumerate()
                        .rev()
                        .find(|(_, stmt)| stmt.span().end() <= error_start)
                })
                .flatten();
            let containing_stmt = body.iter().enumerate().find(|(_, stmt)| {
                let span = stmt.span();
                span.start() < error_start && error_start < span.end()
            });
            let (stmt_index, stmt) = containing_stmt
                .or(boundary_stmt)
                .or_else(|| {
                    body.iter().enumerate().find(|(_, stmt)| {
                        let span = stmt.span();
                        span.start() <= error_start && error_start < span.end()
                    })
                })
                .or_else(|| {
                    body.iter()
                        .enumerate()
                        .find(|(_, stmt)| stmt.span().end() == error_start)
                })?;
            let touched = collect_touched_names(std::slice::from_ref(stmt));
            let later_assignments = definitely_assigned_names_in_body(
                &body[stmt_index + 1..],
                &touched.names,
                touched.all,
            );
            let names = touched
                .names
                .into_iter()
                .filter(|name| !later_assignments.contains(name))
                .collect::<BTreeSet<_>>();
            (!names.is_empty() || touched.all).then(|| PythonSyntaxImpact {
                error: error.clone(),
                names,
                namespace_open: touched.all,
                excluded_names: if touched.all {
                    later_assignments.into_iter().collect()
                } else {
                    BTreeSet::default()
                },
            })
        })
        .collect()
}

fn definitely_assigned_names_in_body(
    body: &[ast::Stmt],
    impacted_names: &FxHashSet<String>,
    namespace_open: bool,
) -> FxHashSet<String> {
    let mut names = FxHashSet::default();
    let mut bool_values = FxHashMap::default();
    let mut taint = AssignmentTaint {
        names: impacted_names.clone(),
        namespace_open,
        clean_names: FxHashSet::default(),
    };
    collect_definite_writes(body, &mut taint, &mut names, &mut bool_values);
    names
}

struct AssignmentTaint {
    names: FxHashSet<String>,
    namespace_open: bool,
    clean_names: FxHashSet<String>,
}

impl AssignmentTaint {
    fn expression_is_independent(&self, value: &ast::Expr) -> bool {
        expr_read_names(value)
            .iter()
            .all(|name| !self.name_is_tainted(name))
    }

    fn name_is_tainted(&self, name: &str) -> bool {
        self.names.contains(name) || (self.namespace_open && !self.clean_names.contains(name))
    }

    fn record_write(&mut self, name: &str, independent: bool) {
        if independent {
            self.names.remove(name);
            self.clean_names.insert(name.to_string());
        } else {
            self.names.insert(name.to_string());
            self.clean_names.remove(name);
        }
    }

    fn record_uncertain_write(&mut self, name: &str) {
        self.names.insert(name.to_string());
        self.clean_names.remove(name);
    }
}

fn collect_definite_writes(
    body: &[ast::Stmt],
    taint: &mut AssignmentTaint,
    names: &mut FxHashSet<String>,
    bool_values: &mut FxHashMap<String, bool>,
) {
    for stmt in body {
        match stmt {
            ast::Stmt::Assign(assign) => {
                let value = exact_bool(&assign.value, bool_values);
                let dominates = taint.expression_is_independent(&assign.value);
                for target in &assign.targets {
                    record_definite_targets(target, value, dominates, taint, names, bool_values);
                }
            }
            ast::Stmt::AnnAssign(assign) if assign.value.is_some() => {
                let value = assign.value.as_deref().expect("guarded by is_some");
                let exact_bool = exact_bool(value, bool_values);
                let dominates = taint.expression_is_independent(value);
                record_definite_targets(
                    &assign.target,
                    exact_bool,
                    dominates,
                    taint,
                    names,
                    bool_values,
                );
            }
            ast::Stmt::Import(import) => {
                for alias in &import.names {
                    let name = alias.asname.as_ref().map_or_else(
                        || first_import_segment(alias.name.as_str()),
                        ast::Identifier::as_str,
                    );
                    record_definite_name(name, None, true, taint, names, bool_values);
                }
            }
            ast::Stmt::ImportFrom(import) => {
                for alias in &import.names {
                    if alias.name.as_str() != "*" {
                        let name = alias
                            .asname
                            .as_ref()
                            .map_or_else(|| alias.name.as_str(), ast::Identifier::as_str);
                        record_definite_name(name, None, true, taint, names, bool_values);
                    }
                }
            }
            ast::Stmt::FunctionDef(function) => {
                record_definite_name(
                    function.name.as_str(),
                    None,
                    true,
                    taint,
                    names,
                    bool_values,
                );
            }
            ast::Stmt::ClassDef(class) => {
                record_definite_name(class.name.as_str(), None, true, taint, names, bool_values);
            }
            ast::Stmt::TypeAlias(alias) => {
                record_definite_targets(&alias.name, None, true, taint, names, bool_values);
            }
            ast::Stmt::If(stmt_if) => {
                if let Some(selected) = deterministic_if_body(stmt_if, bool_values) {
                    collect_definite_writes(selected, taint, names, bool_values);
                } else {
                    let touched = collect_touched_names(std::slice::from_ref(stmt));
                    for name in touched.names {
                        taint.record_uncertain_write(&name);
                        names.remove(&name);
                        bool_values.remove(&name);
                    }
                    if touched.all {
                        taint.namespace_open = true;
                        taint.clean_names.clear();
                    }
                }
            }
            ast::Stmt::AnnAssign(_)
            | ast::Stmt::AugAssign(_)
            | ast::Stmt::Delete(_)
            | ast::Stmt::Expr(_)
            | ast::Stmt::For(_)
            | ast::Stmt::While(_)
            | ast::Stmt::With(_)
            | ast::Stmt::Try(_)
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
}

fn record_definite_targets(
    target: &ast::Expr,
    value: Option<bool>,
    dominates: bool,
    taint: &mut AssignmentTaint,
    names: &mut FxHashSet<String>,
    bool_values: &mut FxHashMap<String, bool>,
) {
    let mut targets = Vec::new();
    collect_definite_binding_names(target, &mut targets);
    let exact_name = target.name_target();
    for name in targets {
        record_definite_name(
            name,
            exact_name.filter(|candidate| *candidate == name).and(value),
            dominates,
            taint,
            names,
            bool_values,
        );
    }
}

fn collect_definite_binding_names<'a>(target: &'a ast::Expr, names: &mut Vec<&'a str>) {
    if let Some(name) = target.name_target() {
        names.push(name);
        return;
    }
    match target {
        ast::Expr::Tuple(tuple) => {
            for element in &tuple.elts {
                collect_definite_binding_names(element, names);
            }
        }
        ast::Expr::List(list) => {
            for element in &list.elts {
                collect_definite_binding_names(element, names);
            }
        }
        ast::Expr::Starred(starred) => {
            collect_definite_binding_names(&starred.value, names);
        }
        ast::Expr::Attribute(_)
        | ast::Expr::Subscript(_)
        | ast::Expr::If(_)
        | ast::Expr::Named(_)
        | ast::Expr::BinOp(_)
        | ast::Expr::UnaryOp(_)
        | ast::Expr::Lambda(_)
        | ast::Expr::BoolOp(_)
        | ast::Expr::Await(_)
        | ast::Expr::Yield(_)
        | ast::Expr::YieldFrom(_)
        | ast::Expr::Compare(_)
        | ast::Expr::Call(_)
        | ast::Expr::FString(_)
        | ast::Expr::TString(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::BytesLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NoneLiteral(_)
        | ast::Expr::EllipsisLiteral(_)
        | ast::Expr::ListComp(_)
        | ast::Expr::Set(_)
        | ast::Expr::SetComp(_)
        | ast::Expr::Dict(_)
        | ast::Expr::DictComp(_)
        | ast::Expr::Generator(_)
        | ast::Expr::Slice(_)
        | ast::Expr::IpyEscapeCommand(_)
        | ast::Expr::Name(_) => {}
    }
}

fn record_definite_name(
    name: &str,
    value: Option<bool>,
    dominates: bool,
    taint: &mut AssignmentTaint,
    names: &mut FxHashSet<String>,
    bool_values: &mut FxHashMap<String, bool>,
) {
    taint.record_write(name, dominates);
    if dominates {
        names.insert(name.to_string());
    } else {
        names.remove(name);
    }
    if dominates && let Some(value) = value {
        bool_values.insert(name.to_string(), value);
    } else {
        bool_values.remove(name);
    }
}

fn deterministic_if_body<'a>(
    stmt_if: &'a ast::StmtIf,
    bool_values: &FxHashMap<String, bool>,
) -> Option<&'a [ast::Stmt]> {
    match known_truthiness(&stmt_if.test, bool_values) {
        Truthiness::AlwaysTrue => Some(&stmt_if.body),
        Truthiness::AlwaysFalse => {
            for clause in &stmt_if.elif_else_clauses {
                let Some(test) = &clause.test else {
                    return Some(&clause.body);
                };
                match known_truthiness(test, bool_values) {
                    Truthiness::AlwaysTrue => return Some(&clause.body),
                    Truthiness::AlwaysFalse => {}
                    Truthiness::Ambiguous => return None,
                }
            }
            Some(&[])
        }
        Truthiness::Ambiguous => None,
    }
}

fn exact_bool(expr: &ast::Expr, bool_values: &FxHashMap<String, bool>) -> Option<bool> {
    match known_truthiness(expr, bool_values) {
        Truthiness::AlwaysTrue => Some(true),
        Truthiness::AlwaysFalse => Some(false),
        Truthiness::Ambiguous => None,
    }
}

fn known_truthiness(expression: &ast::Expr, bool_values: &FxHashMap<String, bool>) -> Truthiness {
    Truthiness::of_expr(expression, &|name| bool_values.get(name).copied())
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
                names.record(target.binding);
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
        ast::Expr::Lambda(lambda) => {
            if let Some(parameters) = &lambda.parameters {
                collect_parameter_read_names(parameters, names);
            }
            collect_expr_read_names(&lambda.body, names);
        }
        ast::Expr::ListComp(comp) => {
            collect_comprehension_read_names(&comp.generators, names);
            collect_expr_read_names(&comp.elt, names);
        }
        ast::Expr::SetComp(comp) => {
            collect_comprehension_read_names(&comp.generators, names);
            collect_expr_read_names(&comp.elt, names);
        }
        ast::Expr::DictComp(comp) => {
            collect_comprehension_read_names(&comp.generators, names);
            collect_expr_read_names(&comp.key, names);
            collect_expr_read_names(&comp.value, names);
        }
        ast::Expr::Generator(generator) => {
            collect_comprehension_read_names(&generator.generators, names);
            collect_expr_read_names(&generator.elt, names);
        }
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
        ast::Expr::FString(f_string) => {
            for part in &f_string.value {
                if let ast::FStringPart::FString(f_string) = part {
                    collect_interpolated_string_read_names(&f_string.elements, names);
                }
            }
        }
        ast::Expr::TString(t_string) => {
            for t_string in &t_string.value {
                collect_interpolated_string_read_names(&t_string.elements, names);
            }
        }
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

fn collect_parameter_read_names(parameters: &ast::Parameters, names: &mut FxHashSet<String>) {
    for parameter in parameters.iter_non_variadic_params() {
        if let Some(default) = &parameter.default {
            collect_expr_read_names(default, names);
        }
    }
    for parameter in parameters {
        if let Some(annotation) = &parameter.as_parameter().annotation {
            collect_expr_read_names(annotation, names);
        }
    }
}

fn collect_comprehension_read_names(
    generators: &[ast::Comprehension],
    names: &mut FxHashSet<String>,
) {
    for generator in generators {
        collect_expr_read_names(&generator.iter, names);
        for condition in &generator.ifs {
            collect_expr_read_names(condition, names);
        }
    }
}

fn collect_interpolated_string_read_names(
    elements: &ast::InterpolatedStringElements,
    names: &mut FxHashSet<String>,
) {
    for element in elements {
        let ast::InterpolatedStringElement::Interpolation(interpolation) = element else {
            continue;
        };
        collect_expr_read_names(&interpolation.expression, names);
        if let Some(format_spec) = &interpolation.format_spec {
            collect_interpolated_string_read_names(&format_spec.elements, names);
        }
    }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ruff_python_ast as ast;
    use ruff_python_parser::parse_module;

    use super::expr_read_names;

    fn read_names(expression: &str) -> BTreeSet<String> {
        let source = format!("VALUE = {expression}\n");
        let module = parse_module(&source)
            .expect("expression should parse")
            .into_syntax();
        let [ast::Stmt::Assign(assignment)] = module.body.as_slice() else {
            panic!("expected one assignment");
        };
        expr_read_names(&assignment.value).into_iter().collect()
    }

    #[test]
    fn expression_reads_include_every_comprehension_input() {
        for (expression, expected) in [
            (
                "[result for target in iterable if condition]",
                &["condition", "iterable", "result"][..],
            ),
            (
                "{result for target in iterable if first if second}",
                &["first", "iterable", "result", "second"],
            ),
            (
                "{key: value for target in iterable if condition}",
                &["condition", "iterable", "key", "value"],
            ),
            (
                "(result for target in first_iterable if first_condition for nested in second_iterable if second_condition if third_condition)",
                &[
                    "first_condition",
                    "first_iterable",
                    "result",
                    "second_condition",
                    "second_iterable",
                    "third_condition",
                ],
            ),
        ] {
            assert_eq!(
                read_names(expression),
                expected.iter().map(|name| (*name).to_string()).collect(),
                "{expression}"
            );
        }
    }

    #[test]
    fn expression_reads_include_nested_formatted_string_interpolations() {
        assert_eq!(
            read_names("f'{value:{width}.{precision}}' f'{other}'"),
            ["other", "precision", "value", "width"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        assert_eq!(
            read_names("t'{value:{width}}' t'{other}'"),
            ["other", "value", "width"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }

    #[test]
    fn expression_reads_include_lambda_parameter_defaults() {
        assert_eq!(
            read_names("lambda parameter=default: parameter + result"),
            ["default", "parameter", "result"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }
}
