use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileKind;
use ruff_python_ast::Comprehension;
use ruff_python_ast::ExceptHandler;
use ruff_python_ast::Expr;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;

use crate::layout::project_layout_index;
use crate::layout::ProjectLayoutIndexOutcome;
use crate::project::Project;
use crate::source_files::SourceFileInventory;
use crate::source_files::SourceFilesIssue;
use crate::Db;
use crate::PyModuleName;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonSourceModel {
    file: File,
    module: PyModuleNameResolution,
    parse_status: PythonSourceParseStatus,
    imports: Vec<ImportStatement>,
    assignments: Vec<Assignment>,
    calls: Vec<CallExpression>,
    class_defs: Vec<ClassDef>,
    function_defs: Vec<FunctionDef>,
    operations: Vec<PythonSourceOperation>,
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
    pub fn parse_status(&self) -> &PythonSourceParseStatus {
        &self.parse_status
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

    #[must_use]
    pub fn operations(&self) -> &[PythonSourceOperation] {
        &self.operations
    }

    fn with_module(mut self, module: PyModuleNameResolution) -> Self {
        self.module = module;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonSourceParseStatus {
    Parsed,
    InvalidSyntax,
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
    Unindexed(PythonSourceIndexIssue),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonSourceIndexIssue {
    NoPythonFiles,
    SourceInventoryUnavailable(SourceFilesIssue),
    LayoutUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PyModuleNameResolution {
    Resolved(PyModuleName),
    Unknown(ModuleNameIssue),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleNameIssue {
    NonPythonFile(Utf8PathBuf),
    OutsideImportRoots(Utf8PathBuf),
    InvalidModuleName(Utf8PathBuf),
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
    kind: AssignmentKind,
    targets: Vec<AssignmentTarget>,
    value: StaticValue,
}

impl Assignment {
    #[must_use]
    pub fn kind(&self) -> AssignmentKind {
        self.kind
    }

    #[must_use]
    pub fn targets(&self) -> &[AssignmentTarget] {
        &self.targets
    }

    #[must_use]
    pub fn value(&self) -> &StaticValue {
        &self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentKind {
    Assign,
    AugAdd,
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
    assignments: Vec<Assignment>,
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

    #[must_use]
    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonSourceOperation {
    Import(ImportStatement),
    Assignment(Assignment),
    Call(CallExpression),
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
    Bool(bool),
    StringList(Vec<StaticValueSegment<String>>),
    List(Vec<StaticValue>),
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
    let module_resolution =
        PyModuleNameResolution::Unknown(ModuleNameIssue::OutsideImportRoots(file.path(db).clone()));
    if *source.kind() != FileKind::Python {
        return PythonSourceModel {
            file,
            module: PyModuleNameResolution::Unknown(ModuleNameIssue::NonPythonFile(
                file.path(db).clone(),
            )),
            parse_status: PythonSourceParseStatus::Parsed,
            imports: Vec::new(),
            assignments: Vec::new(),
            calls: Vec::new(),
            class_defs: Vec::new(),
            function_defs: Vec::new(),
            operations: Vec::new(),
        };
    }

    let parsed = match ruff_python_parser::parse_module(source.as_ref()) {
        Ok(parsed) => parsed.into_syntax(),
        Err(_) => {
            return PythonSourceModel {
                file,
                module: module_resolution,
                parse_status: PythonSourceParseStatus::InvalidSyntax,
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::new(),
                class_defs: Vec::new(),
                function_defs: Vec::new(),
                operations: Vec::new(),
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
        SourceFileInventory::Ready(files) => files,
        SourceFileInventory::Unavailable {
            issue: SourceFilesIssue::NotLoaded,
        } => {
            return PythonSourceIndexOutcome::Unindexed(
                PythonSourceIndexIssue::SourceInventoryUnavailable(SourceFilesIssue::NotLoaded),
            );
        }
        SourceFileInventory::Unavailable { issue } => {
            return PythonSourceIndexOutcome::Unindexed(
                PythonSourceIndexIssue::SourceInventoryUnavailable(issue),
            );
        }
    };

    let ProjectLayoutIndexOutcome::Ready(layout) = project_layout_index(db, project) else {
        return PythonSourceIndexOutcome::Unindexed(PythonSourceIndexIssue::LayoutUnavailable);
    };

    let models = files
        .merged()
        .data(db)
        .files()
        .iter()
        .filter(|file| file.kind() == FileKind::Python)
        .filter_map(|file| {
            let module = layout.module_name_for_path(file.path()).map_or_else(
                || {
                    PyModuleNameResolution::Unknown(ModuleNameIssue::OutsideImportRoots(
                        file.path().to_owned(),
                    ))
                },
                PyModuleNameResolution::Resolved,
            );
            layout
                .file_for_path(file.path())
                .map(|file| python_source_model(db, file).clone().with_module(module))
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        PythonSourceIndexOutcome::Unindexed(PythonSourceIndexIssue::NoPythonFiles)
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
    operations: Vec<PythonSourceOperation>,
    scope_depth: usize,
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
            operations: Vec::new(),
            scope_depth: 0,
        }
    }

    fn collect_body(&mut self, body: &[Stmt]) {
        for stmt in body {
            self.collect_stmt(stmt);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn collect_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Import(stmt) => {
                let imports = stmt
                    .names
                    .iter()
                    .map(|alias| ImportStatement::Import {
                        module: QualifiedName::parse(alias.name.as_str()),
                        alias: alias.asname.as_ref().map(ToString::to_string),
                    })
                    .collect::<Vec<_>>();
                if self.scope_depth == 0 {
                    self.operations
                        .extend(imports.iter().cloned().map(PythonSourceOperation::Import));
                }
                self.imports.extend(imports);
            }
            Stmt::ImportFrom(stmt) => {
                let imports = stmt
                    .names
                    .iter()
                    .map(|alias| ImportStatement::ImportFrom {
                        module: stmt
                            .module
                            .as_ref()
                            .map(|module| QualifiedName::parse(module.as_str())),
                        name: alias.name.to_string(),
                        alias: alias.asname.as_ref().map(ToString::to_string),
                        level: stmt.level,
                    })
                    .collect::<Vec<_>>();
                if self.scope_depth == 0 {
                    self.operations
                        .extend(imports.iter().cloned().map(PythonSourceOperation::Import));
                }
                self.imports.extend(imports);
            }
            Stmt::Assign(stmt) => {
                for target in &stmt.targets {
                    self.collect_expr(target);
                }
                self.collect_expr(&stmt.value);
                let assignment = Assignment {
                    kind: AssignmentKind::Assign,
                    targets: stmt
                        .targets
                        .iter()
                        .filter_map(QualifiedName::from_expr)
                        .map(|name| AssignmentTarget { name })
                        .collect(),
                    value: static_value(&stmt.value),
                };
                if self.scope_depth == 0 {
                    self.operations
                        .push(PythonSourceOperation::Assignment(assignment.clone()));
                }
                self.assignments.push(assignment);
            }
            Stmt::AugAssign(stmt) => {
                self.collect_expr(&stmt.target);
                self.collect_expr(&stmt.value);
                if stmt.op == Operator::Add {
                    let assignment = Assignment {
                        kind: AssignmentKind::AugAdd,
                        targets: QualifiedName::from_expr(&stmt.target)
                            .into_iter()
                            .map(|name| AssignmentTarget { name })
                            .collect(),
                        value: static_value(&stmt.value),
                    };
                    if self.scope_depth == 0 {
                        self.operations
                            .push(PythonSourceOperation::Assignment(assignment.clone()));
                    }
                    self.assignments.push(assignment);
                }
            }
            Stmt::AnnAssign(stmt) => {
                self.collect_expr(&stmt.target);
                self.collect_expr(&stmt.annotation);
                if let Some(value) = &stmt.value {
                    self.collect_expr(value);
                }
            }
            Stmt::ClassDef(stmt) => {
                let assignments = stmt
                    .body
                    .iter()
                    .filter_map(class_assignment_from_stmt)
                    .collect::<Vec<_>>();
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
                    assignments,
                });
                for decorator in &stmt.decorator_list {
                    self.collect_expr(&decorator.expression);
                }
                self.scope_depth += 1;
                self.collect_body(&stmt.body);
                self.scope_depth -= 1;
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
                self.scope_depth += 1;
                self.collect_body(&stmt.body);
                self.scope_depth -= 1;
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

    #[allow(clippy::too_many_lines)]
    fn collect_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call(call) => {
                let call_expression = CallExpression {
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
                };
                if self.scope_depth == 0 {
                    self.operations
                        .push(PythonSourceOperation::Call(call_expression.clone()));
                }
                self.calls.push(call_expression);
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
                for item in dict {
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
                for comparator in &expr.comparators {
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
            parse_status: PythonSourceParseStatus::Parsed,
            imports: self.imports,
            assignments: self.assignments,
            calls: self.calls,
            class_defs: self.class_defs,
            function_defs: self.function_defs,
            operations: self.operations,
        }
    }
}

fn class_assignment_from_stmt(stmt: &Stmt) -> Option<Assignment> {
    match stmt {
        Stmt::Assign(stmt) => Some(Assignment {
            kind: AssignmentKind::Assign,
            targets: stmt
                .targets
                .iter()
                .filter_map(QualifiedName::from_expr)
                .map(|name| AssignmentTarget { name })
                .collect(),
            value: static_value(&stmt.value),
        }),
        Stmt::AugAssign(stmt) if stmt.op == Operator::Add => Some(Assignment {
            kind: AssignmentKind::AugAdd,
            targets: QualifiedName::from_expr(&stmt.target)
                .into_iter()
                .map(|name| AssignmentTarget { name })
                .collect(),
            value: static_value(&stmt.value),
        }),
        _ => None,
    }
}

fn static_sequence_value(elements: &[Expr]) -> StaticValue {
    if elements.iter().all(|element| {
        matches!(
            element,
            Expr::StringLiteral(_) | Expr::Starred(_) | Expr::Name(_) | Expr::Attribute(_)
        )
    }) {
        return StaticValue::StringList(
            elements
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
        );
    }
    StaticValue::List(elements.iter().map(static_value).collect())
}

fn static_value(expr: &Expr) -> StaticValue {
    match expr {
        Expr::StringLiteral(string) => StaticValue::String(string.value.to_str().to_string()),
        Expr::BooleanLiteral(boolean) => StaticValue::Bool(boolean.value),
        Expr::List(list) => static_sequence_value(&list.elts),
        Expr::Tuple(tuple) => static_sequence_value(&tuple.elts),
        Expr::BinOp(bin_op) if bin_op.op == Operator::Add => {
            let left = static_value(&bin_op.left);
            let right = static_value(&bin_op.right);
            match (left, right) {
                (StaticValue::StringList(mut left), StaticValue::StringList(right)) => {
                    left.extend(right);
                    StaticValue::StringList(left)
                }
                (StaticValue::StringList(mut left), other) => {
                    left.push(StaticValueSegment::unknown(static_value_issue(&other)));
                    StaticValue::StringList(left)
                }
                (other, StaticValue::StringList(mut right)) => {
                    let mut segments =
                        vec![StaticValueSegment::unknown(static_value_issue(&other))];
                    segments.append(&mut right);
                    StaticValue::StringList(segments)
                }
                (left, right) => StaticValue::StringList(vec![
                    StaticValueSegment::unknown(static_value_issue(&left)),
                    StaticValueSegment::unknown(static_value_issue(&right)),
                ]),
            }
        }
        Expr::Dict(dict) => {
            let mut entries = Vec::new();
            for item in dict {
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

fn static_value_issue(value: &StaticValue) -> StaticValueIssue {
    match value {
        StaticValue::Unknown { issue } => issue.clone(),
        StaticValue::String(_)
        | StaticValue::Bool(_)
        | StaticValue::StringList(_)
        | StaticValue::List(_)
        | StaticValue::Dict(_) => StaticValueIssue::UnsupportedExpression { kind: "bin_op" },
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
    use crate::enrichment::ProjectEnrichment;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::root_discovery::ProjectRootDiscoveryApplyResult;
    use crate::root_discovery::ProjectRootDiscoveryUpdate;
    use crate::run_django_discovery;
    use crate::source_files::ReadySourceFiles;
    use crate::source_files::SourceFilesApplyResult;
    use crate::DiscoveryApplyOutcome;
    use crate::DiscoveryCancellation;
    use crate::DiscoveryHost;
    use crate::DiscoveryObservationOutcome;
    use crate::DjangoDiscoveryRequest;
    use crate::DjangoEnvironmentCandidatesOutcome;
    use crate::NoopDiscoveryObserver;

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
                        .push(event);
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
                    SourceFileInventory::Unavailable {
                        issue: SourceFilesIssue::NotLoaded,
                    },
                    ProjectRootDiscovery::Absent,
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

    struct PythonSourceDiscoveryHost<'db> {
        db: &'db TestDb,
    }

    impl DiscoveryHost for PythonSourceDiscoveryHost<'_> {
        fn checkpoint(&mut self) -> Result<(), DiscoveryCancellation> {
            Ok(())
        }

        fn load_files_for_roots(
            &mut self,
            request: djls_workspace::FilesForRootsRequest,
        ) -> Result<djls_workspace::FilesForRootsResult, DiscoveryCancellation> {
            Ok(djls_workspace::load_files_for_roots(request))
        }

        fn current_source_files(&mut self) -> Option<ReadySourceFiles> {
            None
        }

        fn apply_source_files(
            &mut self,
            update: crate::SourceFilesUpdate,
        ) -> DiscoveryApplyOutcome<SourceFilesApplyResult> {
            let transition = update.applied_transition().clone();
            DiscoveryApplyOutcome::Applied(SourceFilesApplyResult::Deferred {
                transition,
                issue: SourceFilesIssue::NotLoaded,
                previous: None,
            })
        }

        fn apply_project_root_discovery(
            &mut self,
            _update: ProjectRootDiscoveryUpdate,
        ) -> DiscoveryApplyOutcome<ProjectRootDiscoveryApplyResult> {
            DiscoveryApplyOutcome::Applied(ProjectRootDiscoveryApplyResult::Unavailable(
                ProjectRootDiscovery::Absent,
            ))
        }

        fn observe_python_source_index(
            &mut self,
        ) -> DiscoveryObservationOutcome<PythonSourceIndexOutcome> {
            DiscoveryObservationOutcome::Observed(
                python_source_index(self.db, self.db.project()).clone(),
            )
        }

        fn observe_django_environment_candidates(
            &mut self,
        ) -> DiscoveryObservationOutcome<DjangoEnvironmentCandidatesOutcome> {
            DiscoveryObservationOutcome::Observed(DjangoEnvironmentCandidatesOutcome::Ready {
                candidates: Vec::new(),
                issues: Vec::new(),
            })
        }

        fn observe_installed_app_file_roots(
            &mut self,
        ) -> DiscoveryObservationOutcome<crate::InstalledAppFileRootsDiscovery> {
            DiscoveryObservationOutcome::Observed(crate::InstalledAppFileRootsDiscovery::Ready(
                crate::InstalledAppFileRoots::new(Vec::new(), Vec::new()),
            ))
        }

        fn observe_template_directory_file_roots(
            &mut self,
        ) -> DiscoveryObservationOutcome<crate::TemplateDirectoryFileRootsDiscovery> {
            DiscoveryObservationOutcome::Observed(
                crate::TemplateDirectoryFileRootsDiscovery::Ready(
                    crate::TemplateDirectoryFileRoots::new(Vec::new(), Vec::new()),
                ),
            )
        }

        fn load_project_enrichment(
            &mut self,
        ) -> Result<crate::ProjectEnrichment, DiscoveryCancellation> {
            Ok(crate::ProjectEnrichment::Disabled)
        }

        fn apply_project_enrichment(
            &mut self,
            enrichment: crate::ProjectEnrichment,
        ) -> DiscoveryApplyOutcome<crate::ProjectEnrichment> {
            DiscoveryApplyOutcome::Applied(enrichment)
        }
    }

    fn ready_inventory(db: &TestDb, paths: &[&str]) -> SourceFileInventory {
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
        SourceFileInventory::Ready(ReadySourceFiles::new(
            crate::source_files::SourceFileSetPartitions::default(),
            set,
        ))
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
            PyModuleNameResolution::Unknown(ModuleNameIssue::OutsideImportRoots(_))
        ));
        assert_eq!(model.parse_status(), &PythonSourceParseStatus::Parsed);
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
        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/app/models.py"]));

        let mut host = PythonSourceDiscoveryHost { db: &db };
        let request = DjangoDiscoveryRequest::new(Vec::new(), djls_conf::Settings::default());
        let result = run_django_discovery(&request, &mut host, &mut NoopDiscoveryObserver);
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
            PythonSourceIndexOutcome::Unindexed(
                PythonSourceIndexIssue::SourceInventoryUnavailable(SourceFilesIssue::NotLoaded)
            )
        ));

        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/templates/index.html"]));
        assert_eq!(
            python_source_index(&db, db.project()).clone(),
            PythonSourceIndexOutcome::Unindexed(PythonSourceIndexIssue::NoPythonFiles)
        );

        db.set_file("/workspace/app/models.py", "class Book:\n    pass\n");
        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/app/models.py"]));
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
            model.parse_status(),
            &PythonSourceParseStatus::InvalidSyntax
        );
        assert!(model.imports().is_empty());
    }
}
