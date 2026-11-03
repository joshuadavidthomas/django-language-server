use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::ControlFlow;

use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtAssign;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::visitor::Visitor;
use ruff_python_ast::visitor::walk_expr;

use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::python::ScopeBindings;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;

const MAX_STATIC_VALUES: usize = 32;
const MAX_RESOLUTION_DEPTH: usize = 16;

/// Closed constants shared by every compile function in one source module.
#[derive(Clone, Debug, Default, PartialEq, Eq, salsa::SalsaValue)]
pub(crate) struct StaticBindings {
    values: HashMap<String, AbstractValue>,
    module_bound_names: HashSet<String>,
    module_names_open: bool,
}

/// Resolve module constants once per file revision. Registered tags share this
/// Salsa product rather than rescanning a large module for each function.
#[salsa::tracked(returns(ref))]
pub(crate) fn module_static_bindings(
    db: &dyn djls_source::Db,
    file: djls_source::File,
) -> StaticBindings {
    let Ok(Some(module)) = crate::python::RecoveredPythonModule::from_file(db, file) else {
        return StaticBindings::default();
    };
    StaticBindings::from_module(module.body(db))
}

impl StaticBindings {
    pub(crate) fn from_module(module: &[Stmt]) -> Self {
        let resolver = StaticResolver::new(module);
        let mut values = HashMap::new();
        for name in resolver.assignments.keys() {
            if let Some(value) = resolver.resolve_name(name) {
                values.insert(name.clone(), value);
            }
        }
        let mut mutated_class_roots = None;
        for (class_name, class) in &resolver.classes {
            if !resolver
                .bindings
                .has_one_closed_write(class_name, class.span())
                || class_is_dynamic(class)
            {
                continue;
            }
            let candidates = resolve_class_dict_keys(class);
            if candidates.is_empty()
                || mutated_class_roots
                    .get_or_insert_with(|| static_path_roots_mutated_in(module))
                    .contains(class_name.as_str())
            {
                continue;
            }
            for (attribute, value) in candidates {
                values.insert(format!("{class_name}.{attribute}"), value);
            }
        }
        Self {
            values,
            module_bound_names: resolver.bindings.writes.keys().cloned().collect(),
            module_names_open: resolver.bindings.has_unknown_star_import(),
        }
    }
}

impl StaticBindings {
    /// Module constants that `function` reads by bare name without local shadowing.
    ///
    /// Syntactic readers such as block-spec stop tokens use this view instead of
    /// running the abstract interpreter.
    pub(crate) fn visible_in<'a>(&'a self, function: &StmtFunctionDef) -> VisibleConstants<'a> {
        let function_bindings = FunctionBindings::collect(function);
        VisibleConstants {
            values: self
                .values
                .iter()
                .filter(|(path, _)| {
                    !path.contains('.') && !function_bindings.blocks_module_fallback(path)
                })
                .map(|(path, value)| (path.as_str(), value))
                .collect(),
        }
    }
}

#[derive(Default)]
pub(crate) struct VisibleConstants<'a> {
    values: HashMap<&'a str, &'a AbstractValue>,
}

impl VisibleConstants<'_> {
    pub(crate) fn get(&self, name: &str) -> Option<&AbstractValue> {
        self.values.get(name).copied()
    }
}

/// Seed one function environment while applying Python's function-wide local
/// shadowing rule.
pub(crate) fn seed_static_bindings(
    bindings: &StaticBindings,
    function: &StmtFunctionDef,
    env: &mut Env,
) {
    let function_bindings = FunctionBindings::collect(function);
    for (path, value) in &bindings.values {
        let root = path.split('.').next().unwrap_or(path);
        if function_bindings.blocks_module_fallback(root) {
            // Python decides local and nonlocal scope for the whole function,
            // including reads that occur before the assignment. Preserve a
            // known parameter value while still blocking static fallback.
            env.shadow_static(root.to_string());
        } else {
            env.set_static(path.clone(), value.clone());
        }
    }

    if !bindings.module_names_open && !function_bindings.names_open {
        let mut shadowed_names = bindings.module_bound_names.clone();
        shadowed_names.extend(function_bindings.bound_names);
        env.set_builtin_name_scope(shadowed_names);
    }
}

struct StaticResolver<'a> {
    assignments: HashMap<String, &'a Expr>,
    bindings: ScopeBindings,
    classes: HashMap<String, &'a ruff_python_ast::StmtClassDef>,
    mutated_names: HashSet<String>,
}

