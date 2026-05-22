use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileKind;
use ruff_python_ast::Comprehension;
use ruff_python_ast::ExceptHandler;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;

use crate::project_layout_index;
use crate::Db;
use crate::Project;
use crate::ProjectLayoutIndexOutcome;
use crate::ProjectSourceFilesIssue;
use crate::ProjectSourceInventory;
use crate::PyModuleName;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonSourceModel {
    file: File,
    module: PyModuleNameResolution,
    status: PythonSourceModelStatus,
    imports: Vec<ImportStatement>,
    assignments: Vec<Assignment>,
    calls: Vec<CallExpression>,
    class_defs: Vec<ClassDef>,
    function_defs: Vec<FunctionDef>,
}

impl PythonSourceModel {
    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }

    #[must_use]
    pub fn module(&self) -> &PyModuleNameResolution {
        &self.module
    }

    #[must_use]
    pub fn status(&self) -> &PythonSourceModelStatus {
        &self.status
    }

    #[must_use]
    pub fn imports(&self) -> &[ImportStatement] {
        &self.imports
    }

    #[must_use]
    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }

    #[must_use]
    pub fn calls(&self) -> &[CallExpression] {
        &self.calls
    }

    #[must_use]
    pub fn class_defs(&self) -> &[ClassDef] {
        &self.class_defs
    }

    #[must_use]
    pub fn function_defs(&self) -> &[FunctionDef] {
        &self.function_defs
    }

    fn with_module(mut self, module: PyModuleNameResolution) -> Self {
        self.module = module;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonSourceModelStatus {
    Parsed,
    ParseError { issue: PythonSourceModelIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonSourceModelIssue {
    ParseError,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PythonSourceIndex {
    models: Vec<PythonSourceModel>,
}

impl PythonSourceIndex {
    fn new(models: Vec<PythonSourceModel>) -> Self {
        Self { models }
    }

    #[must_use]
    pub fn models(&self) -> &[PythonSourceModel] {
        &self.models
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonSourceIndexOutcome {
    Ready(PythonSourceIndex),
    Skipped { issue: PythonSourceIndexIssue },
    Unavailable { issue: PythonSourceIndexIssue },
    Deferred { issue: PythonSourceIndexIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonSourceIndexIssue {
    NoPythonFiles,
    SourceInventoryUnavailable { issue: ProjectSourceFilesIssue },
    LayoutUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PyModuleNameResolution {
    Resolved(PyModuleName),
    Unknown { issue: ModuleNameIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleNameIssue {
    NonPythonFile { path: Utf8PathBuf },
    OutsideImportRoots { path: Utf8PathBuf },
    InvalidModuleName { path: Utf8PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualifiedName(Vec<String>);

impl QualifiedName {
    fn from_expr(expr: &Expr) -> Option<Self> {
        match expr {
            Expr::Name(name) => Some(Self(vec![name.id.to_string()])),
            Expr::Attribute(attribute) => {
                let mut base = Self::from_expr(&attribute.value)?.0;
                base.push(attribute.attr.to_string());
                Some(Self(base))
            }
            _ => None,
        }
    }

    fn parse(name: &str) -> Self {
        Self(name.split('.').map(str::to_string).collect())
    }

    #[must_use]
    pub fn parts(&self) -> &[String] {
        &self.0
    }

    #[must_use]
    pub fn as_dotted(&self) -> String {
        self.0.join(".")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportStatement {
    Import {
        module: QualifiedName,
        alias: Option<String>,
    },
    ImportFrom {
        module: Option<QualifiedName>,
        name: String,
        alias: Option<String>,
        level: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentTarget {
    name: QualifiedName,
}

impl AssignmentTarget {
    #[must_use]
    pub fn name(&self) -> &QualifiedName {
        &self.name
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assignment {
    targets: Vec<AssignmentTarget>,
    value: StaticValue,
}

impl Assignment {
    #[must_use]
    pub fn targets(&self) -> &[AssignmentTarget] {
        &self.targets
    }

    #[must_use]
    pub fn value(&self) -> &StaticValue {
        &self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallExpression {
    callee: Option<QualifiedName>,
    arguments: Vec<StaticValue>,
    keywords: Vec<(String, StaticValue)>,
}

impl CallExpression {
    #[must_use]
    pub fn callee(&self) -> Option<&QualifiedName> {
        self.callee.as_ref()
    }

    #[must_use]
    pub fn arguments(&self) -> &[StaticValue] {
        &self.arguments
    }

    #[must_use]
    pub fn keywords(&self) -> &[(String, StaticValue)] {
        &self.keywords
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassDef {
    name: String,
    bases: Vec<QualifiedName>,
}

impl ClassDef {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn bases(&self) -> &[QualifiedName] {
        &self.bases
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDef {
    name: String,
    is_async: bool,
}

impl FunctionDef {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn is_async(&self) -> bool {
        self.is_async
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticValueSegment<T> {
    value: Option<T>,
    issue: Option<StaticValueIssue>,
}

impl<T> StaticValueSegment<T> {
    fn known(value: T) -> Self {
        Self {
            value: Some(value),
            issue: None,
        }
    }

    fn unknown(issue: StaticValueIssue) -> Self {
        Self {
            value: None,
            issue: Some(issue),
        }
    }

    #[must_use]
    pub fn value(&self) -> Option<&T> {
        self.value.as_ref()
    }

    #[must_use]
    pub fn issue(&self) -> Option<&StaticValueIssue> {
        self.issue.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StaticValue {
    String(String),
    StringList(Vec<StaticValueSegment<String>>),
    Dict(Vec<(String, StaticValue)>),
    Unknown { issue: StaticValueIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StaticValueIssue {
    UnsupportedExpression { kind: &'static str },
    UnsupportedDictKey,
    SpreadElement,
}

#[salsa::tracked(returns(ref))]
pub fn python_source_model(db: &dyn Db, file: File) -> PythonSourceModel {
    let source = file.source(db);
    let module_resolution = PyModuleNameResolution::Unknown {
        issue: ModuleNameIssue::OutsideImportRoots {
            path: file.path(db).clone(),
        },
    };
    if *source.kind() != FileKind::Python {
        return PythonSourceModel {
            file,
            module: PyModuleNameResolution::Unknown {
                issue: ModuleNameIssue::NonPythonFile {
                    path: file.path(db).clone(),
                },
            },
            status: PythonSourceModelStatus::Parsed,
            imports: Vec::new(),
            assignments: Vec::new(),
            calls: Vec::new(),
            class_defs: Vec::new(),
            function_defs: Vec::new(),
        };
    }

    let parsed = match ruff_python_parser::parse_module(source.as_ref()) {
        Ok(parsed) => parsed.into_syntax(),
        Err(_) => {
            return PythonSourceModel {
                file,
                module: module_resolution,
                status: PythonSourceModelStatus::ParseError {
                    issue: PythonSourceModelIssue::ParseError,
                },
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::new(),
                class_defs: Vec::new(),
                function_defs: Vec::new(),
            };
        }
    };

    let mut collector = PythonSourceCollector::new(file, module_resolution);
    collector.collect_body(&parsed.body);
    collector.finish()
}

#[salsa::tracked(returns(ref))]
pub fn python_source_index(db: &dyn Db, project: Project) -> PythonSourceIndexOutcome {
    let files = match project.source_inventory(db) {
        ProjectSourceInventory::Ready(files) => files,
        ProjectSourceInventory::Unavailable {
            issue: ProjectSourceFilesIssue::NotLoaded,
        } => {
            return PythonSourceIndexOutcome::Deferred {
                issue: PythonSourceIndexIssue::SourceInventoryUnavailable {
                    issue: ProjectSourceFilesIssue::NotLoaded,
                },
            };
        }
        ProjectSourceInventory::Unavailable { issue } => {
            return PythonSourceIndexOutcome::Unavailable {
                issue: PythonSourceIndexIssue::SourceInventoryUnavailable { issue },
            };
        }
    };

    let ProjectLayoutIndexOutcome::Ready(layout) = project_layout_index(db, project) else {
        return PythonSourceIndexOutcome::Unavailable {
            issue: PythonSourceIndexIssue::LayoutUnavailable,
        };
    };

    let models = files
        .merged()
        .data(db)
        .files()
        .iter()
        .filter(|file| file.kind() == FileKind::Python)
        .filter_map(|file| {
            let module = layout
                .module_name_for_path(file.path())
                .map(PyModuleNameResolution::Resolved)
                .unwrap_or_else(|| PyModuleNameResolution::Unknown {
                    issue: ModuleNameIssue::OutsideImportRoots {
                        path: file.path().to_owned(),
                    },
                });
            layout
                .file_for_path(file.path())
                .map(|file| python_source_model(db, file).clone().with_module(module))
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        PythonSourceIndexOutcome::Skipped {
            issue: PythonSourceIndexIssue::NoPythonFiles,
        }
    } else {
        PythonSourceIndexOutcome::Ready(PythonSourceIndex::new(models))
    }
}

struct PythonSourceCollector {
    file: File,
    module: PyModuleNameResolution,
    imports: Vec<ImportStatement>,
    assignments: Vec<Assignment>,
    calls: Vec<CallExpression>,
    class_defs: Vec<ClassDef>,
    function_defs: Vec<FunctionDef>,
}

impl PythonSourceCollector {
    fn new(file: File, module: PyModuleNameResolution) -> Self {
        Self {
            file,
            module,
            imports: Vec::new(),
            assignments: Vec::new(),
            calls: Vec::new(),
            class_defs: Vec::new(),
            function_defs: Vec::new(),
        }
    }

    fn collect_body(&mut self, body: &[Stmt]) {
        for stmt in body {
            self.collect_stmt(stmt);
        }
    }

    fn collect_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Import(stmt) => {
                self.imports
                    .extend(stmt.names.iter().map(|alias| ImportStatement::Import {
                        module: QualifiedName::parse(alias.name.as_str()),
                        alias: alias.asname.as_ref().map(ToString::to_string),
                    }));
            }
            Stmt::ImportFrom(stmt) => {
                self.imports.extend(stmt.names.iter().map(|alias| {
                    ImportStatement::ImportFrom {
                        module: stmt
                            .module
                            .as_ref()
                            .map(|module| QualifiedName::parse(module.as_str())),
                        name: alias.name.to_string(),
                        alias: alias.asname.as_ref().map(ToString::to_string),
                        level: stmt.level,
                    }
                }));
            }
            Stmt::Assign(stmt) => {
                for target in &stmt.targets {
                    self.collect_expr(target);
                }
                self.collect_expr(&stmt.value);
                self.assignments.push(Assignment {
                    targets: stmt
                        .targets
                        .iter()
                        .filter_map(QualifiedName::from_expr)
                        .map(|name| AssignmentTarget { name })
                        .collect(),
                    value: static_value(&stmt.value),
                });
            }
            Stmt::AugAssign(stmt) => {
                self.collect_expr(&stmt.target);
                self.collect_expr(&stmt.value);
            }
            Stmt::AnnAssign(stmt) => {
                self.collect_expr(&stmt.target);
                self.collect_expr(&stmt.annotation);
                if let Some(value) = &stmt.value {
                    self.collect_expr(value);
                }
            }
            Stmt::ClassDef(stmt) => {
                self.class_defs.push(ClassDef {
                    name: stmt.name.to_string(),
                    bases: stmt
                        .arguments
                        .as_ref()
                        .map(|arguments| {
                            arguments
                                .args
                                .iter()
                                .filter_map(QualifiedName::from_expr)
                                .collect()
                        })
                        .unwrap_or_default(),
                });
                for decorator in &stmt.decorator_list {
                    self.collect_expr(&decorator.expression);
                }
                self.collect_body(&stmt.body);
            }
            Stmt::FunctionDef(stmt) => {
                self.function_defs.push(FunctionDef {
                    name: stmt.name.to_string(),
                    is_async: stmt.is_async,
                });
                for decorator in &stmt.decorator_list {
                    self.collect_expr(&decorator.expression);
                }
                for parameter in stmt.parameters.iter_non_variadic_params() {
                    if let Some(annotation) = parameter.parameter.annotation() {
                        self.collect_expr(annotation);
                    }
                    if let Some(default) = &parameter.default {
                        self.collect_expr(default);
                    }
                }
                if let Some(returns) = &stmt.returns {
                    self.collect_expr(returns);
                }
                self.collect_body(&stmt.body);
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.collect_expr(value);
                }
            }
            Stmt::Delete(stmt) => {
                for target in &stmt.targets {
                    self.collect_expr(target);
                }
            }
            Stmt::TypeAlias(stmt) => {
                self.collect_expr(&stmt.name);
                self.collect_expr(&stmt.value);
            }
            Stmt::Expr(stmt) => self.collect_expr(&stmt.value),
            Stmt::If(stmt) => {
                self.collect_expr(&stmt.test);
                self.collect_body(&stmt.body);
                for clause in &stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.collect_expr(test);
                    }
                    self.collect_body(&clause.body);
                }
            }
            Stmt::For(stmt) => {
                self.collect_expr(&stmt.target);
                self.collect_expr(&stmt.iter);
                self.collect_body(&stmt.body);
                self.collect_body(&stmt.orelse);
            }
            Stmt::While(stmt) => {
                self.collect_expr(&stmt.test);
                self.collect_body(&stmt.body);
                self.collect_body(&stmt.orelse);
            }
            Stmt::With(stmt) => {
                for item in &stmt.items {
                    self.collect_expr(&item.context_expr);
                    if let Some(optional_vars) = &item.optional_vars {
                        self.collect_expr(optional_vars);
                    }
                }
                self.collect_body(&stmt.body);
            }
            Stmt::Match(stmt) => {
                self.collect_expr(&stmt.subject);
                for case in &stmt.cases {
                    if let Some(guard) = &case.guard {
                        self.collect_expr(guard);
                    }
                    self.collect_body(&case.body);
                }
            }
            Stmt::Raise(stmt) => {
                if let Some(exc) = &stmt.exc {
                    self.collect_expr(exc);
                }
                if let Some(cause) = &stmt.cause {
                    self.collect_expr(cause);
                }
            }
            Stmt::Try(stmt) => {
                self.collect_body(&stmt.body);
                for handler in &stmt.handlers {
                    let ExceptHandler::ExceptHandler(handler) = handler;
                    if let Some(type_) = &handler.type_ {
                        self.collect_expr(type_);
                    }
                    self.collect_body(&handler.body);
                }
                self.collect_body(&stmt.orelse);
                self.collect_body(&stmt.finalbody);
            }
            Stmt::Assert(stmt) => {
                self.collect_expr(&stmt.test);
                if let Some(msg) = &stmt.msg {
                    self.collect_expr(msg);
                }
            }
            Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {}
        }
    }

    fn collect_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call(call) => {
                self.calls.push(CallExpression {
                    callee: QualifiedName::from_expr(&call.func),
                    arguments: call.arguments.args.iter().map(static_value).collect(),
                    keywords: call
                        .arguments
                        .keywords
                        .iter()
                        .filter_map(|keyword| {
                            keyword
                                .arg
                                .as_ref()
                                .map(|arg| (arg.to_string(), static_value(&keyword.value)))
                        })
                        .collect(),
                });
                self.collect_expr(&call.func);
                for arg in &call.arguments.args {
                    self.collect_expr(arg);
                }
                for keyword in &call.arguments.keywords {
                    self.collect_expr(&keyword.value);
                }
            }
            Expr::BoolOp(expr) => {
                for value in &expr.values {
                    self.collect_expr(value);
                }
            }
            Expr::Named(expr) => {
                self.collect_expr(&expr.target);
                self.collect_expr(&expr.value);
            }
            Expr::BinOp(expr) => {
                self.collect_expr(&expr.left);
                self.collect_expr(&expr.right);
            }
            Expr::UnaryOp(expr) => self.collect_expr(&expr.operand),
            Expr::Lambda(expr) => self.collect_expr(&expr.body),
            Expr::If(expr) => {
                self.collect_expr(&expr.test);
                self.collect_expr(&expr.body);
                self.collect_expr(&expr.orelse);
            }
            Expr::Dict(dict) => {
                for item in dict.iter() {
                    if let Some(key) = &item.key {
                        self.collect_expr(key);
                    }
                    self.collect_expr(&item.value);
                }
            }
            Expr::Set(set) => {
                for element in &set.elts {
                    self.collect_expr(element);
                }
            }
            Expr::ListComp(expr) => {
                self.collect_expr(&expr.elt);
                self.collect_comprehensions(&expr.generators);
            }
            Expr::SetComp(expr) => {
                self.collect_expr(&expr.elt);
                self.collect_comprehensions(&expr.generators);
            }
            Expr::DictComp(expr) => {
                self.collect_expr(&expr.key);
                self.collect_expr(&expr.value);
                self.collect_comprehensions(&expr.generators);
            }
            Expr::Generator(expr) => {
                self.collect_expr(&expr.elt);
                self.collect_comprehensions(&expr.generators);
            }
            Expr::Await(expr) => self.collect_expr(&expr.value),
            Expr::Yield(expr) => {
                if let Some(value) = &expr.value {
                    self.collect_expr(value);
                }
            }
            Expr::YieldFrom(expr) => self.collect_expr(&expr.value),
            Expr::Compare(expr) => {
                self.collect_expr(&expr.left);
                for comparator in expr.comparators.iter() {
                    self.collect_expr(comparator);
                }
            }
            Expr::Attribute(attribute) => self.collect_expr(&attribute.value),
            Expr::Subscript(expr) => {
                self.collect_expr(&expr.value);
                self.collect_expr(&expr.slice);
            }
            Expr::Starred(expr) => self.collect_expr(&expr.value),
            Expr::List(list) => {
                for element in &list.elts {
                    self.collect_expr(element);
                }
            }
            Expr::Tuple(tuple) => {
                for element in &tuple.elts {
                    self.collect_expr(element);
                }
            }
            Expr::Slice(slice) => {
                if let Some(lower) = &slice.lower {
                    self.collect_expr(lower);
                }
                if let Some(upper) = &slice.upper {
                    self.collect_expr(upper);
                }
                if let Some(step) = &slice.step {
                    self.collect_expr(step);
                }
            }
            Expr::FString(_)
            | Expr::TString(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::Name(_)
            | Expr::IpyEscapeCommand(_) => {}
        }
    }

    fn collect_comprehensions(&mut self, comprehensions: &[Comprehension]) {
        for comprehension in comprehensions {
            self.collect_expr(&comprehension.target);
            self.collect_expr(&comprehension.iter);
            for if_clause in &comprehension.ifs {
                self.collect_expr(if_clause);
            }
        }
    }

    fn finish(self) -> PythonSourceModel {
        PythonSourceModel {
            file: self.file,
            module: self.module,
            status: PythonSourceModelStatus::Parsed,
            imports: self.imports,
            assignments: self.assignments,
            calls: self.calls,
            class_defs: self.class_defs,
            function_defs: self.function_defs,
        }
    }
}

fn static_value(expr: &Expr) -> StaticValue {
    match expr {
        Expr::StringLiteral(string) => StaticValue::String(string.value.to_str().to_string()),
        Expr::List(list) => StaticValue::StringList(
            list.elts
                .iter()
                .map(|element| match element {
                    Expr::StringLiteral(string) => {
                        StaticValueSegment::known(string.value.to_str().to_string())
                    }
                    Expr::Starred(_) => {
                        StaticValueSegment::unknown(StaticValueIssue::SpreadElement)
                    }
                    other => StaticValueSegment::unknown(unsupported_expr(other)),
                })
                .collect(),
        ),
        Expr::Tuple(tuple) => StaticValue::StringList(
            tuple
                .elts
                .iter()
                .map(|element| match element {
                    Expr::StringLiteral(string) => {
                        StaticValueSegment::known(string.value.to_str().to_string())
                    }
                    Expr::Starred(_) => {
                        StaticValueSegment::unknown(StaticValueIssue::SpreadElement)
                    }
                    other => StaticValueSegment::unknown(unsupported_expr(other)),
                })
                .collect(),
        ),
        Expr::Dict(dict) => {
            let mut entries = Vec::new();
            for item in dict.iter() {
                let Some(Expr::StringLiteral(key)) = &item.key else {
                    return StaticValue::Unknown {
                        issue: StaticValueIssue::UnsupportedDictKey,
                    };
                };
                entries.push((key.value.to_str().to_string(), static_value(&item.value)));
            }
            StaticValue::Dict(entries)
        }
        other => StaticValue::Unknown {
            issue: unsupported_expr(other),
        },
    }
}

fn unsupported_expr(expr: &Expr) -> StaticValueIssue {
    StaticValueIssue::UnsupportedExpression {
        kind: expr_kind(expr),
    }
}

fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::BoolOp(_) => "bool_op",
        Expr::Named(_) => "named_expr",
        Expr::BinOp(_) => "bin_op",
        Expr::UnaryOp(_) => "unary_op",
        Expr::Lambda(_) => "lambda",
        Expr::If(_) => "if_expr",
        Expr::Dict(_) => "dict",
        Expr::Set(_) => "set",
        Expr::ListComp(_) => "list_comp",
        Expr::SetComp(_) => "set_comp",
        Expr::DictComp(_) => "dict_comp",
        Expr::Generator(_) => "generator",
        Expr::Await(_) => "await",
        Expr::Yield(_) => "yield",
        Expr::YieldFrom(_) => "yield_from",
        Expr::Compare(_) => "compare",
        Expr::Call(_) => "call",
        Expr::FString(_) => "f_string",
        Expr::TString(_) => "t_string",
        Expr::StringLiteral(_) => "string_literal",
        Expr::BytesLiteral(_) => "bytes_literal",
        Expr::NumberLiteral(_) => "number_literal",
        Expr::BooleanLiteral(_) => "boolean_literal",
        Expr::NoneLiteral(_) => "none_literal",
        Expr::EllipsisLiteral(_) => "ellipsis_literal",
        Expr::Attribute(_) => "attribute",
        Expr::Subscript(_) => "subscript",
        Expr::Starred(_) => "starred",
        Expr::Name(_) => "name",
        Expr::List(_) => "list",
        Expr::Tuple(_) => "tuple",
        Expr::Slice(_) => "slice",
        Expr::IpyEscapeCommand(_) => "ipy_escape_command",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::DiscoveredSourceFile;
    use djls_source::FileRootKind;
    use djls_source::LoadedSourceFile;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;
    use djls_source::SourceRoot;
    use djls_source::SourceRootEntry;
    use djls_source::SourceRootId;
    use rustc_hash::FxHashMap;
    use salsa::Database;

    use super::*;
    use crate::build_project_discovery_data;
    use crate::build_source_roots;
    use crate::first_party_discovery_files_request;
    use crate::first_party_source_files_load_request;
    use crate::merge_first_party_source_file_patch;
    use crate::run_loading_plan;
    use crate::DjangoEnvironmentCandidatesOutcome;
    use crate::FirstPartySourceFilePatch;
    use crate::LoadingApplyOutcome;
    use crate::LoadingEffects;
    use crate::LoadingObservationOutcome;
    use crate::LoadingPlan;
    use crate::LoadingRunControl;
    use crate::NoopLoadingObserver;
    use crate::ProjectDiscovery;
    use crate::ProjectDiscoveryApplyResult;
    use crate::ProjectDiscoveryLoadRequest;
    use crate::ProjectDiscoverySetData;
    use crate::ProjectEnrichment;
    use crate::ProjectSourceFilesApplyResult;
    use crate::ReadyProjectSourceFiles;

    #[salsa::db]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        sources: FxHashMap<Utf8PathBuf, String>,
        project: OnceLock<Project>,
        events: Arc<Mutex<Vec<salsa::Event>>>,
    }

    impl Default for TestDb {
        fn default() -> Self {
            let events = Arc::new(Mutex::new(Vec::new()));
            let storage = salsa::Storage::new(Some(Box::new({
                let events = Arc::clone(&events);
                move |event| {
                    events
                        .lock()
                        .expect("event log is not poisoned")
                        .push(event)
                }
            })));
            Self {
                storage,
                files: SourceFiles::default(),
                sources: FxHashMap::default(),
                project: OnceLock::new(),
                events,
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
            Ok(self.sources.get(path).cloned().unwrap_or_default())
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {
        fn project(&self) -> Project {
            *self.project.get().expect("test project initialized")
        }
    }

    impl TestDb {
        fn with_project() -> Self {
            let db = Self::default();
            db.project
                .set(Project::new(
                    &db,
                    ProjectSourceInventory::Unavailable {
                        issue: ProjectSourceFilesIssue::NotLoaded,
                    },
                    ProjectDiscovery::Absent,
                    ProjectEnrichment::Absent,
                ))
                .expect("project should initialize once");
            db
        }

        fn set_file(&mut self, path: &str, source: &str) -> File {
            let path = Utf8PathBuf::from(path);
            self.sources.insert(path.clone(), source.to_string());
            self.get_or_create_file(path.as_path())
        }

        fn take_events(&self) -> Vec<salsa::Event> {
            std::mem::take(&mut *self.events.lock().expect("event log is not poisoned"))
        }

        fn tracked_query_executed(&self, events: &[salsa::Event], query_name: &str) -> bool {
            events.iter().any(|event| match &event.kind {
                salsa::EventKind::WillExecute { database_key } => self
                    .ingredient_debug_name(database_key.ingredient_index())
                    .contains(query_name),
                _ => false,
            })
        }
    }

    struct PythonSourceLoadingEffects<'db> {
        db: &'db TestDb,
    }

    impl LoadingEffects for PythonSourceLoadingEffects<'_> {
        fn begin_loading_run(&mut self) -> LoadingRunControl {
            LoadingRunControl::Continue
        }

        fn load_source_file_set(&mut self) -> FirstPartySourceFilePatch {
            let plan = build_source_roots(Vec::new());
            let (root_issues, request) =
                first_party_discovery_files_request(first_party_source_files_load_request(plan));
            FirstPartySourceFilePatch::first_party(
                root_issues,
                djls_workspace::load_files_for_roots(request),
            )
        }

        fn apply_source_file_patch(
            &mut self,
            patch: FirstPartySourceFilePatch,
        ) -> LoadingApplyOutcome<ProjectSourceFilesApplyResult> {
            let update = merge_first_party_source_file_patch(None, patch);
            let transition = update.applied_transition().clone();
            LoadingApplyOutcome::Applied(ProjectSourceFilesApplyResult::Deferred {
                transition,
                issue: ProjectSourceFilesIssue::NotLoaded,
                previous: None,
            })
        }

        fn load_project_discovery_set(&mut self) -> ProjectDiscoverySetData {
            build_project_discovery_data(ProjectDiscoveryLoadRequest::new(
                Vec::new(),
                djls_conf::Settings::default(),
            ))
        }

        fn apply_project_discovery_data(
            &mut self,
            _data: ProjectDiscoverySetData,
        ) -> LoadingApplyOutcome<ProjectDiscoveryApplyResult> {
            LoadingApplyOutcome::Applied(ProjectDiscoveryApplyResult::Unavailable(
                ProjectDiscovery::Absent,
            ))
        }

        fn observe_python_source_index(
            &mut self,
        ) -> LoadingObservationOutcome<PythonSourceIndexOutcome> {
            LoadingObservationOutcome::Observed(
                python_source_index(self.db, self.db.project()).clone(),
            )
        }

        fn observe_django_environment_candidates(
            &mut self,
        ) -> LoadingObservationOutcome<DjangoEnvironmentCandidatesOutcome> {
            LoadingObservationOutcome::Observed(DjangoEnvironmentCandidatesOutcome::Ready {
                candidates: Vec::new(),
                issues: Vec::new(),
            })
        }
    }

    fn ready_inventory(db: &TestDb, paths: &[&str]) -> ProjectSourceInventory {
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path, FileRootKind::Project);
        let roots = vec![SourceRootEntry::new(root)];
        let files = paths
            .iter()
            .map(|path| {
                let path = Utf8PathBuf::from(path);
                let discovered = DiscoveredSourceFile::new(path.clone(), root_id.clone());
                LoadedSourceFile::from_discovered(discovered, db.get_or_create_file(&path))
            })
            .collect::<Vec<_>>();
        let data = SourceFileSetData::new(roots, files).expect("test data should be valid");
        let set = SourceFileSet::new(db, data);
        ProjectSourceInventory::Ready(ReadyProjectSourceFiles::merged_for_test(set))
    }

    #[test]
    fn python_source_model_extracts_imports_assignments_calls_and_defs() {
        let mut db = TestDb::with_project();
        let file = db.set_file(
            "/workspace/app/settings.py",
            r#"
import os as operating_system
from django.conf import settings as django_settings

INSTALLED_APPS = ["django.contrib.auth", OTHER]
DATABASES = {"default": {"ENGINE": "django.db.backends.sqlite3"}}
configure(DEBUG=True)
class AppConfig(BaseConfig):
    pass
async def build():
    return None
"#,
        );

        let model = python_source_model(&db, file);

        assert!(matches!(
            model.module(),
            PyModuleNameResolution::Unknown {
                issue: ModuleNameIssue::OutsideImportRoots { .. }
            }
        ));
        assert_eq!(model.status(), &PythonSourceModelStatus::Parsed);
        assert_eq!(model.imports().len(), 2);
        assert_eq!(model.assignments().len(), 2);
        assert_eq!(model.calls().len(), 1);
        assert_eq!(model.class_defs()[0].name(), "AppConfig");
        assert_eq!(model.function_defs()[0].name(), "build");
        let StaticValue::StringList(segments) = model.assignments()[0].value() else {
            panic!("installed apps should be extracted as a string list");
        };
        assert_eq!(segments[0].value.as_deref(), Some("django.contrib.auth"));
        assert!(segments[1].issue.is_some());
    }

    #[test]
    fn python_source_index_reuse_after_loading_python_source_models_ready() {
        let mut db = TestDb::with_project();
        db.set_file("/workspace/app/models.py", "class Book:\n    pass\n");
        db.set_project_source_inventory(ready_inventory(&db, &["/workspace/app/models.py"]));

        let mut effects = PythonSourceLoadingEffects { db: &db };
        let result = run_loading_plan(
            LoadingPlan::phase3(),
            &mut effects,
            &mut NoopLoadingObserver,
        );
        assert!(result.execution_outcome().is_none());
        let events = db.take_events();
        assert!(db.tracked_query_executed(&events, "python_source_index"));

        let PythonSourceIndexOutcome::Ready(index) = python_source_index(&db, db.project()) else {
            panic!("python source index should be reused");
        };
        assert_eq!(index.len(), 1);
        let events = db.take_events();

        assert!(!db.tracked_query_executed(&events, "python_source_index"));
    }

    #[test]
    fn python_source_index_distinguishes_deferred_skipped_and_ready_states() {
        let mut db = TestDb::with_project();

        assert!(matches!(
            python_source_index(&db, db.project()),
            PythonSourceIndexOutcome::Deferred { .. }
        ));

        db.set_project_source_inventory(ready_inventory(&db, &["/workspace/templates/index.html"]));
        assert_eq!(
            python_source_index(&db, db.project()).clone(),
            PythonSourceIndexOutcome::Skipped {
                issue: PythonSourceIndexIssue::NoPythonFiles,
            }
        );

        db.set_file("/workspace/app/models.py", "class Book:\n    pass\n");
        db.set_project_source_inventory(ready_inventory(&db, &["/workspace/app/models.py"]));
        let PythonSourceIndexOutcome::Ready(index) = python_source_index(&db, db.project()) else {
            panic!("python source index should be ready");
        };
        assert_eq!(index.len(), 1);
        let PyModuleNameResolution::Resolved(module) = index.models()[0].module() else {
            panic!("indexed model should resolve module name through layout");
        };
        assert_eq!(module.as_str(), "app.models");
    }

    #[test]
    fn python_source_model_preserves_parse_errors() {
        let mut db = TestDb::with_project();
        let file = db.set_file("/workspace/app/broken.py", "if broken:\n");

        let model = python_source_model(&db, file);

        assert_eq!(
            model.status(),
            &PythonSourceModelStatus::ParseError {
                issue: PythonSourceModelIssue::ParseError,
            }
        );
        assert!(model.imports().is_empty());
    }
}
