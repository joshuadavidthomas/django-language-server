use std::collections::HashMap;
use std::collections::HashSet;

use djls_source::Span;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;

use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::python::evaluation::name_analysis::pattern_bound_names;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;

/// Complete lexical binding inventory for one Python body.
///
/// The visitor follows control flow in the current scope, including named
/// expressions and match patterns, but does not enter nested function or class
/// bodies. Definition headers are evaluated in the containing scope.
#[derive(Default)]
pub(crate) struct ScopeBindings {
    pub(crate) writes: HashMap<String, usize>,
    pub(crate) globals: HashSet<String>,
    pub(crate) nonlocals: HashSet<String>,
    unknown_star_imports: Vec<Span>,
}

impl ScopeBindings {
    pub(crate) fn collect(body: &[Stmt]) -> Self {
        let mut collector = Self::default();
        collector.visit_body(body);
        collector
    }

    pub(crate) fn record_target(&mut self, target: &Expr) {
        record_target_writes(target, &mut self.writes);
    }

    pub(crate) fn has_unknown_star_import(&self) -> bool {
        !self.unknown_star_imports.is_empty()
    }

    pub(crate) fn has_one_closed_write(&self, name: &str, write: Span) -> bool {
        self.writes.get(name) == Some(&1)
            && self
                .unknown_star_imports
                .iter()
                .all(|import| import.start() < write.start())
    }

    fn record_name(&mut self, name: &str) {
        *self.writes.entry(name.to_string()).or_insert(0) += 1;
    }

    fn record_unknown_star_import(&mut self, import: &ruff_python_ast::StmtImportFrom) {
        self.unknown_star_imports.push(import.span());
    }

    fn visit_definition_expressions(&mut self, function: &StmtFunctionDef) {
        for decorator in &function.decorator_list {
            self.visit_expr(&decorator.expression);
        }
        for parameter in function
            .parameters
            .posonlyargs
            .iter()
            .chain(&function.parameters.args)
            .chain(&function.parameters.kwonlyargs)
        {
            if let Some(default) = &parameter.default {
                self.visit_expr(default);
            }
        }
    }
}

impl<'a> Visitor<'a> for ScopeBindings {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(function) => {
                self.record_name(function.name.as_str());
                self.visit_definition_expressions(function);
                return;
            }
            Stmt::ClassDef(class) => {
                self.record_name(class.name.as_str());
                for decorator in &class.decorator_list {
                    self.visit_expr(&decorator.expression);
                }
                if let Some(arguments) = &class.arguments {
                    for argument in &arguments.args {
                        self.visit_expr(argument);
                    }
                    for keyword in &arguments.keywords {
                        self.visit_expr(&keyword.value);
                    }
                }
                return;
            }
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    self.record_target(target);
                }
            }
            Stmt::AnnAssign(assign) => self.record_target(&assign.target),
            Stmt::AugAssign(assign) => self.record_target(&assign.target),
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    self.record_target(target);
                }
            }
            Stmt::For(statement) => self.record_target(&statement.target),
            Stmt::With(statement) => {
                for item in &statement.items {
                    if let Some(target) = &item.optional_vars {
                        self.record_target(target);
                    }
                }
            }
            Stmt::TypeAlias(alias) => self.record_target(&alias.name),
            Stmt::Import(import) => {
                for clause in DirectImportClause::lower(import) {
                    self.record_name(clause.bound());
                }
            }
            Stmt::ImportFrom(import) => {
                let syntax = FromImportSyntax::lower(import);
                if syntax.has_star() {
                    self.record_unknown_star_import(import);
                }
                for clause in syntax.named_members() {
                    self.record_name(clause.bound());
                }
            }
            Stmt::Global(global) => {
                self.globals
                    .extend(global.names.iter().map(ToString::to_string));
                return;
            }
            Stmt::Nonlocal(nonlocal) => {
                self.nonlocals
                    .extend(nonlocal.names.iter().map(ToString::to_string));
                return;
            }
            Stmt::If(_)
            | Stmt::While(_)
            | Stmt::Try(_)
            | Stmt::Match(_)
            | Stmt::Expr(_)
            | Stmt::Return(_)
            | Stmt::Raise(_)
            | Stmt::Assert(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {}
        }
        ruff_python_ast::visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Named(named) = expr {
            self.record_target(&named.target);
        }
        if !matches!(expr, Expr::Lambda(_)) {
            walk_expr(self, expr);
        }
    }

    fn visit_except_handler(&mut self, exception: &'a ruff_python_ast::ExceptHandler) {
        let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = exception;
        if let Some(name) = &handler.name {
            self.record_name(name.as_str());
        }
        ruff_python_ast::visitor::walk_except_handler(self, exception);
    }

    fn visit_pattern(&mut self, pattern: &'a ruff_python_ast::Pattern) {
        for name in pattern_bound_names(pattern) {
            self.record_name(name);
        }
    }
}

fn record_target_writes(target: &Expr, writes: &mut HashMap<String, usize>) {
    if let Some(name) = target.name_target() {
        *writes.entry(name.to_string()).or_insert(0) += 1;
        return;
    }
    match target {
        Expr::Tuple(tuple) => {
            for element in &tuple.elts {
                record_target_writes(element, writes);
            }
        }
        Expr::List(list) => {
            for element in &list.elts {
                record_target_writes(element, writes);
            }
        }
        Expr::Starred(starred) => record_target_writes(&starred.value, writes),
        Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::Compare(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Call(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Attribute(_)
        | Expr::Subscript(_)
        | Expr::Name(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => {}
    }
}
