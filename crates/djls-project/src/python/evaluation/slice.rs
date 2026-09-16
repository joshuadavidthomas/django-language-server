use ruff_python_ast as ast;
use ruff_python_ast::visitor::Visitor;
use rustc_hash::FxHashSet;

use super::name_analysis::expr_read_names;

/// Deliberately finite: settings are requested together to preserve correlated alternatives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EvaluationDemand {
    Full,
    Settings,
}

pub(super) enum SelectedStatement<'a> {
    Full(&'a ast::Stmt),
    UnobservedAssignment(&'a ast::StmtAssign),
}

impl<'a> From<&'a ast::Stmt> for SelectedStatement<'a> {
    fn from(statement: &'a ast::Stmt) -> Self {
        Self::Full(statement)
    }
}

#[derive(Default)]
struct ObservedNames(FxHashSet<String>);

impl<'a> Visitor<'a> for ObservedNames {
    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        if let ast::Expr::Name(name) = expression {
            self.0.insert(name.id.to_string());
        }
        ruff_python_ast::visitor::walk_expr(self, expression);
    }
}

/// Select whole top-level regions, then let the ordinary evaluator execute them in source order.
/// An unclassified effect is a barrier: its entire prefix is needed, including otherwise unused
/// aliases, import bindings, and predicate identities. Within that prefix, a globally unobserved
/// assignment may omit construction only after the evaluator certifies its value cannot carry
/// relevant alias effects. This separates backward value demand from forward effect execution.
/// Syntax recovery uses the full module.
pub(super) fn selected_statements(
    body: &[ast::Stmt],
    demand: EvaluationDemand,
    has_syntax_errors: bool,
) -> Vec<SelectedStatement<'_>> {
    if demand == EvaluationDemand::Full || has_syntax_errors {
        return body.iter().map(SelectedStatement::Full).collect();
    }
    let mut needed: FxHashSet<String> = ["INSTALLED_APPS", "TEMPLATES"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut selected = Vec::new();
    for (index, statement) in body.iter().enumerate().rev() {
        let mut reads = FxHashSet::default();
        let mut writes = FxHashSet::default();
        if !pure_region(statement, &mut reads, &mut writes) {
            selected.extend(body[..=index].iter().rev());
            break;
        }
        if !writes.is_disjoint(&needed) {
            // No kills: retaining previous definitions also preserves predicate provenance and
            // conditional fallthrough. Relevant compound regions remain entirely intact.
            needed.extend(reads);
            selected.push(statement);
        }
    }
    selected.reverse();
    // Observation can only add names to `needed`. Avoid walking the original body when
    // none of the retained assignments could qualify for the omission certificate.
    if !selected.iter().any(|statement| {
        matches!(statement, ast::Stmt::Assign(assign)
            if matches!(assign.targets.as_slice(), [ast::Expr::Name(name)]
                if !needed.contains(name.id.as_str())))
    }) {
        return selected.into_iter().map(SelectedStatement::Full).collect();
    }
    let mut observed = ObservedNames(needed);
    for statement in body {
        if let ast::Stmt::Assign(assign) = statement
            && matches!(assign.targets.as_slice(), [ast::Expr::Name(_)])
        {
            observed.visit_expr(&assign.value);
        } else {
            observed.visit_stmt(statement);
            // Imports at module scope are executed in full. Open writes inside a compound
            // region, however, can cause its degradation to observe any earlier binding.
            if !matches!(statement, ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_)) {
                let Some(writes) = super::touched_names::closed_write_names(statement) else {
                    return selected.into_iter().map(SelectedStatement::Full).collect();
                };
                observed.0.extend(writes);
            }
        }
    }
    selected
        .into_iter()
        .map(|statement| {
            if let ast::Stmt::Assign(assign) = statement
                && let [ast::Expr::Name(name)] = assign.targets.as_slice()
                && !observed.0.contains(name.id.as_str())
            {
                SelectedStatement::UnobservedAssignment(assign)
            } else {
                SelectedStatement::Full(statement)
            }
        })
        .collect()
}

