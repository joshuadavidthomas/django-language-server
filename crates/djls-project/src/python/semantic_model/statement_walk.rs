use djls_source::Span;
use ruff_python_ast as ast;

use super::control_flow::BranchPath;
use super::control_flow::IfBranches;
use super::control_flow::Truthiness;
use super::control_flow::if_branches;
use super::control_flow::is_irrefutable_match_case;
use super::control_flow::try_paths;
use crate::ast::RangedExt;

pub(super) trait StatementInterpreter {
    type State: Clone;

    fn walk_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAssign);
    fn walk_ann_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAnnAssign);
    fn walk_aug_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAugAssign);
    fn walk_import(&mut self, state: &mut Self::State, import: &ast::StmtImport);
    fn walk_import_from(&mut self, state: &mut Self::State, import: &ast::StmtImportFrom);
    fn walk_expr(&mut self, state: &mut Self::State, expr: &ast::StmtExpr);
    fn bind_for_target(&mut self, state: &mut Self::State, target: &ast::Expr);
    fn bind_with_target(&mut self, state: &mut Self::State, target: &ast::Expr);
    fn bind_function(&mut self, state: &mut Self::State, function: &ast::StmtFunctionDef);
    fn bind_class(&mut self, state: &mut Self::State, class: &ast::StmtClassDef);
    fn bind_delete_target(&mut self, state: &mut Self::State, target: &ast::Expr);
    fn bind_type_alias(&mut self, state: &mut Self::State, alias: &ast::StmtTypeAlias);
    fn bind_pattern_names(&mut self, state: &mut Self::State, pattern: &ast::Pattern);
    fn evaluate_test(&self, state: &Self::State, expr: &ast::Expr) -> Truthiness;
    fn degrade_loop_bodies(
        &mut self,
        state: Self::State,
        bodies: &[&[ast::Stmt]],
        control_span: Span,
    ) -> Self::State;
    fn join_ambiguous_paths(
        &mut self,
        state: Self::State,
        paths: &[BranchPath<'_>],
        control_span: Span,
    ) -> Self::State;
    fn join_match_cases(
        &mut self,
        state: Self::State,
        cases: &[ast::MatchCase],
        control_span: Span,
    ) -> Self::State;
}

pub(super) fn walk_body<S>(semantics: &mut S, mut state: S::State, body: &[ast::Stmt]) -> S::State
where
    S: StatementInterpreter,
{
    for stmt in body {
        state = walk_stmt(semantics, state, stmt);
    }
    state
}

fn walk_stmt<S>(semantics: &mut S, mut state: S::State, stmt: &ast::Stmt) -> S::State
where
    S: StatementInterpreter,
{
    match stmt {
        ast::Stmt::Assign(assign) => semantics.walk_assign(&mut state, assign),
        ast::Stmt::AnnAssign(assign) => semantics.walk_ann_assign(&mut state, assign),
        ast::Stmt::AugAssign(assign) => semantics.walk_aug_assign(&mut state, assign),
        ast::Stmt::Expr(expr) => semantics.walk_expr(&mut state, expr),
        ast::Stmt::Import(import) => semantics.walk_import(&mut state, import),
        ast::Stmt::ImportFrom(import) => semantics.walk_import_from(&mut state, import),
        ast::Stmt::If(stmt_if) => return walk_if(semantics, state, stmt_if),
        ast::Stmt::For(stmt_for) => {
            semantics.bind_for_target(&mut state, &stmt_for.target);
            return semantics.degrade_loop_bodies(
                state,
                &[&stmt_for.body, &stmt_for.orelse],
                stmt_for.span(),
            );
        }
        ast::Stmt::While(stmt_while) => {
            return match semantics.evaluate_test(&state, &stmt_while.test) {
                Truthiness::AlwaysFalse => walk_body(semantics, state, &stmt_while.orelse),
                Truthiness::AlwaysTrue | Truthiness::Ambiguous => semantics.degrade_loop_bodies(
                    state,
                    &[&stmt_while.body, &stmt_while.orelse],
                    stmt_while.span(),
                ),
            };
        }
        ast::Stmt::With(stmt_with) => {
            for item in &stmt_with.items {
                if let Some(optional_vars) = &item.optional_vars {
                    semantics.bind_with_target(&mut state, optional_vars);
                }
            }
            return walk_body(semantics, state, &stmt_with.body);
        }
        ast::Stmt::Try(stmt_try) => return walk_try(semantics, state, stmt_try),
        ast::Stmt::FunctionDef(function) => semantics.bind_function(&mut state, function),
        ast::Stmt::ClassDef(class) => semantics.bind_class(&mut state, class),
        ast::Stmt::Delete(delete) => {
            for target in &delete.targets {
                semantics.bind_delete_target(&mut state, target);
            }
        }
        ast::Stmt::TypeAlias(type_alias) => semantics.bind_type_alias(&mut state, type_alias),
        ast::Stmt::Match(stmt_match) => return walk_match(semantics, state, stmt_match),
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
    state
}

fn walk_if<S>(semantics: &mut S, state: S::State, stmt_if: &ast::StmtIf) -> S::State
where
    S: StatementInterpreter,
{
    match if_branches(
        &state,
        &stmt_if.test,
        &stmt_if.body,
        &stmt_if.elif_else_clauses,
        |state, expr| semantics.evaluate_test(state, expr),
    ) {
        IfBranches::Deterministic(Some(body)) => walk_body(semantics, state, body),
        IfBranches::Deterministic(None) => state,
        IfBranches::Ambiguous(arms) => {
            let paths: Vec<BranchPath<'_>> =
                arms.iter().map(|arm| BranchPath::Single(arm)).collect();
            semantics.join_ambiguous_paths(state, &paths, stmt_if.span())
        }
    }
}

fn walk_try<S>(semantics: &mut S, state: S::State, stmt_try: &ast::StmtTry) -> S::State
where
    S: StatementInterpreter,
{
    if stmt_try.handlers.is_empty() {
        let state = walk_body(semantics, state, &stmt_try.body);
        let state = walk_body(semantics, state, &stmt_try.orelse);
        return walk_body(semantics, state, &stmt_try.finalbody);
    }

    let state = semantics.join_ambiguous_paths(state, &try_paths(stmt_try), stmt_try.span());
    walk_body(semantics, state, &stmt_try.finalbody)
}

fn walk_match<S>(semantics: &mut S, mut state: S::State, stmt_match: &ast::StmtMatch) -> S::State
where
    S: StatementInterpreter,
{
    if stmt_match.cases.is_empty() {
        return state;
    }

    if stmt_match.cases.len() == 1 && is_irrefutable_match_case(&stmt_match.cases[0]) {
        semantics.bind_pattern_names(&mut state, &stmt_match.cases[0].pattern);
        return walk_body(semantics, state, &stmt_match.cases[0].body);
    }

    semantics.join_match_cases(state, &stmt_match.cases, stmt_match.span())
}