impl<'a> StaticResolver<'a> {
    fn new(module: &'a [Stmt]) -> Self {
        let mut assignments = HashMap::new();
        let mut classes = HashMap::new();

        for stmt in module {
            if let Stmt::Assign(StmtAssign { targets, value, .. }) = stmt
                && let [target] = targets.as_slice()
                && let Some(name) = target.name_target()
            {
                assignments.insert(name.to_string(), value.as_ref());
            }
            if let Stmt::ClassDef(class) = stmt {
                classes.insert(class.name.to_string(), class);
            }
        }

        let mut bindings = ScopeBindings::collect(module);
        collect_nested_global_effects(module, &mut bindings.writes);

        let mut mutated_names = HashSet::new();
        collect_mutated_roots(module, &mut mutated_names);
        collect_nested_global_mutations(module, &mut mutated_names);

        Self {
            assignments,
            bindings,
            classes,
            mutated_names,
        }
    }

    fn resolve_name(&self, name: &str) -> Option<AbstractValue> {
        self.resolve_name_inner(name, &mut Vec::new(), 0)
    }

    fn resolve_name_inner(
        &self,
        name: &str,
        resolving: &mut Vec<String>,
        depth: usize,
    ) -> Option<AbstractValue> {
        let expression = *self.assignments.get(name)?;
        if depth >= MAX_RESOLUTION_DEPTH
            || !self.bindings.has_one_closed_write(name, expression.span())
            || self.mutated_names.contains(name)
            || resolving.iter().any(|current| current == name)
        {
            return None;
        }
        resolving.push(name.to_string());
        let value = self.resolve_expr(expression, resolving, depth + 1);
        resolving.pop();
        value
    }

    fn resolve_expr(
        &self,
        expr: &Expr,
        resolving: &mut Vec<String>,
        depth: usize,
    ) -> Option<AbstractValue> {
        if depth >= MAX_RESOLUTION_DEPTH {
            return None;
        }
        if let Some(value) = expr.string_literal() {
            return Some(AbstractValue::Str(value.to_string()));
        }
        if let Some(value) = expr.non_negative_integer() {
            return i64::try_from(value).ok().map(AbstractValue::Int);
        }
        if let Some(name) = expr.name_target() {
            return self.resolve_name_inner(name, resolving, depth + 1);
        }

        // Module-level lists and sets remain mutable through aliases and
        // arbitrary calls. Only tuples can retain exact collection evidence.
        let Expr::Tuple(tuple) = expr else {
            return None;
        };
        let elements = &tuple.elts;
        if elements.is_empty() || elements.len() > MAX_STATIC_VALUES {
            return None;
        }
        let mut values = Vec::with_capacity(elements.len());
        for element in elements {
            let value = self.resolve_expr(element, resolving, depth + 1)?;
            if !matches!(value, AbstractValue::Str(_)) {
                return None;
            }
            values.push(value);
        }
        Some(AbstractValue::Tuple(values))
    }
}

fn class_is_dynamic(class: &ruff_python_ast::StmtClassDef) -> bool {
    !class.decorator_list.is_empty()
        || class.arguments.as_ref().is_some_and(|arguments| {
            arguments.keywords.iter().any(|keyword| {
                keyword.arg.is_none()
                    || keyword.arg.as_ref().is_some_and(|name| name == "metaclass")
            })
        })
}

fn collect_nested_global_effects(body: &[Stmt], writes: &mut HashMap<String, usize>) {
    walk_stmts(body, Recurse::IntoClasses, |stmt| {
        if let Stmt::FunctionDef(function) = stmt {
            let nested = ScopeBindings::collect(&function.body);
            for name in &nested.globals {
                if nested.writes.contains_key(name) {
                    *writes.entry(name.clone()).or_insert(0) += 1;
                }
            }
            collect_nested_global_effects(&function.body, writes);
        }
        ControlFlow::Continue(())
    });
}

fn collect_nested_global_mutations(body: &[Stmt], mutated: &mut HashSet<String>) {
    walk_stmts(body, Recurse::IntoClasses, |stmt| {
        if let Stmt::FunctionDef(function) = stmt {
            let globals = ScopeBindings::collect(&function.body).globals;
            if !globals.is_empty() {
                let mut nested_mutations = HashSet::new();
                collect_mutated_roots(&function.body, &mut nested_mutations);
                mutated.extend(
                    nested_mutations
                        .into_iter()
                        .filter(|name| globals.contains(name)),
                );
            }
            collect_nested_global_mutations(&function.body, mutated);
        }
        ControlFlow::Continue(())
    });
}

