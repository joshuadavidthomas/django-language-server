//! Django's template tag pattern, `django.template.base.tag_re`, as a Project's
//! startup code leaves it.
//!
//! Django's Lexer splits templates with `{%.*?%}|{{.*?}}|{#.*?#}`, whose `.` stops
//! at a newline. Projects that want tags to span lines replace it while Django
//! starts, usually from an app package or its `AppConfig.ready()`:
//!
//! ```python
//! from django.template import base
//! base.tag_re = re.compile(base.tag_re.pattern, re.DOTALL)
//! ```
//!
//! Extraction reads the modules Django imports at startup (the settings module,
//! each installed app package with its `apps` and `models` modules, and the
//! modules they import from their own top-level package) and classifies every
//! assignment to `tag_re` on `django.template.base`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;

use ruff_python_ast::Expr;
use ruff_python_ast::Number;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::StmtImport;
use ruff_python_ast::StmtImportFrom;

use crate::ast::ExprExt;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::python::PythonSourceModule;
use crate::python::RecoveredPythonModule;
use crate::python::ScopeBindings;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;
use crate::python::module::relative_import_source;
use crate::settings::django_settings;
use crate::settings::settings_module;
use crate::settings::types::InstalledAppEvidence;
use crate::settings::types::SettingCase;
use crate::templates::installed_app_package_module;

const DJANGO_TEMPLATE_BASE: &str = "django.template.base";
const TAG_RE: &str = "tag_re";
// `re.DOTALL`'s value, for flags written as integers.
const RE_DOTALL_BIT: i64 = 16;

/// Django's `tag_re` after a Project's startup code runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TemplateTagRegex {
    /// No startup module assigns a `re.DOTALL` pattern: a tag closes on the line it opens.
    Default,
    /// A startup module assigns a pattern compiled with `re.DOTALL`: tags may span lines.
    DotAll,
    /// A startup module could not be found or read, or assigns `tag_re` a value
    /// extraction cannot classify.
    Unknown,
}

impl TemplateTagRegex {
    // Any DOTALL assignment can run, so it decides the fact. Otherwise an
    // unclassified source leaves the fact open.
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::DotAll, _) | (_, Self::DotAll) => Self::DotAll,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Default, Self::Default) => Self::Default,
        }
    }
}

#[salsa::tracked(returns(copy))]
pub fn template_tag_regex(db: &dyn ProjectDb, project: Project) -> TemplateTagRegex {
    let Some(settings) = settings_module(db, project) else {
        return TemplateTagRegex::Unknown;
    };
    let mut result = TemplateTagRegex::Default;
    let mut queue = VecDeque::from([settings.name().clone()]);

    for case in django_settings(db, project).feasible_cases() {
        let entries: Vec<&str> = match case.installed_apps {
            SettingCase::Known(apps) => apps.apps.iter().map(|app| app.value.as_str()).collect(),
            SettingCase::Dynamic(partial) | SettingCase::Malformed(partial) => partial
                .evidence
                .iter()
                .filter_map(|evidence| match evidence {
                    InstalledAppEvidence::Known(app) => Some(app.value.as_str()),
                    InstalledAppEvidence::Issue(_) => {
                        result = result.join(TemplateTagRegex::Unknown);
                        None
                    }
                })
                .collect(),
            SettingCase::Unset => Vec::new(),
        };
        for entry in entries {
            let Some(package) = installed_app_package_module(db, project, entry) else {
                result = result.join(TemplateTagRegex::Unknown);
                continue;
            };
            queue.extend(package.prefixes());
            queue.extend(
                ["apps", "models"]
                    .map(|child| package.exact_child(child))
                    .into_iter()
                    .flatten(),
            );
        }
    }

    let mut seen = BTreeSet::new();
    while let Some(name) = queue.pop_front() {
        if top_level(&name) == "django" || !seen.insert(name.clone()) {
            continue;
        }
        let Some(module) = PythonSourceModule::resolve(db, project, name) else {
            continue;
        };
        let scan = startup_module_scan(db, project, module);
        if let Some(tag_regex) = scan.tag_regex {
            result = result.join(tag_regex);
        }
        queue.extend(scan.imports.iter().cloned());
    }
    result
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct StartupModuleScan {
    /// The joined classification of this module's `tag_re` assignments, if any.
    tag_regex: Option<TemplateTagRegex>,
    /// Modules this module imports from its own top-level package.
    imports: BTreeSet<PythonModuleName>,
}

// Salsa tracked-query keys are by-value; `module` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked(returns(ref))]
fn startup_module_scan(
    db: &dyn ProjectDb,
    _project: Project,
    module: PythonSourceModule,
) -> StartupModuleScan {
    let Ok(Some(parsed)) = RecoveredPythonModule::from_file(db, module.file()) else {
        return StartupModuleScan {
            tag_regex: Some(TemplateTagRegex::Unknown),
            imports: BTreeSet::new(),
        };
    };
    let mut scanner = Scanner {
        module: &module,
        scan: StartupModuleScan::default(),
        functions: Vec::new(),
    };
    let mut env = Env::new();
    scanner.body(parsed.body(db), &mut env);

    // Function bodies run later, against the module's final globals.
    while let Some(function) = scanner.functions.pop() {
        let mut function_env = env.clone();
        let scope = ScopeBindings::collect(&function.body);
        for name in scope
            .writes
            .keys()
            .filter(|name| !scope.globals.contains(*name))
            .chain(&scope.nonlocals)
        {
            function_env.remove(name);
        }
        let parameters = &function.parameters;
        for parameter in parameters
            .posonlyargs
            .iter()
            .chain(&parameters.args)
            .chain(&parameters.kwonlyargs)
        {
            function_env.remove(parameter.parameter.name.as_str());
        }
        for parameter in parameters.vararg.iter().chain(&parameters.kwarg) {
            function_env.remove(parameter.name.as_str());
        }
        scanner.body(&function.body, &mut function_env);
    }
    scanner.scan
}

fn top_level(name: &PythonModuleName) -> &str {
    name.as_str().split('.').next().unwrap_or_default()
}

/// What a name statically denotes, for the few values a `tag_re` patch uses.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Module(String),
    ReModule,
    ReCompile,
    ReRegexFlag,
    /// Regex flags; whether `re.DOTALL` is among them.
    Flags(bool),
    /// A compiled pattern.
    Pattern(TemplateTagRegex),
}

