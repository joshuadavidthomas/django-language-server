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
use crate::python::evaluation::name_analysis::pattern_bound_names;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;
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
        for (class_name, class) in &resolver.classes {
            if !resolver
                .bindings
                .has_one_closed_write(class_name, class.span())
                || class_is_dynamic(class)
            {
                continue;
            }
            for (attribute, value) in resolve_class_dict_keys(module, class_name, class) {
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

#[derive(Default)]
struct ScopeBindings {
    writes: HashMap<String, usize>,
    globals: HashSet<String>,
    nonlocals: HashSet<String>,
    unknown_star_imports: Vec<djls_source::Span>,
}

impl ScopeBindings {
    fn collect(body: &[Stmt]) -> Self {
        let mut collector = Self::default();
        collector.visit_body(body);
        collector
    }

    fn record_name(&mut self, name: &str) {
        *self.writes.entry(name.to_string()).or_insert(0) += 1;
    }

    fn record_target(&mut self, target: &Expr) {
        record_target_writes(target, &mut self.writes);
    }

    fn record_unknown_star_import(&mut self, import: &ruff_python_ast::StmtImportFrom) {
        self.unknown_star_imports.push(import.span());
    }

    fn has_unknown_star_import(&self) -> bool {
        !self.unknown_star_imports.is_empty()
    }

    fn has_one_closed_write(&self, name: &str, write: djls_source::Span) -> bool {
        self.writes.get(name) == Some(&1)
            && self
                .unknown_star_imports
                .iter()
                .all(|import| import.start() < write.start())
    }

    fn visit_definition_expressions(&mut self, function: &ruff_python_ast::StmtFunctionDef) {
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

fn resolve_class_dict_keys(
    module: &[Stmt],
    class_name: &str,
    class: &ruff_python_ast::StmtClassDef,
) -> Vec<(String, AbstractValue)> {
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

    let candidates: Vec<_> = direct
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
        .collect();
    if !candidates.is_empty() && body_static_path_is_mutated(module, class_name) {
        return Vec::new();
    }
    candidates
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

fn expression_contains_static_root(expression: &Expr, root: &str) -> bool {
    struct RootObserver<'a> {
        root: &'a str,
        found: bool,
    }

    impl<'ast> Visitor<'ast> for RootObserver<'_> {
        fn visit_expr(&mut self, expr: &'ast Expr) {
            if expr
                .path_segments()
                .is_some_and(|path| path.first().is_some_and(|name| name == self.root))
            {
                self.found = true;
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

    let mut observer = RootObserver { root, found: false };
    observer.visit_expr(expression);
    observer.found
}

fn body_static_path_is_mutated(body: &[Stmt], class_name: &str) -> bool {
    let mut mutated = false;
    walk_stmts(body, Recurse::WithinScope, |stmt| {
        let target_mutates = |target: &Expr| {
            target.path_segments().is_some_and(|path| {
                path.first().is_some_and(|root| root == class_name) && path.len() > 1
            }) || matches!(target, Expr::Subscript(subscript) if subscript.value.path_segments().is_some_and(|path| path.first().is_some_and(|root| root == class_name) && path.len() > 1))
        };
        let path_escapes =
            |expression: &Expr| expression_contains_static_root(expression, class_name);
        mutated |= match stmt {
            Stmt::Assign(assign) => {
                assign.targets.iter().any(target_mutates) || path_escapes(&assign.value)
            }
            Stmt::AnnAssign(assign) => {
                target_mutates(&assign.target) || assign.value.as_deref().is_some_and(path_escapes)
            }
            Stmt::AugAssign(assign) => target_mutates(&assign.target),
            Stmt::Delete(delete) => delete.targets.iter().any(target_mutates),
            Stmt::Expr(expression) => {
                matches!(expression.value.as_ref(), Expr::Call(call) if
                    call.func.path_segments().is_some_and(|path| path.first().is_some_and(|root| root == class_name) && path.len() > 2)
                    || call.arguments.args.iter().any(path_escapes)
                    || call.arguments.keywords.iter().any(|keyword| path_escapes(&keyword.value)))
            }
            Stmt::Return(statement) => statement.value.as_deref().is_some_and(path_escapes),
            Stmt::FunctionDef(function) => body_static_path_is_mutated(&function.body, class_name),
            Stmt::ClassDef(class) => body_static_path_is_mutated(&class.body, class_name),
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
            | Stmt::IpyEscapeCommand(_) => false,
        };
        if mutated {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    mutated
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
            if let Some(root) = base.path_segments().and_then(|path| path.first().cloned()) {
                mutated.insert(root);
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
                    && let Some(root) = method
                        .value
                        .path_segments()
                        .and_then(|path| path.first().cloned())
                {
                    mutated.insert(root);
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