fn resolve_class_dict_keys(class: &ruff_python_ast::StmtClassDef) -> Vec<(String, AbstractValue)> {
    let bindings = ScopeBindings::collect(&class.body);
    let mut direct = HashMap::<String, &Expr>::new();
    for stmt in &class.body {
        if let Stmt::Assign(assign) = stmt
            && let [target] = assign.targets.as_slice()
            && let Some(name) = target.name_target()
        {
            direct.insert(name.to_string(), assign.value.as_ref());
        }
    }

    direct
        .into_iter()
        .filter_map(|(attribute, expression)| {
            let Expr::Dict(dict) = expression else {
                return None;
            };
            if !bindings.has_one_closed_write(&attribute, expression.span())
                || class_local_mapping_is_mutated_or_escaped(&class.body, &attribute)
            {
                return None;
            }
            if dict.items.is_empty() || dict.items.len() > MAX_STATIC_VALUES {
                return None;
            }
            let mut values = Vec::with_capacity(dict.items.len());
            for item in &dict.items {
                let key = item.key.as_ref()?.string_literal()?.to_string();
                if !class_mapping_value_is_static(&item.value) {
                    return None;
                }
                let value = AbstractValue::Str(key);
                if !values.contains(&value) {
                    values.push(value);
                }
            }
            Some((attribute, AbstractValue::Tuple(values)))
        })
        .collect()
}

fn class_local_mapping_is_mutated_or_escaped(body: &[Stmt], name: &str) -> bool {
    let mut unsafe_use = false;
    walk_stmts(body, Recurse::ControlFlow, |stmt| {
        let is_name = |expression: &Expr| expression.name_target() == Some(name);
        let mutates = |target: &Expr| match target {
            Expr::Attribute(attribute) => is_name(&attribute.value),
            Expr::Subscript(subscript) => is_name(&subscript.value),
            Expr::BoolOp(_)
            | Expr::Named(_)
            | Expr::BinOp(_)
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
            | Expr::Compare(_)
            | Expr::Call(_)
            | Expr::FString(_)
            | Expr::TString(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::Starred(_)
            | Expr::Name(_)
            | Expr::List(_)
            | Expr::Tuple(_)
            | Expr::Slice(_)
            | Expr::IpyEscapeCommand(_) => false,
        };
        unsafe_use |= match stmt {
            Stmt::Assign(assign) => assign.targets.iter().any(mutates) || is_name(&assign.value),
            Stmt::AnnAssign(assign) => {
                mutates(&assign.target) || assign.value.as_deref().is_some_and(is_name)
            }
            Stmt::AugAssign(assign) => mutates(&assign.target),
            Stmt::Delete(delete) => delete.targets.iter().any(mutates),
            Stmt::Expr(statement) => matches!(statement.value.as_ref(), Expr::Call(call) if
                matches!(call.func.as_ref(), Expr::Attribute(attribute) if is_name(&attribute.value))
                || call.arguments.args.iter().any(is_name)
                || call.arguments.keywords.iter().any(|keyword| is_name(&keyword.value))),
            Stmt::FunctionDef(_)
            | Stmt::ClassDef(_)
            | Stmt::Return(_)
            | Stmt::TypeAlias(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Raise(_)
            | Stmt::Try(_)
            | Stmt::Assert(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => false,
        };
        if unsafe_use {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    unsafe_use
}

fn class_mapping_value_is_static(value: &Expr) -> bool {
    matches!(
        value,
        Expr::Name(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
    )
}

/// The root name and segment count of a dotted name path such as `A.b.c`.
fn path_root(expression: &Expr) -> Option<(&str, usize)> {
    if let Expr::Name(name) = expression {
        return Some((name.id.as_str(), 1));
    }
    let Expr::Attribute(attribute) = expression else {
        return None;
    };
    path_root(&attribute.value).map(|(root, length)| (root, length + 1))
}

/// Record every path root that `expression` exposes as an object.
fn collect_escaped_roots<'a>(expression: &'a Expr, roots: &mut HashSet<&'a str>) {
    struct RootObserver<'a, 'r> {
        roots: &'r mut HashSet<&'a str>,
    }

    impl<'a> Visitor<'a> for RootObserver<'a, '_> {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Some((root, _)) = path_root(expr) {
                self.roots.insert(root);
                return;
            }
            if let Expr::Call(call) = expr {
                // Calling the class constructs an instance; it does not expose
                // the class object. Arguments may still contain an alias.
                for argument in &call.arguments.args {
                    self.visit_expr(argument);
                }
                for keyword in &call.arguments.keywords {
                    self.visit_expr(&keyword.value);
                }
                return;
            }
            walk_expr(self, expr);
        }
    }

    RootObserver { roots }.visit_expr(expression);
}

/// Roots whose static attribute paths `body`, including nested scopes,
/// mutates or lets escape. One walk serves every class in the module.
fn static_path_roots_mutated_in(body: &[Stmt]) -> HashSet<&str> {
    let mut roots = HashSet::new();
    collect_static_path_mutations(body, &mut roots);
    roots
}

fn collect_static_path_mutations<'a>(body: &'a [Stmt], roots: &mut HashSet<&'a str>) {
    walk_stmts(body, Recurse::WithinScope, |stmt| {
        let mut record_target = |target: &'a Expr| {
            let path = if let Expr::Subscript(subscript) = target {
                path_root(&subscript.value)
            } else {
                path_root(target)
            };
            if let Some((root, length)) = path
                && length > 1
            {
                roots.insert(root);
            }
        };
        match stmt {
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    record_target(target);
                }
                collect_escaped_roots(&assign.value, roots);
            }
            Stmt::AnnAssign(assign) => {
                record_target(&assign.target);
                if let Some(value) = assign.value.as_deref() {
                    collect_escaped_roots(value, roots);
                }
            }
            Stmt::AugAssign(assign) => record_target(&assign.target),
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    record_target(target);
                }
            }
            Stmt::Expr(expression) => {
                if let Expr::Call(call) = expression.value.as_ref() {
                    if let Some((root, length)) = path_root(&call.func)
                        && length > 2
                    {
                        roots.insert(root);
                    }
                    for argument in &call.arguments.args {
                        collect_escaped_roots(argument, roots);
                    }
                    for keyword in &call.arguments.keywords {
                        collect_escaped_roots(&keyword.value, roots);
                    }
                }
            }
            Stmt::Return(statement) => {
                if let Some(value) = statement.value.as_deref() {
                    collect_escaped_roots(value, roots);
                }
            }
            Stmt::FunctionDef(function) => collect_static_path_mutations(&function.body, roots),
            Stmt::ClassDef(class) => collect_static_path_mutations(&class.body, roots),
            Stmt::TypeAlias(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Try(_)
            | Stmt::Raise(_)
            | Stmt::Assert(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {}
        }
        ControlFlow::Continue(())
    });
}

fn collect_mutated_roots(body: &[Stmt], mutated: &mut HashSet<String>) {
    walk_stmts(body, Recurse::WithinScope, |stmt| {
        let mut record_target = |target: &Expr| {
            let base = match target {
                Expr::Subscript(subscript) => subscript.value.as_ref(),
                Expr::Attribute(attribute) => attribute.value.as_ref(),
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
                | Expr::Starred(_)
                | Expr::Name(_)
                | Expr::List(_)
                | Expr::Tuple(_)
                | Expr::Slice(_)
                | Expr::IpyEscapeCommand(_) => return,
            };
            if let Some((root, _)) = path_root(base) {
                mutated.insert(root.to_string());
            }
        };
        match stmt {
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    record_target(target);
                }
            }
            Stmt::AnnAssign(assign) => record_target(&assign.target),
            Stmt::AugAssign(assign) => record_target(&assign.target),
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    record_target(target);
                }
            }
            Stmt::Expr(expression) => {
                if let Expr::Call(call) = expression.value.as_ref()
                    && let Expr::Attribute(method) = call.func.as_ref()
                    && let Some((root, _)) = path_root(&method.value)
                {
                    mutated.insert(root.to_string());
                }
            }
            Stmt::FunctionDef(_)
            | Stmt::ClassDef(_)
            | Stmt::TypeAlias(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Try(_)
            | Stmt::Return(_)
            | Stmt::Raise(_)
            | Stmt::Assert(_)
            | Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {}
        }
        ControlFlow::Continue(())
    });
}

