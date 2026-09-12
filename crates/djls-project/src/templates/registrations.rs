use std::collections::BTreeMap;

use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtExpr;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::visitor;
use ruff_python_ast::visitor::Visitor;

use super::filters::FilterArityMap;
use super::libraries::TemplateLibraryId;
use super::names::TemplateSymbolName;
use super::symbols::SymbolDefinition;
use super::symbols::SymbolKey;
use super::symbols::TemplateSymbol;
use super::symbols::TemplateSymbolKind;
use super::symbols::TemplateSymbolSource;
use super::tags::BlockSpec;
use super::tags::BlockSpecs;
use super::tags::TagRuleMap;
use super::tags::blocks::EndTagEvidence;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::db::Db as ProjectDb;
use crate::python::PythonFunctionDefinition;
use crate::python::PythonSourceLookup;
use crate::python::PythonSourceModule;
use crate::python::RecoveredPythonModule;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;

/// Information about a single tag or filter registration found in source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistrationInfo {
    name: String,
    kind: RegistrationKind,
    callable: RegistrationCallable,
    options: RegistrationOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistrationOptions {
    pub(crate) context: ContextProvision,
    pub(crate) block_end: Option<RegisteredEnd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextProvision {
    None,
    Context,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegisteredEnd {
    Default,
    Named(String),
    Unknown,
}

impl RegistrationOptions {
    fn resolve(
        kind: RegistrationKind,
        takes_context: Option<&Expr>,
        end_name: Option<&Expr>,
        mut python_facts: Option<&mut PythonSourceLookup<'_>>,
    ) -> Self {
        let context = match takes_context {
            None | Some(Expr::NoneLiteral(_)) => ContextProvision::None,
            Some(value) => match value.bool_literal().or_else(|| {
                python_facts
                    .as_mut()
                    .and_then(|facts| facts.exact_bool(value))
            }) {
                Some(true) => ContextProvision::Context,
                Some(false) => ContextProvision::None,
                None => ContextProvision::Unknown,
            },
        };
        let block_end = matches!(kind, RegistrationKind::SimpleBlockTag).then(|| match end_name {
            None | Some(Expr::NoneLiteral(_)) => RegisteredEnd::Default,
            Some(value) => python_facts
                .and_then(|facts| facts.exact_string(value))
                .or_else(|| value.string_literal().map(str::to_string))
                .map_or(RegisteredEnd::Unknown, RegisteredEnd::Named),
        });
        Self { context, block_end }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RegistrationCallable {
    DecoratedLocal {
        function_name: String,
        navigation: Option<LocalFunctionSource>,
    },
    ResolvedFunction(PythonFunctionDefinition),
    Unresolved(Option<String>),
}

impl RegistrationInfo {
    #[cfg(test)]
    fn func_name(&self) -> Option<&str> {
        match &self.callable {
            RegistrationCallable::DecoratedLocal { function_name, .. } => Some(function_name),
            RegistrationCallable::ResolvedFunction(_) => None,
            RegistrationCallable::Unresolved(function_name) => function_name.as_deref(),
        }
    }

    #[cfg(test)]
    fn local_source(&self) -> Option<LocalFunctionSource> {
        match self.callable {
            RegistrationCallable::DecoratedLocal { navigation, .. } => navigation,
            RegistrationCallable::ResolvedFunction(_) | RegistrationCallable::Unresolved(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalFunctionSource {
    definition_span: djls_source::Span,
    name_span: djls_source::Span,
}

impl LocalFunctionSource {
    fn from_function(function: &StmtFunctionDef) -> Self {
        Self {
            definition_span: function.span(),
            name_span: function.name.span(),
        }
    }
}

/// The style of registration, distinguishing decorator helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegistrationKind {
    Tag,
    SimpleTag,
    InclusionTag,
    SimpleBlockTag,
    Filter,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RegistrationInventory {
    #[default]
    NotLibrary,
    Observed,
    Open,
}

#[derive(Debug, Default)]
struct RegistrationSourceAnalysis {
    registrations: Vec<RegistrationInfo>,
    inventory: RegistrationInventory,
}

impl RegistrationSourceAnalysis {
    fn observe_fresh_library(&mut self) {
        self.registrations.clear();
        self.inventory = RegistrationInventory::Observed;
    }

    fn observe_register_use(&mut self) {
        if matches!(self.inventory, RegistrationInventory::NotLibrary) {
            self.inventory = RegistrationInventory::Open;
        }
    }

    fn open_inventory(&mut self) {
        self.inventory = RegistrationInventory::Open;
    }

    fn defines_library(&self) -> bool {
        !matches!(self.inventory, RegistrationInventory::NotLibrary)
    }

    fn inventory_is_open(&self) -> bool {
        matches!(self.inventory, RegistrationInventory::Open)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RegisterReference {
    #[default]
    Absent,
    Present,
}

struct RegisterUseVisitor {
    reference: RegisterReference,
}

impl<'a> Visitor<'a> for RegisterUseVisitor {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if expr.name_target() == Some("register") {
            self.reference = RegisterReference::Present;
            return;
        }
        visitor::walk_expr(self, expr);
    }
}

fn contains_register(expr: &Expr) -> bool {
    let mut visitor = RegisterUseVisitor {
        reference: RegisterReference::Absent,
    };
    visitor.visit_expr(expr);
    matches!(visitor.reference, RegisterReference::Present)
}

fn statement_contains_register(stmt: &Stmt) -> bool {
    let mut visitor = RegisterUseVisitor {
        reference: RegisterReference::Absent,
    };
    visitor.visit_stmt(stmt);
    matches!(visitor.reference, RegisterReference::Present)
}

fn body_contains_register(body: &[Stmt]) -> bool {
    body.iter().any(statement_contains_register)
}

struct NamedBindingVisitor {
    found: bool,
}

impl<'a> Visitor<'a> for NamedBindingVisitor {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if matches!(expr, Expr::Named(_)) {
            self.found = true;
            return;
        }
        visitor::walk_expr(self, expr);
    }
}

fn statement_contains_named_binding(stmt: &Stmt) -> bool {
    let mut visitor = NamedBindingVisitor { found: false };
    visitor.visit_stmt(stmt);
    visitor.found
}

fn collect_from_class_body(body: &[Stmt], analysis: &mut RegistrationSourceAnalysis) {
    let mut register_is_module_binding = true;
    for stmt in body {
        if let Stmt::FunctionDef(function) = stmt
            && register_is_module_binding
        {
            collect_from_decorated_function(function, None, None, analysis);
            if body_contains_register(&function.body) {
                analysis.open_inventory();
            }
            continue;
        }
        if let Stmt::ClassDef(class) = stmt {
            collect_from_class_body(&class.body, analysis);
            continue;
        }
        if let Stmt::Assign(assign) = stmt
            && assign
                .targets
                .iter()
                .any(|target| target.name_target() == Some("register"))
        {
            register_is_module_binding = false;
        }
    }
}

fn is_register_inventory_target(expr: &Expr) -> bool {
    if let Expr::Subscript(subscript) = expr {
        return is_register_inventory_target(&subscript.value);
    }
    expr.path_segments().is_some_and(|path| {
        matches!(path.as_slice(), [register, inventory, ..]
            if register == "register" && matches!(inventory.as_str(), "tags" | "filters"))
    })
}

fn call_rooted_at_register(call: &ExprCall) -> bool {
    call.func
        .path_segments()
        .is_some_and(|path| path.first().is_some_and(|root| root == "register"))
}

fn call_escapes_register(call: &ExprCall) -> bool {
    contains_register(&call.func)
        || call.arguments.args.iter().any(contains_register)
        || call
            .arguments
            .keywords
            .iter()
            .any(|keyword| contains_register(&keyword.value))
}

fn is_fresh_canonical_library(
    expr: &Expr,
    template_is_django: bool,
    library_constructor: Option<&str>,
) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
        return false;
    }
    if template_is_django
        && call.func.path_segments().is_some_and(|path| {
            matches!(path.as_slice(), [template, library]
                if template == "template" && library == "Library")
        })
    {
        return true;
    }
    call.func
        .name_target()
        .is_some_and(|name| Some(name) == library_constructor)
}

fn is_canonical_library_import(syntax: &FromImportSyntax, module_name: &str) -> bool {
    (syntax.level() == 0
        && matches!(
            syntax.module(),
            Some("django.template" | "django.template.library")
        ))
        || (syntax.level() == 1
            && syntax.module() == Some("library")
            && module_name.starts_with("django.template."))
}

fn invalidate_transparent_decorator_target(
    functions: &mut BTreeMap<String, LocalFunctionSource>,
    target: &Expr,
) {
    if let Some(name) = target.name_target() {
        functions.remove(name);
    } else if !matches!(target, Expr::Attribute(_) | Expr::Subscript(_)) {
        functions.clear();
    }
}

#[allow(clippy::too_many_lines)]
fn analyze_registrations_from_body_in_module(
    body: &[Stmt],
    module_name: &str,
    mut python_facts: Option<&mut PythonSourceLookup<'_>>,
) -> RegistrationSourceAnalysis {
    let mut analysis = RegistrationSourceAnalysis::default();
    let mut transparent_decorated_functions = BTreeMap::new();
    let mut template_is_django = false;
    let mut library_constructor = None;

    for stmt in body {
        if statement_contains_named_binding(stmt) {
            transparent_decorated_functions.clear();
        }
        match stmt {
            Stmt::Import(import) => {
                for clause in DirectImportClause::lower(import) {
                    if clause.bound() == "template" {
                        template_is_django = false;
                    }
                    if library_constructor == Some(clause.bound()) {
                        library_constructor = None;
                    }
                    if clause.bound() == "register" {
                        analysis.registrations.clear();
                        analysis.open_inventory();
                    }
                    transparent_decorated_functions.remove(clause.bound());
                }
            }
            Stmt::ImportFrom(import) => {
                let syntax = FromImportSyntax::lower(import);
                if syntax.has_star() {
                    template_is_django = false;
                    library_constructor = None;
                    transparent_decorated_functions.clear();
                }
                let canonical_library_import = is_canonical_library_import(&syntax, module_name);
                for member in syntax.named_members() {
                    if member.bound() == "template" {
                        template_is_django = syntax.level() == 0
                            && syntax.module() == Some("django")
                            && member.imported() == "template";
                    }
                    if library_constructor == Some(member.bound()) {
                        library_constructor = None;
                    }
                    if canonical_library_import && member.imported() == "Library" {
                        library_constructor = Some(member.bound());
                    }
                    if member.bound() == "register" {
                        analysis.registrations.clear();
                        analysis.open_inventory();
                    }
                    transparent_decorated_functions.remove(member.bound());
                }
            }
            Stmt::Assign(assign) => {
                let binds_register = assign
                    .targets
                    .iter()
                    .any(|target| target.name_target() == Some("register"));
                let binds_template = assign
                    .targets
                    .iter()
                    .any(|target| target.name_target() == Some("template"));
                let shares_register_binding = binds_register
                    && (assign.targets.len() != 1
                        || assign.targets[0].name_target() != Some("register"));
                if assign.targets.iter().any(|target| {
                    target
                        .name_target()
                        .is_some_and(|name| library_constructor == Some(name))
                }) {
                    library_constructor = None;
                }
                if binds_register {
                    let fresh_canonical = is_fresh_canonical_library(
                        &assign.value,
                        template_is_django,
                        library_constructor,
                    );
                    if fresh_canonical && !binds_template && !shares_register_binding {
                        analysis.observe_fresh_library();
                    } else {
                        analysis.registrations.clear();
                        analysis.open_inventory();
                    }
                }
                if assign.targets.iter().any(is_register_inventory_target)
                    || (contains_register(&assign.value) && !binds_register)
                {
                    analysis.open_inventory();
                }
                if binds_template {
                    template_is_django = false;
                }
                for target in &assign.targets {
                    invalidate_transparent_decorator_target(
                        &mut transparent_decorated_functions,
                        target,
                    );
                }
            }
            Stmt::AnnAssign(assign) => {
                if assign.target.name_target() == Some("register") {
                    analysis.registrations.clear();
                    analysis.open_inventory();
                }
                if is_register_inventory_target(&assign.target)
                    || assign.value.as_deref().is_some_and(contains_register)
                {
                    analysis.open_inventory();
                }
                if assign.target.name_target() == Some("template") {
                    template_is_django = false;
                }
                if assign
                    .target
                    .name_target()
                    .is_some_and(|name| library_constructor == Some(name))
                {
                    library_constructor = None;
                }
                invalidate_transparent_decorator_target(
                    &mut transparent_decorated_functions,
                    &assign.target,
                );
            }
            Stmt::AugAssign(assign) => {
                if assign.target.name_target() == Some("register")
                    || is_register_inventory_target(&assign.target)
                    || contains_register(&assign.value)
                {
                    analysis.open_inventory();
                }
                if assign.target.name_target() == Some("template") {
                    template_is_django = false;
                }
                if assign
                    .target
                    .name_target()
                    .is_some_and(|name| library_constructor == Some(name))
                {
                    library_constructor = None;
                }
                invalidate_transparent_decorator_target(
                    &mut transparent_decorated_functions,
                    &assign.target,
                );
            }
            Stmt::Delete(delete) => {
                if delete.targets.iter().any(|target| {
                    target.name_target() == Some("register")
                        || is_register_inventory_target(target)
                        || contains_register(target)
                }) {
                    analysis.open_inventory();
                }
                if delete
                    .targets
                    .iter()
                    .any(|target| target.name_target() == Some("template"))
                {
                    template_is_django = false;
                }
                if delete.targets.iter().any(|target| {
                    target
                        .name_target()
                        .is_some_and(|name| library_constructor == Some(name))
                }) {
                    library_constructor = None;
                }
                for target in &delete.targets {
                    invalidate_transparent_decorator_target(
                        &mut transparent_decorated_functions,
                        target,
                    );
                }
            }
            Stmt::FunctionDef(function) => {
                if function.name.as_str() == "template" {
                    template_is_django = false;
                }
                if library_constructor == Some(function.name.as_str()) {
                    library_constructor = None;
                }
                if function.name.as_str() == "register" {
                    analysis.registrations.clear();
                    analysis.open_inventory();
                }
                let local_source = LocalFunctionSource::from_function(function);
                collect_from_decorated_function(
                    function,
                    Some(local_source),
                    python_facts.as_deref_mut(),
                    &mut analysis,
                );
                if body_contains_register(&function.body) {
                    analysis.open_inventory();
                }
                if !function.decorator_list.is_empty()
                    && function.decorator_list.iter().all(|decorator| {
                        registration_decorator_rooted_at_register(&decorator.expression)
                    })
                {
                    transparent_decorated_functions.insert(function.name.to_string(), local_source);
                } else {
                    transparent_decorated_functions.remove(function.name.as_str());
                }
            }
            Stmt::ClassDef(class) => {
                if class.name.as_str() == "template" {
                    template_is_django = false;
                }
                if library_constructor == Some(class.name.as_str()) {
                    library_constructor = None;
                }
                if class.name.as_str() == "register" {
                    analysis.registrations.clear();
                    analysis.open_inventory();
                }
                collect_from_class_body(&class.body, &mut analysis);
                if statement_contains_register(stmt) {
                    analysis.open_inventory();
                }
                transparent_decorated_functions.remove(class.name.as_str());
            }
            Stmt::Expr(StmtExpr { value, .. }) => {
                if let Expr::Call(call) = value.as_ref() {
                    collect_from_call_statement(
                        call,
                        &transparent_decorated_functions,
                        python_facts.as_deref_mut(),
                        &mut analysis,
                    );
                } else if contains_register(value) {
                    analysis.open_inventory();
                }
            }
            Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Try(_) => {
                template_is_django = false;
                library_constructor = None;
                transparent_decorated_functions.clear();
                if statement_contains_register(stmt) {
                    analysis.open_inventory();
                }
            }
            Stmt::TypeAlias(_) => {
                transparent_decorated_functions.clear();
                if statement_contains_register(stmt) {
                    analysis.open_inventory();
                }
            }
            Stmt::Return(_)
            | Stmt::Raise(_)
            | Stmt::Assert(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {
                if statement_contains_register(stmt) {
                    analysis.open_inventory();
                }
            }
        }
    }

    analysis
}

#[cfg(test)]
fn analyze_registrations_from_body(body: &[Stmt]) -> RegistrationSourceAnalysis {
    analyze_registrations_from_body_in_module(body, "", None)
}

fn for_each_registration<'db>(
    db: &'db dyn ProjectDb,
    analysis: &RegistrationSourceAnalysis,
    body: &'db [Stmt],
    registration_file: djls_source::File,
    module_name: &str,
    mut f: impl FnMut(
        &RegistrationInfo,
        Option<(&'db StmtFunctionDef, djls_source::File, bool)>,
        SymbolKey,
    ),
) {
    let func_defs = collect_func_defs(body);

    for reg in &analysis.registrations {
        let func = match &reg.callable {
            RegistrationCallable::DecoratedLocal {
                function_name,
                navigation,
            } => func_defs
                .iter()
                .find(|function| {
                    function.name.as_str() == function_name
                        && navigation.is_none_or(|source| function.span() == source.definition_span)
                })
                .copied()
                .map(|function| (function, registration_file, false)),
            RegistrationCallable::ResolvedFunction(definition) => {
                definition.statement(db).map(|function| {
                    let file = definition.file();
                    (function, file, file != registration_file)
                })
            }
            RegistrationCallable::Unresolved(_) => None,
        };

        let kind = reg.kind;
        let key = SymbolKey {
            registration_module: module_name.to_string(),
            name: reg.name.clone(),
            kind: kind.symbol_kind(),
        };

        f(reg, func, key);
    }
}

/// Collect module-level function definitions that can own definite registrations.
fn collect_func_defs(body: &[Stmt]) -> Vec<&StmtFunctionDef> {
    body.iter()
        .filter_map(|stmt| {
            if let Stmt::FunctionDef(function) = stmt {
                Some(function)
            } else {
                None
            }
        })
        .collect()
}

/// Extract registrations from a decorated function definition.
fn collect_from_decorated_function(
    func_def: &StmtFunctionDef,
    local_source: Option<LocalFunctionSource>,
    mut python_facts: Option<&mut PythonSourceLookup<'_>>,
    analysis: &mut RegistrationSourceAnalysis,
) {
    // Decorators execute from the function outward, so registrations must be recorded bottom-up.
    for (index, decorator) in func_def.decorator_list.iter().enumerate().rev() {
        let expression = &decorator.expression;
        if !registration_decorator_rooted_at_register(expression) {
            if contains_register(expression) {
                analysis.open_inventory();
            }
            continue;
        }
        analysis.observe_register_use();

        let navigation = local_source.filter(|_| {
            func_def.decorator_list[index + 1..]
                .iter()
                .all(|decorator| registration_decorator_rooted_at_register(&decorator.expression))
        });
        let applied = LoweredCallable::Decorated {
            function: func_def,
            navigation,
        };
        let Some(lowered) = lower_registration_expression(expression, Some(applied)) else {
            analysis.open_inventory();
            continue;
        };
        let Some(registration) =
            registration_from_lowered(lowered, &BTreeMap::new(), python_facts.as_deref_mut())
        else {
            analysis.open_inventory();
            continue;
        };
        analysis.registrations.push(registration);
    }
}

fn direct_register_helper(expr: &Expr) -> Option<&str> {
    let Expr::Attribute(ExprAttribute { value, attr, .. }) = expr else {
        return None;
    };
    (value.name_target() == Some("register")).then_some(attr.as_str())
}

fn registration_kind(helper: &str) -> Option<RegistrationKind> {
    match helper {
        "tag" => Some(RegistrationKind::Tag),
        "simple_tag" => Some(RegistrationKind::SimpleTag),
        "inclusion_tag" => Some(RegistrationKind::InclusionTag),
        "simple_block_tag" => Some(RegistrationKind::SimpleBlockTag),
        "filter" => Some(RegistrationKind::Filter),
        _ => None,
    }
}

fn registration_decorator_rooted_at_register(expr: &Expr) -> bool {
    let helper = if matches!(expr, Expr::Attribute(_)) {
        direct_register_helper(expr)
    } else if let Expr::Call(call) = expr {
        direct_register_helper(&call.func)
    } else {
        None
    };
    helper.and_then(registration_kind).is_some()
}

#[derive(Clone, Copy, Debug)]
enum LoweredCallable<'a> {
    Expression(&'a Expr),
    Decorated {
        function: &'a StmtFunctionDef,
        navigation: Option<LocalFunctionSource>,
    },
}

#[derive(Clone, Copy, Debug)]
struct LoweredRegistration<'a> {
    kind: RegistrationKind,
    name: Option<&'a Expr>,
    callable: Option<LoweredCallable<'a>>,
    takes_context: Option<&'a Expr>,
    end_name: Option<&'a Expr>,
}

fn lower_registration_expression<'a>(
    expression: &'a Expr,
    applied: Option<LoweredCallable<'a>>,
) -> Option<LoweredRegistration<'a>> {
    let mut lowered = if matches!(expression, Expr::Attribute(_)) {
        let helper = direct_register_helper(expression)?;
        let kind = registration_kind(helper)?;
        if matches!(kind, RegistrationKind::InclusionTag) {
            return None;
        }
        LoweredRegistration {
            kind,
            name: None,
            callable: None,
            takes_context: None,
            end_name: None,
        }
    } else if let Expr::Call(call) = expression {
        lower_registration_call(call, applied.is_some())?
    } else {
        return None;
    };
    if let Some(applied) = applied {
        if lowered.callable.is_some() {
            return None;
        }
        lowered.callable = Some(applied);
    }
    Some(lowered)
}

#[allow(clippy::too_many_lines)]
fn lower_registration_call(
    call: &ExprCall,
    has_applied_callable: bool,
) -> Option<LoweredRegistration<'_>> {
    let helper = direct_register_helper(&call.func)?;
    let args = &call.arguments.args;
    let keywords = &call.arguments.keywords;
    if args
        .iter()
        .any(|argument| matches!(argument, Expr::Starred(_)))
        || keywords.iter().any(|keyword| keyword.arg.is_none())
    {
        return None;
    }

    let keyword = |names: &[&str]| {
        keywords.iter().find_map(|keyword| {
            let name = keyword.arg.as_ref()?;
            names.contains(&name.as_str()).then_some(&keyword.value)
        })
    };
    let keywords_are_supported = |supported: &[&str]| {
        keywords.iter().all(|keyword| {
            keyword
                .arg
                .as_ref()
                .is_some_and(|name| supported.contains(&name.as_str()))
        })
    };

    let (kind, name, callable) = match helper {
        "tag" if keywords_are_supported(&["name", "compile_function"]) => {
            let name_keyword = keyword(&["name"]);
            let callable_keyword = keyword(&["compile_function"]);
            match &args[..] {
                [name, callable]
                    if name_keyword.is_none()
                        && callable_keyword.is_none()
                        && !matches!(name, Expr::NoneLiteral(_)) =>
                {
                    (RegistrationKind::Tag, Some(name), Some(callable))
                }
                [name] if has_applied_callable && name_keyword.is_none() => {
                    (RegistrationKind::Tag, Some(name), callable_keyword)
                }
                [name] if name.string_literal().is_some() && name_keyword.is_none() => {
                    (RegistrationKind::Tag, Some(name), callable_keyword)
                }
                [callable] if name_keyword.is_none() => {
                    (RegistrationKind::Tag, None, Some(callable))
                }
                [] => (RegistrationKind::Tag, name_keyword, callable_keyword),
                _ => return None,
            }
        }
        "simple_tag" if keywords_are_supported(&["func", "takes_context", "name"]) => {
            let callable_keyword = keyword(&["func"]);
            let callable = match &args[..] {
                [callable] if callable_keyword.is_none() => Some(callable),
                [] => callable_keyword,
                _ => return None,
            };
            (RegistrationKind::SimpleTag, keyword(&["name"]), callable)
        }
        "inclusion_tag"
            if keywords_are_supported(&["filename", "func", "takes_context", "name"]) =>
        {
            let filename_keyword = keyword(&["filename"]);
            if !matches!((&args[..], filename_keyword), ([_], None) | ([], Some(_))) {
                return None;
            }
            // Django accepts `func` in this signature but never reads it. The returned
            // decorator always registers the function to which it is later applied.
            (RegistrationKind::InclusionTag, keyword(&["name"]), None)
        }
        "simple_block_tag"
            if keywords_are_supported(&["func", "takes_context", "name", "end_name"]) =>
        {
            let callable_keyword = keyword(&["func"]);
            let callable = match &args[..] {
                [callable] if callable_keyword.is_none() => Some(callable),
                [] => callable_keyword,
                _ => return None,
            };
            (
                RegistrationKind::SimpleBlockTag,
                keyword(&["name"]),
                callable,
            )
        }
        // Library.filter accepts arbitrary registration flags such as is_safe and
        // needs_autoescape in both direct and decorator forms.
        "filter" => {
            let name_keyword = keyword(&["name"]);
            let callable_keyword = keyword(&["filter_func"]);
            let func_flag = keyword(&["func"]);
            match &args[..] {
                [name, callable]
                    if name_keyword.is_none()
                        && callable_keyword.is_none()
                        && !matches!(name, Expr::NoneLiteral(_)) =>
                {
                    (RegistrationKind::Filter, Some(name), Some(callable))
                }
                [name] if has_applied_callable && name_keyword.is_none() => {
                    (RegistrationKind::Filter, Some(name), callable_keyword)
                }
                [name] if name.string_literal().is_some() && name_keyword.is_none() => {
                    (RegistrationKind::Filter, Some(name), callable_keyword)
                }
                [callable] if name_keyword.is_none() => {
                    (RegistrationKind::Filter, None, Some(callable))
                }
                [] if !(has_applied_callable
                    && func_flag.is_some()
                    && name_keyword.is_none_or(|name| matches!(name, Expr::NoneLiteral(_)))) =>
                {
                    (RegistrationKind::Filter, name_keyword, callable_keyword)
                }
                _ => return None,
            }
        }
        _ => return None,
    };

    if matches!(kind, RegistrationKind::Tag | RegistrationKind::Filter)
        && args.is_empty()
        && callable.is_some()
        && name.is_none_or(|name| matches!(name, Expr::NoneLiteral(_)))
    {
        // Unlike simple_tag, these helpers require a name with their callable keyword.
        return None;
    }

    Some(LoweredRegistration {
        kind,
        name,
        callable: callable.map(LoweredCallable::Expression),
        takes_context: keyword(&["takes_context"]),
        end_name: keyword(&["end_name"]),
    })
}

/// Extract registrations from a call expression statement.
fn collect_from_call_statement(
    call: &ExprCall,
    transparent_decorated_functions: &BTreeMap<String, LocalFunctionSource>,
    python_facts: Option<&mut PythonSourceLookup<'_>>,
    analysis: &mut RegistrationSourceAnalysis,
) {
    let lowered = if call_rooted_at_register(call) {
        lower_registration_call(call, false)
    } else if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() {
        lower_registration_expression(
            &call.func,
            call.arguments.args.first().map(LoweredCallable::Expression),
        )
    } else {
        None
    };
    if lowered.is_none() && !call_escapes_register(call) {
        return;
    }
    analysis.observe_register_use();

    let Some(lowered) = lowered else {
        analysis.open_inventory();
        return;
    };
    if lowered.callable.is_none() {
        // Calling a decorator factory without applying the returned decorator does not
        // mutate the library. This includes `filter(func=...)`, where `func` is only an
        // ignored flag captured by `**flags`.
        return;
    }
    let Some(registration) =
        registration_from_lowered(lowered, transparent_decorated_functions, python_facts)
    else {
        analysis.open_inventory();
        return;
    };
    analysis.registrations.push(registration);
}

fn resolved_registration_name(
    kind: RegistrationKind,
    name: Option<&Expr>,
    callable_name: Option<&str>,
    resolve_string: impl FnOnce(&Expr) -> Option<String>,
) -> Option<String> {
    let Some(name) = name else {
        return callable_name.map(str::to_string);
    };
    if matches!(name, Expr::NoneLiteral(_)) {
        return callable_name.map(str::to_string);
    }
    let resolved = resolve_string(name)?;
    if resolved.is_empty()
        && matches!(
            kind,
            RegistrationKind::SimpleTag
                | RegistrationKind::InclusionTag
                | RegistrationKind::SimpleBlockTag
        )
    {
        callable_name.map(str::to_string)
    } else {
        Some(resolved)
    }
}

fn registration_from_lowered(
    lowered: LoweredRegistration<'_>,
    transparent_decorated_functions: &BTreeMap<String, LocalFunctionSource>,
    mut python_facts: Option<&mut PythonSourceLookup<'_>>,
) -> Option<RegistrationInfo> {
    let callable = lowered.callable?;
    match callable {
        LoweredCallable::Decorated {
            function,
            navigation,
        } => {
            let name = resolved_registration_name(
                lowered.kind,
                lowered.name,
                Some(function.name.as_str()),
                |expression| {
                    python_facts
                        .as_deref_mut()
                        .and_then(|facts| facts.exact_string(expression))
                        .or_else(|| expression.string_literal().map(str::to_string))
                },
            )?;
            let options = RegistrationOptions::resolve(
                lowered.kind,
                lowered.takes_context,
                lowered.end_name,
                python_facts,
            );
            Some(RegistrationInfo {
                name,
                kind: lowered.kind,
                callable: RegistrationCallable::DecoratedLocal {
                    function_name: function.name.to_string(),
                    navigation,
                },
                options,
            })
        }
        LoweredCallable::Expression(expression) => {
            if let Some(facts) = python_facts
                && let Some(function) = facts.function(expression)
            {
                let name = resolved_registration_name(
                    lowered.kind,
                    lowered.name,
                    Some(function.name()),
                    |expression| facts.exact_string(expression),
                )?;
                let options = RegistrationOptions::resolve(
                    lowered.kind,
                    lowered.takes_context,
                    lowered.end_name,
                    Some(facts),
                );
                return Some(RegistrationInfo {
                    name,
                    kind: lowered.kind,
                    callable: RegistrationCallable::ResolvedFunction(function),
                    options,
                });
            }

            let function_name = callable_name(expression);
            let navigation = function_name
                .as_deref()
                .and_then(|name| transparent_decorated_functions.get(name))
                .copied();
            let callable_name = navigation.and(function_name.as_deref());
            let name = resolved_registration_name(
                lowered.kind,
                lowered.name,
                callable_name,
                |expression| expression.string_literal().map(str::to_string),
            )?;
            let callable =
                function_name.map_or(RegistrationCallable::Unresolved(None), |function_name| {
                    if let Some(navigation) = navigation {
                        RegistrationCallable::DecoratedLocal {
                            function_name,
                            navigation: Some(navigation),
                        }
                    } else {
                        RegistrationCallable::Unresolved(Some(function_name))
                    }
                });
            Some(RegistrationInfo {
                name,
                kind: lowered.kind,
                callable,
                options: RegistrationOptions::resolve(
                    lowered.kind,
                    lowered.takes_context,
                    lowered.end_name,
                    None,
                ),
            })
        }
    }
}

/// Best-effort callable name extraction for debugging / registration mapping.
fn callable_name(expr: &Expr) -> Option<String> {
    if let Some(name) = expr.name_target() {
        return Some(name.to_string());
    }

    match expr {
        Expr::Attribute(ExprAttribute { value, attr, .. }) => {
            let base = callable_name(value)?;
            Some(format!("{base}.{}", attr.as_str()))
        }
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
        | Expr::Subscript(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::List(_)
        | Expr::Tuple(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryDefinitionState {
    Failed,
    ParsedNotLibrary {
        parse_quality: TemplateLibraryParseQuality,
    },
    Library {
        parse_quality: TemplateLibraryParseQuality,
        inventory: TemplateLibrarySymbolInventory,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateLibrarySymbolInventory {
    Observed,
    Open,
}

/// Equality-bearing registration facts for one Template Library source module.
#[derive(Clone, Debug, PartialEq, Eq, salsa::SalsaValue)]
pub struct TemplateLibraryDefinitionFacts<'db> {
    state: TemplateLibraryDefinitionState,
    tags: BTreeMap<String, TemplateSymbol<'db>>,
    filters: BTreeMap<String, TemplateSymbol<'db>>,
}

impl<'db> TemplateLibraryDefinitionFacts<'db> {
    #[must_use]
    pub fn is_library(&self) -> bool {
        matches!(self.state, TemplateLibraryDefinitionState::Library { .. })
    }

    #[must_use]
    pub(crate) fn is_recovered(&self) -> bool {
        matches!(
            self.state,
            TemplateLibraryDefinitionState::ParsedNotLibrary {
                parse_quality: TemplateLibraryParseQuality::Recovered,
            } | TemplateLibraryDefinitionState::Library {
                parse_quality: TemplateLibraryParseQuality::Recovered,
                ..
            }
        )
    }

    #[must_use]
    pub(crate) fn source_failed(&self) -> bool {
        matches!(self.state, TemplateLibraryDefinitionState::Failed)
    }

    #[must_use]
    pub(crate) fn symbols_are_unobserved(&self) -> bool {
        matches!(
            self.state,
            TemplateLibraryDefinitionState::Library {
                inventory: TemplateLibrarySymbolInventory::Open,
                ..
            }
        )
    }

    pub(crate) fn symbols(&self) -> impl Iterator<Item = &TemplateSymbol<'db>> {
        self.tags.values().chain(self.filters.values())
    }

    #[must_use]
    pub fn symbol(&self, kind: TemplateSymbolKind, name: &str) -> Option<&TemplateSymbol<'db>> {
        match kind {
            TemplateSymbolKind::Tag => self.tags.get(name),
            TemplateSymbolKind::Filter => self.filters.get(name),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplateLibraryParseQuality {
    Exact,
    Recovered,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TemplateLibrarySymbolSources {
    tags: BTreeMap<String, TemplateSymbolSource>,
    filters: BTreeMap<String, TemplateSymbolSource>,
}

impl TemplateLibrarySymbolSources {
    fn set(
        &mut self,
        kind: TemplateSymbolKind,
        name: String,
        source: Option<TemplateSymbolSource>,
    ) {
        let symbols = match kind {
            TemplateSymbolKind::Tag => &mut self.tags,
            TemplateSymbolKind::Filter => &mut self.filters,
        };
        if let Some(source) = source {
            symbols.insert(name, source);
        } else {
            symbols.remove(&name);
        }
    }

    fn symbol(&self, kind: TemplateSymbolKind, name: &str) -> Option<TemplateSymbolSource> {
        match kind {
            TemplateSymbolKind::Tag => self.tags.get(name),
            TemplateSymbolKind::Filter => self.filters.get(name),
        }
        .copied()
    }
}

/// Canonical indexed analysis of one Template Library source module.
///
/// Registration discovery happens here once. Equality-bearing projections below keep changes in
/// Tag Definitions, Filter Definitions, source locations, Tag Rules, Block Specs, and Filter Arity
/// independent.
#[derive(Clone, Debug, PartialEq, salsa::SalsaValue)]
struct TemplateLibrarySourceAnalysis<'db> {
    definitions: TemplateLibraryDefinitionFacts<'db>,
    symbol_sources: TemplateLibrarySymbolSources,
    registration_dependencies: Vec<djls_source::File>,
    tag_rules: TagRuleMap,
    block_specs: BlockSpecs,
    filter_arities: FilterArityMap,
}

impl TemplateLibrarySourceAnalysis<'_> {
    fn failed() -> Self {
        Self {
            definitions: TemplateLibraryDefinitionFacts {
                state: TemplateLibraryDefinitionState::Failed,
                tags: BTreeMap::new(),
                filters: BTreeMap::new(),
            },
            symbol_sources: TemplateLibrarySymbolSources::default(),
            registration_dependencies: Vec::new(),
            tag_rules: TagRuleMap::default(),
            block_specs: BlockSpecs::default(),
            filter_arities: FilterArityMap::default(),
        }
    }
}

#[allow(clippy::too_many_lines)]
#[salsa::tracked(returns(ref))]
fn template_library_source_analysis<'db>(
    db: &'db dyn ProjectDb,
    key: TemplateLibraryId<'db>,
) -> TemplateLibrarySourceAnalysis<'db> {
    let Some(file) = key.file(db) else {
        return TemplateLibrarySourceAnalysis::failed();
    };
    let Ok(Some(module)) = RecoveredPythonModule::from_file(db, file) else {
        return TemplateLibrarySourceAnalysis::failed();
    };
    let parse_quality = if module.has_ordinary_syntax_errors(db) {
        TemplateLibraryParseQuality::Recovered
    } else {
        TemplateLibraryParseQuality::Exact
    };

    let mut tags = BTreeMap::new();
    let mut filters = BTreeMap::new();
    let mut symbol_sources = TemplateLibrarySymbolSources::default();
    let mut tag_rules = TagRuleMap::default();
    let mut block_specs = BlockSpecs::default();
    let mut filter_arities = FilterArityMap::default();
    let registration_module = key.module(db).as_str();
    let project_module = db.project().and_then(|project| {
        PythonSourceModule::resolve(db, project, key.module(db).clone())
            .filter(|source| source.file() == file)
            .map(|source| (project, source))
    });
    let mut python_facts = project_module.map_or_else(
        || PythonSourceLookup::for_file(db, file),
        |(project, module)| PythonSourceLookup::for_module(db, project, module),
    );
    let registration_analysis = analyze_registrations_from_body_in_module(
        module.body(db),
        registration_module,
        Some(&mut python_facts),
    );
    let used_recovered_source = python_facts.has_recovered_source();
    let registration_dependencies = python_facts.consulted_files().to_vec();
    let mut symbols_unobserved = parse_quality == TemplateLibraryParseQuality::Recovered
        || used_recovered_source
        || registration_analysis.inventory_is_open();

    for_each_registration(
        db,
        &registration_analysis,
        module.body(db),
        file,
        registration_module,
        |registration, func, symbol_key| {
            if let Ok(name) = TemplateSymbolName::parse(&registration.name) {
                let kind = registration.kind.symbol_kind();
                let symbol = TemplateSymbol {
                    kind,
                    name,
                    definition: SymbolDefinition::Exact { library: key },
                    doc: None,
                };
                let symbol_name = symbol.name().to_string();
                match kind {
                    TemplateSymbolKind::Tag => {
                        tags.insert(symbol_name.clone(), symbol);
                    }
                    TemplateSymbolKind::Filter => {
                        filters.insert(symbol_name.clone(), symbol);
                    }
                }
                let source = (parse_quality == TemplateLibraryParseQuality::Exact
                    && !used_recovered_source
                    && !registration_analysis.inventory_is_open())
                .then(|| match &registration.callable {
                    RegistrationCallable::ResolvedFunction(_) => {
                        func.map(|(function, implementation_file, _)| {
                            TemplateSymbolSource::new(
                                implementation_file,
                                function.span(),
                                function.name.span(),
                            )
                        })
                    }
                    RegistrationCallable::DecoratedLocal {
                        navigation: Some(local_source),
                        ..
                    } => Some(TemplateSymbolSource::new(
                        file,
                        local_source.definition_span,
                        local_source.name_span,
                    )),
                    RegistrationCallable::DecoratedLocal {
                        navigation: None, ..
                    }
                    | RegistrationCallable::Unresolved(_) => None,
                })
                .flatten();
                symbol_sources.set(kind, symbol_name, source);
            } else {
                symbols_unobserved = true;
            }

            tag_rules.remove(&symbol_key);
            block_specs.0.remove(&symbol_key);
            filter_arities.remove(&symbol_key);

            let Some((func, implementation_file, imported)) = func else {
                return;
            };
            if let Some(rule) = registration.kind.extract_tag_rule(
                db,
                imported.then_some(implementation_file),
                func,
                &registration.options,
            ) {
                tag_rules.insert(symbol_key.clone(), rule.into());
            }
            if let Some(block_spec) = registration
                .kind
                .extract_block_spec(func, &registration.options)
            {
                let end_tag = match block_spec.end_tag {
                    EndTagEvidence::Literal(end_tag) => Some(end_tag),
                    EndTagEvidence::SelfNamed => Some(format!("end{}", symbol_key.name)),
                    EndTagEvidence::Unknown => None,
                };
                block_specs.insert(
                    symbol_key.clone(),
                    BlockSpec {
                        end_tag,
                        intermediates: block_spec.intermediates,
                        body_analysis_evidence: block_spec.body_analysis_evidence,
                    },
                );
            }
            if let Some(arity) = registration.kind.extract_filter_arity(func) {
                filter_arities.insert(symbol_key, arity);
            }
        },
    );

    let state =
        if registration_analysis.defines_library() || !tags.is_empty() || !filters.is_empty() {
            let inventory = if symbols_unobserved {
                TemplateLibrarySymbolInventory::Open
            } else {
                TemplateLibrarySymbolInventory::Observed
            };
            TemplateLibraryDefinitionState::Library {
                parse_quality,
                inventory,
            }
        } else {
            TemplateLibraryDefinitionState::ParsedNotLibrary { parse_quality }
        };
    TemplateLibrarySourceAnalysis {
        definitions: TemplateLibraryDefinitionFacts {
            state,
            tags,
            filters,
        },
        symbol_sources,
        registration_dependencies,
        tag_rules,
        block_specs,
        filter_arities,
    }
}

/// Independently backdatable Tag analysis for one Template Library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemplateLibraryTagFacts {
    tag_rules: TagRuleMap,
    block_specs: BlockSpecs,
}

impl TemplateLibraryTagFacts {
    #[must_use]
    pub fn tag_rules(&self) -> &TagRuleMap {
        &self.tag_rules
    }

    #[must_use]
    pub fn block_specs(&self) -> &BlockSpecs {
        &self.block_specs
    }
}

/// Independently backdatable Filter analysis for one Template Library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemplateLibraryFilterFacts {
    filter_arities: FilterArityMap,
}

impl TemplateLibraryFilterFacts {
    #[must_use]
    pub fn filter_arities(&self) -> &FilterArityMap {
        &self.filter_arities
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_library_definition_facts<'db>(
    db: &'db dyn ProjectDb,
    key: TemplateLibraryId<'db>,
) -> TemplateLibraryDefinitionFacts<'db> {
    template_library_source_analysis(db, key)
        .definitions
        .clone()
}

#[salsa::tracked(returns(ref))]
fn template_library_symbol_sources<'db>(
    db: &'db dyn ProjectDb,
    key: TemplateLibraryId<'db>,
) -> TemplateLibrarySymbolSources {
    template_library_source_analysis(db, key)
        .symbol_sources
        .clone()
}

/// Python source dependencies followed while resolving one Template Library's registrations.
#[salsa::tracked(returns(ref))]
pub fn template_library_registration_dependencies<'db>(
    db: &'db dyn ProjectDb,
    key: TemplateLibraryId<'db>,
) -> Vec<djls_source::File> {
    template_library_source_analysis(db, key)
        .registration_dependencies
        .clone()
}

#[must_use]
pub fn template_symbol_source<'db>(
    db: &'db dyn ProjectDb,
    symbol: &TemplateSymbol<'db>,
) -> Option<TemplateSymbolSource> {
    let SymbolDefinition::Exact { library } = &symbol.definition else {
        return None;
    };
    template_library_symbol_sources(db, *library).symbol(symbol.kind, symbol.name())
}

#[salsa::tracked(returns(ref))]
pub fn template_library_tag_facts<'db>(
    db: &'db dyn ProjectDb,
    key: TemplateLibraryId<'db>,
) -> TemplateLibraryTagFacts {
    let analysis = template_library_source_analysis(db, key);
    TemplateLibraryTagFacts {
        tag_rules: analysis.tag_rules.clone(),
        block_specs: analysis.block_specs.clone(),
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_library_filter_facts<'db>(
    db: &'db dyn ProjectDb,
    key: TemplateLibraryId<'db>,
) -> TemplateLibraryFilterFacts {
    TemplateLibraryFilterFacts {
        filter_arities: template_library_source_analysis(db, key)
            .filter_arities
            .clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::tags::testing::fixture_source;

    fn fixture(path: &str) -> &'static str {
        fixture_source(path).expect("requested corpus fixture should exist")
    }

    fn analyze_registrations(source: &str) -> RegistrationSourceAnalysis {
        let parsed = ruff_python_parser::parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        analyze_registrations_from_body(&module.body)
    }

    fn collect_registrations(source: &str) -> Vec<RegistrationInfo> {
        analyze_registrations(source).registrations
    }

    fn find_reg<'a>(regs: &'a [RegistrationInfo], name: &str) -> &'a RegistrationInfo {
        regs.iter()
            .find(|r| r.name == name)
            .expect("requested registration should have been collected")
    }

    fn registered_source<'a>(source: &'a str, registration: &RegistrationInfo) -> Option<&'a str> {
        let span = registration.local_source()?.definition_span;
        source.get(span.start_usize()..span.end_usize())
    }

    #[test]
    fn fresh_library_replaces_prior_registrations() {
        let source = "from django import template\nregister = template.Library()\n@register.simple_tag\ndef stale(): pass\nregister = template.Library()\n@register.simple_tag\ndef current(): pass\n";
        let registrations = collect_registrations(source);

        assert!(
            registrations
                .iter()
                .all(|registration| registration.name != "stale")
        );
        assert_eq!(
            find_reg(&registrations, "current").func_name(),
            Some("current")
        );
    }

    #[test]
    fn decorator_registration_keeps_its_local_function_source() {
        let source = "from django import template\nregister = template.Library()\n@register.tag('shown')\ndef implementation(parser, token):\n    pass\n";
        let registrations = collect_registrations(source);
        let registration = find_reg(&registrations, "shown");

        assert_eq!(
            registered_source(source, registration),
            Some("@register.tag('shown')\ndef implementation(parser, token):\n    pass")
        );
        let local_source = registration
            .local_source()
            .expect("decorated registration should retain its local source");
        assert_eq!(
            source.get(local_source.name_span.start_usize()..local_source.name_span.end_usize()),
            Some("implementation")
        );
    }

    #[test]
    fn decorator_registration_rejects_an_inner_transforming_decorator() {
        let source = "from django import template\nregister = template.Library()\ndef swap(function): return replacement\n@register.tag('shown')\n@swap\ndef original(parser, token): pass\n";
        let registrations = collect_registrations(source);

        assert_eq!(find_reg(&registrations, "shown").local_source(), None);
    }

    #[test]
    fn outer_transforming_decorator_does_not_change_the_registered_source() {
        let source = "from django import template\nregister = template.Library()\ndef swap(function): return replacement\n@swap\n@register.tag('shown')\ndef original(parser, token): pass\nregister.tag('later', original)\n";
        let registrations = collect_registrations(source);

        assert!(find_reg(&registrations, "shown").local_source().is_some());
        assert_eq!(find_reg(&registrations, "later").local_source(), None);
    }

    #[test]
    fn outer_registration_wins_after_an_inner_transforming_decorator() {
        let source = "from django import template\nregister = template.Library()\ndef swap(function): return replacement\n@register.tag('shown')\n@swap\n@register.tag('shown')\ndef original(parser, token): pass\n";
        let registrations = collect_registrations(source);
        let final_registration = registrations
            .iter()
            .rev()
            .find(|registration| registration.name == "shown")
            .expect("final shown registration should be collected");

        assert_eq!(final_registration.local_source(), None);
    }

    #[test]
    fn body_only_analysis_defers_plain_function_identity_to_python_facts() {
        let source = "from django import template\nregister = template.Library()\ndef implementation(parser, token):\n    pass\nregister.tag('shown', implementation)\n";
        let registrations = collect_registrations(source);

        assert_eq!(
            registered_source(source, find_reg(&registrations, "shown")),
            None,
            "the body-only Django test seam has no Python occurrence product",
        );
    }

    #[test]
    fn direct_registration_rejects_rebound_forward_and_member_callables() {
        let source = "from django import template\nregister = template.Library()\ndef imported(parser, token):\n    pass\nfrom elsewhere import imported\nregister.tag('imported', imported)\nregister.tag('forward', later)\nclass Node:\n    def handle(self, parser, token):\n        pass\nregister.tag('member', Node.handle)\ndef later(parser, token):\n    pass\n";
        let registrations = collect_registrations(source);

        for name in ["imported", "forward", "member"] {
            assert_eq!(find_reg(&registrations, name).local_source(), None);
        }
    }

    #[test]
    fn direct_registration_rejects_a_named_expression_rebinding() {
        let source = "from django import template\nregister = template.Library()\ndef implementation(parser, token):\n    pass\n(implementation := replacement)\nregister.tag('shown', implementation)\n";
        let registrations = collect_registrations(source);

        assert_eq!(find_reg(&registrations, "shown").local_source(), None);
    }

    #[test]
    fn body_only_analysis_does_not_guess_between_plain_function_definitions() {
        let source = "from django import template\nregister = template.Library()\ndef first(parser, token):\n    pass\ndef second(parser, token):\n    pass\nregister.tag('shown', first)\nregister.tag('shown', second)\n";
        let registrations = collect_registrations(source);
        let registration = registrations
            .iter()
            .rev()
            .find(|registration| registration.name == "shown")
            .expect("final registration should be collected");

        assert_eq!(
            registered_source(source, registration),
            None,
            "the body-only Django test seam has no Python occurrence product",
        );
    }

    // Corpus: `autoescape` in django/template/defaulttags.py uses `@register.tag` (bare)
    #[test]
    fn decorator_bare_tag() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "autoescape");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name(), Some("autoescape"));
    }

    // Corpus: `querystring` in django/template/defaulttags.py uses
    // `@register.simple_tag(name="querystring", takes_context=True)`
    #[test]
    fn decorator_simple_tag_with_name_kwarg() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "querystring");
        assert_eq!(reg.kind, RegistrationKind::SimpleTag);
        assert_eq!(reg.func_name(), Some("querystring"));
    }

    // Corpus: `inclusion_no_params` in tests/template_tests/templatetags/inclusion.py uses
    // `@register.inclusion_tag("inclusion.html")`
    #[test]
    fn decorator_inclusion_tag() {
        let source = fixture("tests/template_tests/templatetags/inclusion.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "inclusion_no_params");
        assert_eq!(reg.kind, RegistrationKind::InclusionTag);
    }

    // Corpus: `cut` in django/template/defaultfilters.py uses `@register.filter` (bare)
    #[test]
    fn decorator_filter_bare() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "cut");
        assert_eq!(reg.kind, RegistrationKind::Filter);
    }

    // Corpus: `escapejs` in django/template/defaultfilters.py uses
    // `@register.filter("escapejs")` — positional string name, func is `escapejs_filter`
    #[test]
    fn decorator_filter_with_positional_string_name() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "escapejs");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name(), Some("escapejs_filter"));
    }

    // Corpus: `other_echo` in tests/template_tests/templatetags/testtags.py uses
    // `register.tag("other_echo", echo)` — call-style registration
    #[test]
    fn call_style_tag_registration() {
        let source = fixture("tests/template_tests/templatetags/testtags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "other_echo");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name(), Some("echo"));
    }

    // Corpus: `intcomma` in wagtail/admin/templatetags/wagtailadmin_tags.py uses
    // `register.filter("intcomma", intcomma)` — call-style filter registration
    #[test]
    fn call_style_filter_registration() {
        let source = fixture("wagtail/admin/templatetags/wagtailadmin_tags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "intcomma");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name(), Some("intcomma"));
    }

    // Corpus: `for` in django/template/defaulttags.py uses `@register.tag("for")`
    // — positional string name overrides function name `do_for`
    #[test]
    fn tag_with_positional_string_name() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "for");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name(), Some("do_for"));
    }

    // Corpus: `addslashes` in django/template/defaultfilters.py uses
    // `@register.filter(is_safe=True)` — name defaults to function name
    #[test]
    fn filter_with_is_safe_kwarg() {
        let source = fixture("django/template/defaultfilters.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "addslashes");
        assert_eq!(reg.kind, RegistrationKind::Filter);
        assert_eq!(reg.func_name(), Some("addslashes"));
    }

    // Corpus: `partialdef` in django/template/defaulttags.py uses
    // `@register.tag(name="partialdef")` — name kwarg overrides func name `partialdef_func`
    #[test]
    fn tag_with_name_kwarg() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "partialdef");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name(), Some("partialdef_func"));
    }

    // Corpus: `dialog` in wagtail/admin/templatetags/wagtailadmin_tags.py uses
    // `register.tag("dialog", DialogNode.handle)` — call-style with method callable
    #[test]
    fn call_style_tag_with_method_callable() {
        let source = fixture("wagtail/admin/templatetags/wagtailadmin_tags.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "dialog");
        assert_eq!(reg.kind, RegistrationKind::Tag);
        assert_eq!(reg.func_name(), Some("DialogNode.handle"));
    }

    // Corpus: `div` in tests/template_tests/templatetags/custom.py uses
    // `@register.simple_block_tag` (bare decorator)
    #[test]
    fn simple_block_tag_decorator() {
        let source = fixture("tests/template_tests/templatetags/custom.py");
        let regs = collect_registrations(source);
        let reg = find_reg(&regs, "div");
        assert_eq!(reg.kind, RegistrationKind::SimpleBlockTag);
    }

    // Corpus: defaulttags.py has many registrations (tags + simple_tags)
    #[test]
    fn multiple_registrations() {
        let source = fixture("django/template/defaulttags.py");
        let regs = collect_registrations(source);
        assert!(
            regs.len() > 10,
            "expected many registrations in defaulttags.py, got {}",
            regs.len()
        );
        let tags: Vec<_> = regs
            .iter()
            .filter(|r| r.kind == RegistrationKind::Tag)
            .collect();
        assert!(
            tags.len() > 5,
            "expected multiple Tag registrations, got {}",
            tags.len()
        );
        assert!(regs.iter().any(|r| r.name == "for"));
        assert!(regs.iter().any(|r| r.name == "if"));
        assert!(regs.iter().any(|r| r.name == "autoescape"));
    }

    // Corpus: testtags.py has decorator @register.tag + call-style register.tag
    // Tests that both decorator and call-style registrations are discovered
    #[test]
    fn mixed_decorator_and_call_style() {
        let source = fixture("tests/template_tests/templatetags/testtags.py");
        let regs = collect_registrations(source);
        let tag_regs: Vec<_> = regs
            .iter()
            .filter(|r| r.kind == RegistrationKind::Tag)
            .collect();
        assert_eq!(tag_regs.len(), 2);
        assert!(tag_regs.iter().any(|r| r.name == "echo"));
        assert!(tag_regs.iter().any(|r| r.name == "other_echo"));
    }

    // Edge case: @register.tag() with empty parens — function name used as tag name.
    // Corpus: no clean isolatable example of empty parens (all corpus uses bare or with args).
    #[test]
    fn function_name_fallback() {
        let source = r"
from django import template
register = template.Library()

@register.tag()
def current_time(parser, token):
    pass
";
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "current_time");
        assert_eq!(regs[0].kind, RegistrationKind::Tag);
    }

    // Edge case: register.simple_tag(my_func, name="alias") — call-style with func positional
    // and name kwarg. Rare pattern, not found cleanly in corpus.
    #[test]
    fn simple_tag_func_positional() {
        let source = r#"
from django import template
register = template.Library()

register.simple_tag(my_func, name="alias")
"#;
        let regs = collect_registrations(source);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "alias");
        assert_eq!(regs[0].kind, RegistrationKind::SimpleTag);
        assert_eq!(regs[0].func_name(), Some("my_func"));
    }

    #[test]
    fn direct_and_curried_helpers_share_registration_options() {
        let source = r#"
from django import template
register = template.Library()

def direct(context, value): pass
register.simple_tag(direct, takes_context=True, name="direct_alias")

def curried(context, value): pass
register.simple_tag(takes_context=True, name="curried_alias")(curried)
"#;
        let regs = collect_registrations(source);
        for name in ["direct_alias", "curried_alias"] {
            let registration = find_reg(&regs, name);
            assert_eq!(registration.options.context, ContextProvision::Context);
        }
    }

    #[test]
    fn simple_block_options_retain_static_and_dynamic_end_names() {
        let source = r#"
from django import template
register = template.Library()

@register.simple_block_tag(name="panel", end_name="closepanel", takes_context=True)
def panel_impl(context, content): pass

@register.simple_block_tag(end_name=dynamic)
def uncertain(content): pass

@register.simple_block_tag(name="defaulted", end_name=None)
def defaulted_impl(content): pass
"#;
        let regs = collect_registrations(source);
        assert_eq!(
            find_reg(&regs, "panel").options,
            RegistrationOptions {
                context: ContextProvision::Context,
                block_end: Some(RegisteredEnd::Named("closepanel".to_string())),
            }
        );
        assert_eq!(
            find_reg(&regs, "uncertain").options.block_end,
            Some(RegisteredEnd::Unknown)
        );
        assert_eq!(
            find_reg(&regs, "defaulted").options.block_end,
            Some(RegisteredEnd::Default)
        );
    }

    #[test]
    fn filter_registration_flags_work_in_decorator_and_direct_forms() {
        let source = r#"
from django import template
register = template.Library()

@register.filter(is_safe=True, needs_autoescape=True)
def decorated(value, autoescape=None): pass

def direct(value): pass
register.filter("direct", direct, expects_localtime=True)
"#;
        let analysis = analyze_registrations(source);
        assert!(!analysis.inventory_is_open());
        assert_eq!(
            analysis
                .registrations
                .iter()
                .map(|registration| registration.name.as_str())
                .collect::<Vec<_>>(),
            ["decorated", "direct"]
        );
    }

    #[test]
    fn direct_filter_func_flag_is_a_known_noop() {
        let source = r"
from django import template
register = template.Library()

def unused(value): pass
register.filter(func=unused)
";
        let analysis = analyze_registrations(source);
        assert!(!analysis.inventory_is_open());
        assert!(analysis.registrations.is_empty());
    }

    #[test]
    fn filter_func_flag_is_not_a_callable_alias() {
        let source = r#"
from django import template
register = template.Library()

def ignored(value): pass

def unused(value): pass
register.filter(func=unused)

@register.filter(func=ignored)
def invalid(value): pass

@register.filter(name="decorated", func=ignored)
def decorated(value): pass
"#;
        let analysis = analyze_registrations(source);
        assert!(analysis.inventory_is_open());
        assert_eq!(analysis.registrations.len(), 1);
        let registration = &analysis.registrations[0];
        assert_eq!(registration.name, "decorated");
        assert_eq!(registration.func_name(), Some("decorated"));
    }

    #[test]
    fn tag_func_keyword_does_not_invent_a_callable_alias() {
        let source = r"
from django import template
register = template.Library()

def invented(parser, token): pass
register.tag(func=invented)
";
        let analysis = analyze_registrations(source);
        assert!(analysis.registrations.is_empty());
        assert!(analysis.inventory_is_open());
    }

    #[test]
    fn inclusion_func_argument_is_ignored_by_the_returned_decorator() {
        let source = r#"
from django import template
register = template.Library()

def ignored(value): pass
register.inclusion_tag("unused.html", func=ignored)

@register.inclusion_tag("included.html", func=ignored)
def included(value): pass
"#;
        let analysis = analyze_registrations(source);
        assert!(!analysis.inventory_is_open());
        assert_eq!(analysis.registrations.len(), 1);
        let registration = &analysis.registrations[0];
        assert_eq!(registration.name, "included");
        assert_eq!(registration.func_name(), Some("included"));
    }

    #[test]
    fn django_name_fallbacks_keep_none_empty_and_dynamic_names_distinct() {
        let source = r#"
from django import template
register = template.Library()

@register.tag(name=None)
def none_tag(parser, token): pass

@register.filter(name=None)
def none_filter(value): pass

@register.simple_tag(name=None)
def none_simple(): pass

@register.inclusion_tag("included.html", name=None)
def none_inclusion(): pass

@register.simple_block_tag(name=None)
def none_block(content): pass

@register.tag(name="")
def empty_tag(parser, token): pass

@register.filter(name="")
def empty_filter(value): pass

@register.simple_tag(name="")
def empty_simple(): pass

@register.inclusion_tag("included.html", name="")
def empty_inclusion(): pass

@register.simple_block_tag(name="")
def empty_block(content): pass

@register.simple_tag(name=dynamic_name)
def dynamic_simple(): pass
"#;
        let analysis = analyze_registrations(source);
        assert!(analysis.inventory_is_open());
        assert_eq!(
            analysis
                .registrations
                .iter()
                .map(|registration| registration.name.as_str())
                .collect::<Vec<_>>(),
            [
                "none_tag",
                "none_filter",
                "none_simple",
                "none_inclusion",
                "none_block",
                "",
                "",
                "empty_simple",
                "empty_inclusion",
                "empty_block",
            ]
        );
    }

    #[test]
    fn malformed_registration_decorators_open_inventory_without_symbols() {
        for decorator in [
            "@register.simple_tag(unsupported=True)",
            "@register.inclusion_tag",
            "@register.inclusion_tag()",
            "@register.simple_tag(other)",
        ] {
            let source = format!(
                "from django import template\nregister = template.Library()\n{decorator}\ndef invented(value): pass\n"
            );
            let analysis = analyze_registrations(&source);
            assert!(
                analysis.inventory_is_open(),
                "malformed decorator should open inventory: {decorator}"
            );
            assert!(
                analysis.registrations.is_empty(),
                "malformed decorator must not invent a registration: {decorator}"
            );
        }
    }

    #[test]
    fn empty_source() {
        let regs = collect_registrations("");
        assert!(regs.is_empty());
    }

    // Edge case: source with no registration patterns
    #[test]
    fn no_registrations() {
        let source = r"
def regular_function():
    pass

class MyClass:
    pass
";
        let regs = collect_registrations(source);
        assert!(regs.is_empty());
    }

    // Edge case: register.tag(do_something) — single func arg, no name string.
    // Valid Django API but rare. Not found cleanly in corpus.
    #[test]
    fn unresolved_callable_derived_tag_name_stays_unknown() {
        let source = r"
from django import template
register = template.Library()

register.tag(do_something)
";
        let analysis = analyze_registrations(source);
        assert!(analysis.registrations.is_empty());
        assert!(analysis.inventory_is_open());
    }

    // Edge case: register.filter(my_filter_func) — single func arg, no name string.
    // Valid Django API but rare. Not found cleanly in corpus.
    #[test]
    fn unresolved_callable_derived_filter_name_stays_unknown() {
        let source = r"
from django import template
register = template.Library()

register.filter(my_filter_func)
";
        let analysis = analyze_registrations(source);
        assert!(analysis.registrations.is_empty());
        assert!(analysis.inventory_is_open());
    }

    #[test]
    fn duplicate_positional_and_keyword_names_open_inventory() {
        let source = r#"
from django import template
register = template.Library()

@register.tag("positional_name", name="kwarg_name")
def my_tag(parser, token):
    pass
"#;
        let analysis = analyze_registrations(source);
        assert!(analysis.registrations.is_empty());
        assert!(analysis.inventory_is_open());
    }

    #[test]
    fn imported_register_preserves_known_symbols_but_opens_inventory() {
        let analysis = analyze_registrations(
            "from shared import register\n@register.simple_tag\ndef known(): pass\n@register.filter\ndef known_filter(value): return value\n",
        );
        assert!(analysis.defines_library());
        assert!(analysis.inventory_is_open());
        assert_eq!(analysis.registrations.len(), 2);
    }

    #[test]
    fn only_unshadowed_canonical_constructor_is_closed() {
        let canonical = analyze_registrations(
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known(): pass\n",
        );
        assert!(!canonical.inventory_is_open());
        let direct_import =
            analyze_registrations("from django.template import Library\nregister = Library()\n");
        assert!(!direct_import.inventory_is_open());

        for source in [
            "register = Library()\n",
            "from django import template as dt\nregister = dt.Library()\n",
            "import django.template as template\nregister = template.Library()\n",
            "from django import template\nalias = register = template.Library()\n",
            "from django import template\nimport other as template\nregister = template.Library()\n",
            "from django import template\nregister = template.Library()\nfrom shared import register\n",
        ] {
            assert!(
                analyze_registrations(source).inventory_is_open(),
                "constructor should remain open: {source}"
            );
        }
    }

    #[test]
    fn uncertain_scopes_open_inventory_without_contributing_symbols() {
        for uncertain in [
            "if enabled:\n    @register.simple_tag\n    def conditional(): pass",
            "class Helpers:\n    register = template.Library()\n    @register.simple_tag\n    def class_local(): pass",
        ] {
            let source = format!(
                "from django import template\nregister = template.Library()\n{uncertain}\n"
            );
            let analysis = analyze_registrations(&source);
            assert!(analysis.inventory_is_open());
            assert!(analysis.registrations.is_empty());
        }
    }

    #[test]
    fn nested_helper_mutation_opens_inventory() {
        let analysis = analyze_registrations(
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known(): pass\ndef configure():\n    register.tags.update(dynamic_tags)\n",
        );
        assert!(analysis.inventory_is_open());
        assert_eq!(analysis.registrations.len(), 1);
        assert_eq!(analysis.registrations[0].name, "known");
    }

    #[test]
    fn mixed_positional_keyword_calls_are_known_and_closed() {
        let analysis = analyze_registrations(
            "from django import template\nregister = template.Library()\nregister.tag('known', compile_function=tag_func)\nregister.filter('known_filter', filter_func=filter_func)\n",
        );
        assert!(!analysis.inventory_is_open());
        assert!(
            analysis
                .registrations
                .iter()
                .any(|registration| registration.name == "known")
        );
        assert!(
            analysis
                .registrations
                .iter()
                .any(|registration| registration.name == "known_filter")
        );
    }

    #[test]
    fn only_register_root_contributes_symbols() {
        let analysis = analyze_registrations(
            "from django import template\nregister = template.Library()\n@other.tag\ndef invented(parser, token): pass\nother.filter('also_invented', func)\n",
        );
        assert!(analysis.registrations.is_empty());
        assert!(!analysis.inventory_is_open());
    }

    #[test]
    fn uncertain_operations_open_inventory_without_dropping_known_symbols() {
        for operation in [
            "@register.tag(name=dynamic_name)\ndef dynamic(parser, token): pass",
            "register.tags.update(dynamic_tags)",
            "register.tags['dynamic'] = func",
            "del register.filters['dynamic']",
            "register.tag('invented', name=dynamic_name)",
            "register.filter('invented', name=dynamic_name)",
            "register.tag('invented', name='duplicate', compile_function=func)",
            "register.filter('invented', name='duplicate', filter_func=func)",
            "register.inclusion_tag('partial.html', func, extra, name='invented')",
            "configure(register)",
        ] {
            let source = format!(
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef known(): pass\n{operation}\n"
            );
            let analysis = analyze_registrations(&source);
            assert!(
                analysis.inventory_is_open(),
                "operation should open inventory: {operation}"
            );
            assert_eq!(
                analysis
                    .registrations
                    .iter()
                    .map(|registration| registration.name.as_str())
                    .collect::<Vec<_>>(),
                ["known"],
                "uncertain operation must not invent symbols: {operation}",
            );
        }
    }
}
