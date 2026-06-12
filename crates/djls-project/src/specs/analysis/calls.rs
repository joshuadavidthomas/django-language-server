//! Intra-module function call resolution via Salsa tracked functions.

use ruff_python_ast::Stmt;
use ruff_python_ast::statement_visitor::StatementVisitor;
use ruff_python_ast::statement_visitor::walk_stmt;

use crate::specs::HelperCall;
use crate::specs::analysis::CallContext;
use crate::specs::analysis::state::AbstractValue;
use crate::specs::analysis::state::Env;
use crate::specs::analysis::state::TokenSplit;
use crate::specs::analyze_helper;

/// A hashable representation of `AbstractValue` for Salsa interned keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum AbstractValueKey {
    Unknown,
    Token,
    Parser,
    SplitResult(TokenSplit),
    SplitElement(crate::specs::types::SplitPosition),
    SplitLength(TokenSplit),
    Int(i64),
    Str(String),
    Other,
}

impl From<&AbstractValue> for AbstractValueKey {
    fn from(v: &AbstractValue) -> Self {
        match v {
            AbstractValue::Unknown => AbstractValueKey::Unknown,
            AbstractValue::Token => AbstractValueKey::Token,
            AbstractValue::Parser => AbstractValueKey::Parser,
            AbstractValue::SplitResult(split) => AbstractValueKey::SplitResult(*split),
            AbstractValue::SplitElement { index } => AbstractValueKey::SplitElement(*index),
            AbstractValue::SplitLength(split) => AbstractValueKey::SplitLength(*split),
            AbstractValue::Int(n) => AbstractValueKey::Int(*n),
            AbstractValue::Str(s) => AbstractValueKey::Str(s.clone()),
            AbstractValue::Tuple(_) => AbstractValueKey::Other,
        }
    }
}

impl From<&AbstractValueKey> for AbstractValue {
    fn from(k: &AbstractValueKey) -> Self {
        match k {
            AbstractValueKey::Unknown | AbstractValueKey::Other => AbstractValue::Unknown,
            AbstractValueKey::Token => AbstractValue::Token,
            AbstractValueKey::Parser => AbstractValue::Parser,
            AbstractValueKey::SplitResult(split) => AbstractValue::SplitResult(*split),
            AbstractValueKey::SplitElement(index) => AbstractValue::SplitElement { index: *index },
            AbstractValueKey::SplitLength(split) => AbstractValue::SplitLength(*split),
            AbstractValueKey::Int(n) => AbstractValue::Int(*n),
            AbstractValueKey::Str(s) => AbstractValue::Str(s.clone()),
        }
    }
}

/// Resolve a function call to a module-local helper.
///
/// When a Salsa database and file are available (`ctx.db` and `ctx.file`
/// are `Some`), constructs a `HelperCall` interned value and delegates to
/// `analyze_helper` — a Salsa tracked function with cycle recovery and
/// automatic memoization. This replaces manual caching, depth limits, and
/// self-recursion guards.
///
/// When running without Salsa (standalone extraction), returns `Unknown`
/// for all helper calls.
///
/// Returns `Unknown` if:
/// - No Salsa database is available (standalone extraction)
/// - The callee is not found in the parsed module
/// - Multiple return statements yield different abstract values
/// - A cycle is detected (Salsa cycle recovery returns `Unknown`)
pub(crate) fn resolve_call(
    callee_name: &str,
    args: &[AbstractValue],
    ctx: &mut CallContext<'_>,
) -> AbstractValue {
    // When Salsa is available, use tracked function with cycle recovery
    if let (Some(db), Some(file)) = (ctx.db, ctx.file) {
        let arg_keys: Vec<AbstractValueKey> = args.iter().map(AbstractValueKey::from).collect();
        let call = HelperCall::new(db, file, callee_name.to_string(), arg_keys);
        return analyze_helper(db, call);
    }

    // No Salsa database — cannot resolve helper calls
    AbstractValue::Unknown
}

/// Extract the abstract return value from a function body.
///
/// Scans for `return expr` statements. If exactly one return path yields
/// a non-Unknown value, returns that. If multiple yields differ, returns Unknown.
pub(crate) fn extract_return_value(body: &[Stmt], env: &mut Env) -> AbstractValue {
    let mut visitor = ReturnVisitor::new(env);
    visitor.visit_body(body);

    if visitor.returns.is_empty() {
        return AbstractValue::Unknown;
    }

    // If all returns are the same, use that value
    let first = &visitor.returns[0];
    if visitor.returns.iter().all(|r| r == first) {
        return first.clone();
    }

    // Filter out Unknown values — if a single non-Unknown value remains, use it
    let non_unknown: Vec<_> = visitor
        .returns
        .iter()
        .filter(|r| !matches!(r, AbstractValue::Unknown))
        .collect();

    if non_unknown.len() == 1 {
        return non_unknown[0].clone();
    }
    if non_unknown.len() > 1 && non_unknown.iter().all(|r| *r == non_unknown[0]) {
        return non_unknown[0].clone();
    }

    AbstractValue::Unknown
}

struct ReturnVisitor<'a> {
    env: &'a mut Env,
    returns: Vec<AbstractValue>,
}

impl<'a> ReturnVisitor<'a> {
    fn new(env: &'a mut Env) -> Self {
        Self {
            env,
            returns: Vec::new(),
        }
    }
}

impl StatementVisitor<'_> for ReturnVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Return(ret) => {
                let value = ret.value.as_deref().map_or(AbstractValue::Unknown, |expr| {
                    crate::specs::analysis::expressions::eval_expr(expr, self.env)
                });
                self.returns.push(value);
            }
            // Only collect returns from the current function scope.
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            _ => walk_stmt(self, stmt),
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/support/specs_analysis_calls.rs"]
mod tests;