fn pure_region(
    statement: &ast::Stmt,
    reads: &mut FxHashSet<String>,
    writes: &mut FxHashSet<String>,
) -> bool {
    match statement {
        ast::Stmt::Assign(assign) => {
            let [ast::Expr::Name(target)] = assign.targets.as_slice() else {
                return false;
            };
            writes.insert(target.id.to_string());
            reads.extend(expr_read_names(&assign.value));
            pure_expression(&assign.value)
        }
        ast::Stmt::If(branch) => {
            reads.extend(expr_read_names(&branch.test));
            pure_expression(&branch.test)
                && branch
                    .body
                    .iter()
                    .all(|stmt| pure_region(stmt, reads, writes))
                && branch.elif_else_clauses.iter().all(|clause| {
                    if let Some(test) = &clause.test {
                        reads.extend(expr_read_names(test));
                        if !pure_expression(test) {
                            return false;
                        }
                    }
                    clause
                        .body
                        .iter()
                        .all(|stmt| pure_region(stmt, reads, writes))
                })
        }
        ast::Stmt::Pass(_) => true,
        // In particular, never classify an import by whether its bound name is demanded.
        ast::Stmt::FunctionDef(_)
        | ast::Stmt::ClassDef(_)
        | ast::Stmt::Return(_)
        | ast::Stmt::Delete(_)
        | ast::Stmt::TypeAlias(_)
        | ast::Stmt::AugAssign(_)
        | ast::Stmt::AnnAssign(_)
        | ast::Stmt::For(_)
        | ast::Stmt::While(_)
        | ast::Stmt::With(_)
        | ast::Stmt::Match(_)
        | ast::Stmt::Raise(_)
        | ast::Stmt::Try(_)
        | ast::Stmt::Assert(_)
        | ast::Stmt::Import(_)
        | ast::Stmt::ImportFrom(_)
        | ast::Stmt::Global(_)
        | ast::Stmt::Nonlocal(_)
        | ast::Stmt::Expr(_)
        | ast::Stmt::Break(_)
        | ast::Stmt::Continue(_)
        | ast::Stmt::IpyEscapeCommand(_) => false,
    }
}

/// A whitelist, not a blacklist: new evaluator syntax must establish its effect contract before
/// it can be omitted. Calls (even apparent intrinsics), attributes, and named expressions remain
/// barriers because their meaning can depend on aliases or open namespaces.
fn pure_expression(expression: &ast::Expr) -> bool {
    match expression {
        ast::Expr::Name(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::BytesLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NoneLiteral(_)
        | ast::Expr::EllipsisLiteral(_) => true,
        ast::Expr::List(list) => list.elts.iter().all(pure_expression),
        ast::Expr::Tuple(tuple) => tuple.elts.iter().all(pure_expression),
        ast::Expr::Dict(dict) => dict.items.iter().all(|item| {
            item.key.as_ref().is_none_or(pure_expression) && pure_expression(&item.value)
        }),
        ast::Expr::BinOp(binary) => pure_expression(&binary.left) && pure_expression(&binary.right),
        ast::Expr::UnaryOp(unary) => pure_expression(&unary.operand),
        ast::Expr::BoolOp(boolean) => boolean.values.iter().all(pure_expression),
        ast::Expr::If(branch) => {
            pure_expression(&branch.test)
                && pure_expression(&branch.body)
                && pure_expression(&branch.orelse)
        }
        ast::Expr::Named(_)
        | ast::Expr::Lambda(_)
        | ast::Expr::Set(_)
        | ast::Expr::ListComp(_)
        | ast::Expr::SetComp(_)
        | ast::Expr::DictComp(_)
        | ast::Expr::Generator(_)
        | ast::Expr::Await(_)
        | ast::Expr::Yield(_)
        | ast::Expr::YieldFrom(_)
        | ast::Expr::Compare(_)
        | ast::Expr::Call(_)
        | ast::Expr::FString(_)
        | ast::Expr::TString(_)
        | ast::Expr::Attribute(_)
        | ast::Expr::Subscript(_)
        | ast::Expr::Starred(_)
        | ast::Expr::Slice(_)
        | ast::Expr::IpyEscapeCommand(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::EvaluationDemand;
    use super::SelectedStatement;
    use super::selected_statements;

    #[test]
    fn settings_slice_observes_declarations_and_nested_writes() {
        for observer in [
            r"@decorate(UNUSED)
def f(): pass",
            "def f(arg=UNUSED): pass",
            "def f(arg: UNUSED): pass",
            "def f() -> UNUSED: pass",
            "class C(UNUSED): pass",
            r"for item in unknown:
    UNUSED = []",
            r"if FLAG:
    import other as UNUSED",
            r"match unknown:
    case UNUSED: pass",
            r"if FLAG:
    from other import *",
        ] {
            let source = format!(
                r"UNUSED = {{'key': unknown.attr}}
{observer}
INSTALLED_APPS = []
TEMPLATES = []
"
            );
            let parsed = ruff_python_parser::parse_module(&source)
                .expect("fixture should parse")
                .into_syntax();
            let selected = selected_statements(&parsed.body, EvaluationDemand::Settings, false);
            assert!(
                matches!(selected[0], SelectedStatement::Full(_)),
                "{source}"
            );
        }
    }
}