type Env = BTreeMap<String, Value>;

struct Scanner<'a> {
    module: &'a PythonSourceModule,
    scan: StartupModuleScan,
    functions: Vec<&'a StmtFunctionDef>,
}

impl<'a> Scanner<'a> {
    fn body(&mut self, body: &'a [Stmt], env: &mut Env) {
        for statement in body {
            self.statement(statement, env);
        }
    }

    fn branches(&mut self, env: &mut Env, bodies: &[&'a [Stmt]], may_skip: bool) {
        let mut joined: Option<Env> = may_skip.then(|| env.clone());
        for body in bodies {
            let mut branch = env.clone();
            self.body(body, &mut branch);
            joined = Some(match joined {
                None => branch,
                Some(mut joined) => {
                    joined.retain(|name, value| branch.get(name) == Some(value));
                    joined
                }
            });
        }
        if let Some(joined) = joined {
            *env = joined;
        }
    }

    fn statement(&mut self, statement: &'a Stmt, env: &mut Env) {
        match statement {
            Stmt::Import(import) => self.import(import, env),
            Stmt::ImportFrom(import) => self.import_from(import, env),
            Stmt::Assign(assign) => {
                let value = eval(&assign.value, env);
                for target in &assign.targets {
                    self.assign(target, &assign.value, value.clone(), env);
                }
            }
            Stmt::AnnAssign(assign) => {
                if let Some(value) = &assign.value {
                    self.assign(&assign.target, value, eval(value, env), env);
                }
            }
            Stmt::AugAssign(assign) => {
                self.record_write(&assign.target, None, env);
                unbind(&assign.target, env);
            }
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    unbind(target, env);
                }
            }
            Stmt::Expr(expression) => self.setattr(&expression.value, env),
            Stmt::FunctionDef(function) => {
                env.remove(function.name.as_str());
                self.functions.push(function);
            }
            Stmt::ClassDef(class) => {
                // Class bodies run now in their own namespace; methods run later.
                let mut class_env = env.clone();
                self.body(&class.body, &mut class_env);
                env.remove(class.name.as_str());
            }
            Stmt::If(stmt_if) => {
                let mut bodies = vec![stmt_if.body.as_slice()];
                bodies.extend(
                    stmt_if
                        .elif_else_clauses
                        .iter()
                        .map(|clause| clause.body.as_slice()),
                );
                let has_else = stmt_if
                    .elif_else_clauses
                    .last()
                    .is_some_and(|clause| clause.test.is_none());
                self.branches(env, &bodies, !has_else);
            }
            Stmt::For(stmt_for) => {
                unbind(&stmt_for.target, env);
                self.branches(env, &[&stmt_for.body], true);
                self.body(&stmt_for.orelse, env);
            }
            Stmt::While(stmt_while) => {
                self.branches(env, &[&stmt_while.body], true);
                self.body(&stmt_while.orelse, env);
            }
            Stmt::With(stmt_with) => {
                for item in &stmt_with.items {
                    if let Some(target) = &item.optional_vars {
                        unbind(target, env);
                    }
                }
                self.body(&stmt_with.body, env);
            }
            Stmt::Try(stmt_try) => {
                let mut bodies = vec![stmt_try.body.as_slice()];
                for handler in &stmt_try.handlers {
                    let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = handler;
                    if let Some(name) = &handler.name {
                        env.remove(name.as_str());
                    }
                    bodies.push(&handler.body);
                }
                self.branches(env, &bodies, false);
                self.body(&stmt_try.orelse, env);
                self.body(&stmt_try.finalbody, env);
            }
            Stmt::Match(stmt_match) => {
                let bodies: Vec<&[Stmt]> = stmt_match
                    .cases
                    .iter()
                    .map(|case| case.body.as_slice())
                    .collect();
                self.branches(env, &bodies, true);
            }
            Stmt::TypeAlias(alias) => unbind(&alias.name, env),
            Stmt::Return(_)
            | Stmt::Raise(_)
            | Stmt::Assert(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {}
        }
    }

