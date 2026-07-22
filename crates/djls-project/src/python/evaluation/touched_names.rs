use std::collections::BTreeSet;
use std::slice;

use ruff_python_ast as ast;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use super::PythonSyntaxErrorImpact;
use super::mutation::MutationTarget;
use super::name_analysis::expr_read_names;
use super::name_analysis::pattern_bound_names;
use super::name_analysis::reachable_expr_read_names;
use super::name_analysis::target_write_names;
use super::truthiness::Truthiness;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::python::PythonSyntaxError;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TouchedNames {
    names: FxHashSet<String>,
    all: bool,
}

impl TouchedNames {
    fn from_body(body: &[ast::Stmt]) -> Self {
        let mut touched = Self::default();
        touched.visit_body(body);
        touched
    }

    fn visit_body(&mut self, body: &[ast::Stmt]) {
        for statement in body {
            self.visit_stmt(statement);
        }
    }

    fn visit_stmt(&mut self, statement: &ast::Stmt) {
        match statement {
            ast::Stmt::Assign(assign) => {
                for target in &assign.targets {
                    self.record_targets(target);
                }
            }
            ast::Stmt::AnnAssign(assign) => self.record_targets(&assign.target),
            ast::Stmt::AugAssign(assign) => self.record_targets(&assign.target),
            ast::Stmt::Delete(delete) => {
                for target in &delete.targets {
                    self.record_targets(target);
                }
            }
            ast::Stmt::Expr(expression) => {
                if let ast::Expr::Call(call) = expression.value.as_ref()
                    && let ast::Expr::Attribute(attribute) = call.func.as_ref()
                    && let Some(target) = MutationTarget::from_expr(&attribute.value)
                {
                    self.record(target.binding);
                }
                for name in expr_read_names(&expression.value) {
                    self.record(&name);
                }
            }
            ast::Stmt::Import(import) => {
                for clause in DirectImportClause::lower(import) {
                    self.record(clause.bound());
                }
            }
            ast::Stmt::ImportFrom(import) => {
                let syntax = FromImportSyntax::lower(import);
                if syntax.has_star() {
                    self.record_all();
                } else {
                    for clause in syntax.named_members() {
                        self.record(clause.bound());
                    }
                }
            }
            ast::Stmt::FunctionDef(function) => self.record(function.name.as_str()),
            ast::Stmt::ClassDef(class) => self.record(class.name.as_str()),
            ast::Stmt::TypeAlias(alias) => self.record_targets(&alias.name),
            ast::Stmt::For(statement) => {
                self.record_targets(&statement.target);
                self.visit_body(&statement.body);
                self.visit_body(&statement.orelse);
            }
            ast::Stmt::While(statement) => {
                self.visit_body(&statement.body);
                self.visit_body(&statement.orelse);
            }
            ast::Stmt::With(statement) => {
                for item in &statement.items {
                    if let Some(optional_vars) = &item.optional_vars {
                        self.record_targets(optional_vars);
                    }
                }
                self.visit_body(&statement.body);
            }
            ast::Stmt::Try(statement) => {
                self.visit_body(&statement.body);
                for handler in &statement.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    self.visit_body(&handler.body);
                }
                self.visit_body(&statement.orelse);
                self.visit_body(&statement.finalbody);
            }
            ast::Stmt::If(statement) => {
                self.visit_body(&statement.body);
                for clause in &statement.elif_else_clauses {
                    self.visit_body(&clause.body);
                }
            }
            ast::Stmt::Match(statement) => {
                for case in &statement.cases {
                    for name in pattern_bound_names(&case.pattern) {
                        self.record(name);
                    }
                    self.visit_body(&case.body);
                }
            }
            ast::Stmt::Return(_)
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

    fn record(&mut self, name: &str) {
        self.names.insert(name.to_string());
    }

    fn record_targets(&mut self, target: &ast::Expr) {
        for name in target_write_names(target) {
            self.record(name);
        }
    }

    fn record_all(&mut self) {
        self.all = true;
    }
}

// Recovered syntax impacts intentionally over-approximate source effects without
// depending on evaluator reachability.
pub(super) fn collect_syntax_impacts(
    body: &[ast::Stmt],
    errors: &[PythonSyntaxError],
) -> Vec<PythonSyntaxErrorImpact> {
    errors
        .iter()
        .filter_map(|error| {
            let error_start = error.span.start();
            let boundary_stmt = (error.span.length() == 0)
                .then(|| {
                    body.iter()
                        .enumerate()
                        .rev()
                        .find(|(_, statement)| statement.span().end() <= error_start)
                })
                .flatten();
            let containing_stmt = body.iter().enumerate().find(|(_, statement)| {
                let span = statement.span();
                span.start() < error_start && error_start < span.end()
            });
            let (statement_index, statement) = containing_stmt
                .or(boundary_stmt)
                .or_else(|| {
                    body.iter().enumerate().find(|(_, statement)| {
                        let span = statement.span();
                        span.start() <= error_start && error_start < span.end()
                    })
                })
                .or_else(|| {
                    body.iter()
                        .enumerate()
                        .find(|(_, statement)| statement.span().end() == error_start)
                })?;
            let touched = TouchedNames::from_body(slice::from_ref(statement));
            let later_assignments = DefiniteWriteCollector::collect(
                &body[statement_index + 1..],
                &touched.names,
                touched.all,
            );
            let names = touched
                .names
                .into_iter()
                .filter(|name| !later_assignments.contains(name))
                .collect::<BTreeSet<_>>();
            (!names.is_empty() || touched.all).then(|| PythonSyntaxErrorImpact {
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

struct DefiniteWriteCollector {
    taint: AssignmentTaint,
    names: FxHashSet<String>,
    known_name_truthiness: FxHashMap<String, Truthiness>,
}

impl DefiniteWriteCollector {
    fn collect(
        body: &[ast::Stmt],
        impacted_names: &FxHashSet<String>,
        namespace_open: bool,
    ) -> FxHashSet<String> {
        let mut collector = Self {
            taint: AssignmentTaint::new(impacted_names, namespace_open),
            names: FxHashSet::default(),
            known_name_truthiness: FxHashMap::default(),
        };
        collector.visit_body(body);
        collector.names
    }

    fn visit_body(&mut self, body: &[ast::Stmt]) {
        for statement in body {
            self.visit_stmt(statement);
        }
    }

    fn visit_stmt(&mut self, statement: &ast::Stmt) {
        match statement {
            ast::Stmt::Assign(assign) => {
                let value = self.known_truthiness(&assign.value);
                let dominates = self.expression_is_independent(&assign.value);
                for target in &assign.targets {
                    self.record_targets(target, value, dominates);
                }
            }
            ast::Stmt::AnnAssign(assign) => {
                if let Some(value) = assign.value.as_deref() {
                    let truthiness = self.known_truthiness(value);
                    let dominates = self.expression_is_independent(value);
                    self.record_targets(&assign.target, truthiness, dominates);
                }
            }
            ast::Stmt::Import(import) => {
                for clause in DirectImportClause::lower(import) {
                    self.record_name(clause.bound(), None, true);
                }
            }
            ast::Stmt::ImportFrom(import) => {
                for clause in FromImportSyntax::lower(import).named_members() {
                    self.record_name(clause.bound(), None, true);
                }
            }
            ast::Stmt::FunctionDef(function) => {
                self.record_name(function.name.as_str(), None, true);
            }
            ast::Stmt::ClassDef(class) => {
                self.record_name(class.name.as_str(), None, true);
            }
            ast::Stmt::TypeAlias(alias) => self.record_targets(&alias.name, None, true),
            ast::Stmt::If(statement) => {
                if let Some(selected) = self.deterministic_if_body(statement) {
                    self.visit_body(selected);
                } else {
                    let mut touched = TouchedNames::default();
                    touched.visit_body(&statement.body);
                    for clause in &statement.elif_else_clauses {
                        touched.visit_body(&clause.body);
                    }
                    self.record_uncertain_writes(touched);
                }
            }
            ast::Stmt::AugAssign(_)
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

    fn record_targets(&mut self, target: &ast::Expr, value: Option<Truthiness>, dominates: bool) {
        let exact_name = target.name_target();
        self.record_binding_targets(target, exact_name, value, dominates);
    }

    fn record_binding_targets(
        &mut self,
        target: &ast::Expr,
        exact_name: Option<&str>,
        value: Option<Truthiness>,
        dominates: bool,
    ) {
        if let Some(name) = target.name_target() {
            let value = exact_name.filter(|candidate| *candidate == name).and(value);
            self.record_name(name, value, dominates);
            return;
        }
        match target {
            ast::Expr::Tuple(tuple) => {
                for element in &tuple.elts {
                    self.record_binding_targets(element, exact_name, value, dominates);
                }
            }
            ast::Expr::List(list) => {
                for element in &list.elts {
                    self.record_binding_targets(element, exact_name, value, dominates);
                }
            }
            ast::Expr::Starred(starred) => {
                self.record_binding_targets(&starred.value, exact_name, value, dominates);
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

    fn record_name(&mut self, name: &str, value: Option<Truthiness>, dominates: bool) {
        self.taint.record_write(name, dominates);
        if dominates {
            self.names.insert(name.to_string());
        } else {
            self.names.remove(name);
        }
        if dominates && let Some(value) = value {
            self.known_name_truthiness.insert(name.to_string(), value);
        } else {
            self.known_name_truthiness.remove(name);
        }
    }

    fn record_uncertain_writes(&mut self, touched: TouchedNames) {
        for name in touched.names {
            self.taint.record_uncertain_write(&name);
            self.names.remove(&name);
            self.known_name_truthiness.remove(&name);
        }
        if touched.all {
            self.taint.record_uncertain_namespace_write();
        }
    }

    fn deterministic_if_body<'a>(&self, statement: &'a ast::StmtIf) -> Option<&'a [ast::Stmt]> {
        match self.known_truthiness(&statement.test) {
            Some(Truthiness::Truthy) => Some(&statement.body),
            Some(Truthiness::Falsy) => {
                for clause in &statement.elif_else_clauses {
                    let Some(test) = &clause.test else {
                        return Some(&clause.body);
                    };
                    match self.known_truthiness(test) {
                        Some(Truthiness::Truthy) => return Some(&clause.body),
                        Some(Truthiness::Falsy) => {}
                        None => return None,
                    }
                }
                Some(&[])
            }
            None => None,
        }
    }

    fn known_truthiness(&self, expression: &ast::Expr) -> Option<Truthiness> {
        Truthiness::of_expr(expression, &|name| {
            self.known_name_truthiness.get(name).copied()
        })
    }

    fn expression_is_independent(&self, value: &ast::Expr) -> bool {
        reachable_expr_read_names(value, &|expression| self.known_truthiness(expression))
            .iter()
            .all(|name| !self.taint.name_is_tainted(name))
    }
}

struct AssignmentTaint {
    names: FxHashSet<String>,
    namespace_open: bool,
    clean_names: FxHashSet<String>,
}

impl AssignmentTaint {
    fn new(impacted_names: &FxHashSet<String>, namespace_open: bool) -> Self {
        Self {
            names: impacted_names.clone(),
            namespace_open,
            clean_names: FxHashSet::default(),
        }
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

    fn record_uncertain_namespace_write(&mut self) {
        self.namespace_open = true;
        self.clean_names.clear();
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;
    use rustc_hash::FxHashSet;

    use super::DefiniteWriteCollector;
    use super::TouchedNames;

    #[test]
    fn direct_imports_touch_python_bound_names_in_clause_order() {
        let module = parse_module("import alpha.beta, gamma.delta as local\n")
            .expect("imports should parse")
            .into_syntax();
        let touched = TouchedNames::from_body(&module.body);

        assert_eq!(
            touched.names,
            ["alpha".to_string(), "local".to_string()]
                .into_iter()
                .collect()
        );
        assert!(!touched.all);
    }

    #[test]
    fn definite_writes_reject_attribute_and_subscript_targets() {
        let module = parse_module("root.attribute = source\nroot['key'] = source\nname = source\n")
            .expect("assignments should parse")
            .into_syntax();

        assert_eq!(
            DefiniteWriteCollector::collect(&module.body, &FxHashSet::default(), false),
            ["name".to_string()].into_iter().collect()
        );
    }
}