struct FunctionBindings {
    locals: HashSet<String>,
    nonlocals: HashSet<String>,
    bound_names: HashSet<String>,
    names_open: bool,
}

impl FunctionBindings {
    fn collect(function: &StmtFunctionDef) -> Self {
        let scope = ScopeBindings::collect(&function.body);
        let mut locals = scope.writes.keys().cloned().collect::<HashSet<_>>();
        for parameter in function
            .parameters
            .posonlyargs
            .iter()
            .chain(&function.parameters.args)
            .chain(&function.parameters.kwonlyargs)
        {
            locals.insert(parameter.parameter.name.to_string());
        }
        if let Some(parameter) = &function.parameters.vararg {
            locals.insert(parameter.name.to_string());
        }
        if let Some(parameter) = &function.parameters.kwarg {
            locals.insert(parameter.name.to_string());
        }
        locals.retain(|name| !scope.globals.contains(name) && !scope.nonlocals.contains(name));
        let names_open = scope.has_unknown_star_import();
        let mut bound_names = scope.writes.keys().cloned().collect::<HashSet<_>>();
        bound_names.extend(locals.iter().cloned());
        bound_names.extend(scope.nonlocals.iter().cloned());
        Self {
            locals,
            nonlocals: scope.nonlocals,
            bound_names,
            names_open,
        }
    }

    fn blocks_module_fallback(&self, name: &str) -> bool {
        self.names_open || self.locals.contains(name) || self.nonlocals.contains(name)
    }
}