    fn import(&mut self, import: &StmtImport, env: &mut Env) {
        for clause in DirectImportClause::lower(import) {
            self.follow(clause.requested());
            let value = if clause.requested() == "re" {
                Value::ReModule
            } else {
                Value::Module(clause.target().to_string())
            };
            env.insert(clause.bound().to_string(), value);
        }
    }

    fn import_from(&mut self, import: &StmtImportFrom, env: &mut Env) {
        let syntax = FromImportSyntax::lower(import);
        let source = relative_import_source(self.module.package(), syntax.level(), syntax.module());
        if syntax.has_star() {
            env.clear();
        }
        let Some(source) = source else {
            for member in syntax.named_members() {
                env.remove(member.bound());
            }
            return;
        };
        self.follow(&source);
        for member in syntax.named_members() {
            let value = if source == "re" {
                re_member(member.imported())
            } else {
                let child = format!("{source}.{}", member.imported());
                self.follow(&child);
                Some(Value::Module(child))
            };
            match value {
                Some(value) => env.insert(member.bound().to_string(), value),
                None => env.remove(member.bound()),
            };
        }
    }

    fn follow(&mut self, requested: &str) {
        let Ok(name) = PythonModuleName::parse(requested) else {
            return;
        };
        if top_level(&name) == top_level(self.module.name()) {
            self.scan.imports.extend(name.prefixes());
        }
    }

    fn assign(&mut self, target: &Expr, value: &Expr, evaluated: Option<Value>, env: &mut Env) {
        self.record_write(target, Some(value), env);
        match target.name_target() {
            Some(name) => match evaluated {
                Some(evaluated) => {
                    env.insert(name.to_string(), evaluated);
                }
                None => {
                    env.remove(name);
                }
            },
            None => unbind(target, env),
        }
    }

    // `setattr(base, "tag_re", pattern)`.
    fn setattr(&mut self, expression: &Expr, env: &Env) {
        let Expr::Call(call) = expression else {
            return;
        };
        if call.func.name_target() != Some("setattr") || env.contains_key("setattr") {
            return;
        }
        if let [owner, attribute, value] = &*call.arguments.args
            && attribute.string_literal() == Some(TAG_RE)
            && is_template_base(owner, env)
        {
            self.record(classify_pattern(value, env));
        }
    }

    fn record_write(&mut self, target: &Expr, value: Option<&Expr>, env: &Env) {
        let Expr::Attribute(attribute) = target else {
            return;
        };
        if attribute.attr.as_str() == TAG_RE && is_template_base(&attribute.value, env) {
            self.record(value.map_or(TemplateTagRegex::Unknown, |value| {
                classify_pattern(value, env)
            }));
        }
    }

    fn record(&mut self, tag_regex: TemplateTagRegex) {
        self.scan.tag_regex = Some(
            self.scan
                .tag_regex
                .map_or(tag_regex, |recorded| recorded.join(tag_regex)),
        );
    }
}

// Attribute and subscript targets bind no local name.
fn unbind(target: &Expr, env: &mut Env) {
    if let Expr::Name(name) = target {
        env.remove(name.id.as_str());
    } else if let Expr::Tuple(tuple) = target {
        tuple.elts.iter().for_each(|element| unbind(element, env));
    } else if let Expr::List(list) = target {
        list.elts.iter().for_each(|element| unbind(element, env));
    } else if let Expr::Starred(starred) = target {
        unbind(&starred.value, env);
    }
}

fn is_template_base(expression: &Expr, env: &Env) -> bool {
    matches!(eval(expression, env), Some(Value::Module(name)) if name == DJANGO_TEMPLATE_BASE)
}

fn re_member(name: &str) -> Option<Value> {
    match name {
        "compile" => Some(Value::ReCompile),
        "RegexFlag" => Some(Value::ReRegexFlag),
        _ => re_flag(name),
    }
}

fn re_flag(name: &str) -> Option<Value> {
    match name {
        "DOTALL" | "S" => Some(Value::Flags(true)),
        "NOFLAG" | "ASCII" | "A" | "DEBUG" | "IGNORECASE" | "I" | "LOCALE" | "L" | "MULTILINE"
        | "M" | "UNICODE" | "U" | "VERBOSE" | "X" => Some(Value::Flags(false)),
        _ => None,
    }
}

fn eval(expression: &Expr, env: &Env) -> Option<Value> {
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "every other expression is unclassified"
    )]
    match expression {
        Expr::Name(name) => env.get(name.id.as_str()).cloned(),
        Expr::Attribute(attribute) => match eval(&attribute.value, env)? {
            Value::Module(module) => Some(Value::Module(format!("{module}.{}", attribute.attr))),
            Value::ReModule => re_member(attribute.attr.as_str()),
            Value::ReRegexFlag => re_flag(attribute.attr.as_str()),
            Value::ReCompile | Value::Flags(_) | Value::Pattern(_) => None,
        },
        Expr::BinOp(binary) if binary.op == Operator::BitOr => {
            match (eval(&binary.left, env)?, eval(&binary.right, env)?) {
                (Value::Flags(left), Value::Flags(right)) => Some(Value::Flags(left || right)),
                _ => None,
            }
        }
        Expr::NumberLiteral(number) => match &number.value {
            Number::Int(value) => Some(Value::Flags(value.as_i64()? & RE_DOTALL_BIT != 0)),
            Number::Float(_) | Number::Complex { .. } => None,
        },
        Expr::Call(call) if matches!(eval(&call.func, env), Some(Value::ReCompile)) => {
            Some(Value::Pattern(classify_compile(call, env)))
        }
        _ => None,
    }
}

fn classify_pattern(expression: &Expr, env: &Env) -> TemplateTagRegex {
    match eval(expression, env) {
        Some(Value::Pattern(tag_regex)) => tag_regex,
        _ => TemplateTagRegex::Unknown,
    }
}

// `re.compile(pattern, flags=0)`.
fn classify_compile(call: &ruff_python_ast::ExprCall, env: &Env) -> TemplateTagRegex {
    let arguments = &call.arguments;
    let keyword = |name: &str| {
        arguments
            .keywords
            .iter()
            .find(|keyword| keyword.arg.as_ref().is_some_and(|arg| arg.as_str() == name))
            .map(|keyword| &keyword.value)
    };
    if arguments.args.len() > 2
        || arguments.args.iter().any(Expr::is_starred_expr)
        || arguments
            .keywords
            .iter()
            .any(|keyword| keyword.arg.is_none())
    {
        return TemplateTagRegex::Unknown;
    }
    let pattern = arguments.args.first().or_else(|| keyword("pattern"));
    let flags = arguments.args.get(1).or_else(|| keyword("flags"));

    let flags_dotall = match flags.map(|flags| eval(flags, env)) {
        None => false,
        Some(Some(Value::Flags(dotall))) => dotall,
        Some(_) => return TemplateTagRegex::Unknown,
    };
    if flags_dotall {
        return TemplateTagRegex::DotAll;
    }
    let Some(pattern) = pattern else {
        return TemplateTagRegex::Unknown;
    };
    if let Some(literal) = pattern.string_literal() {
        return if has_inline_dotall(literal) {
            TemplateTagRegex::DotAll
        } else {
            TemplateTagRegex::Default
        };
    }
    // `base.tag_re.pattern` is Django's own pattern, which has no inline flags.
    if let Expr::Attribute(attribute) = pattern
        && attribute.attr.as_str() == "pattern"
        && let Expr::Attribute(owner) = &*attribute.value
        && owner.attr.as_str() == TAG_RE
        && is_template_base(&owner.value, env)
    {
        return TemplateTagRegex::Default;
    }
    TemplateTagRegex::Unknown
}

// A leading global flag group such as `(?s)` or `(?si)`.
fn has_inline_dotall(pattern: &str) -> bool {
    pattern
        .strip_prefix("(?")
        .and_then(|rest| rest.split_once(')'))
        .is_some_and(|(flags, _)| {
            flags.contains('s') && flags.chars().all(|flag| "aiLmsux".contains(flag))
        })
}
