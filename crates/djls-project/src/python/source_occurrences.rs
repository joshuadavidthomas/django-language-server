use std::collections::BTreeMap;

use djls_source::File;
use djls_source::Span;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtClassDef;
use ruff_python_ast::StmtFunctionDef;
use ruff_python_ast::visitor;
use ruff_python_ast::visitor::Visitor;

use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::PythonSourceModule;
use crate::python::RecoveredPythonModule;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;
use crate::python::module::PythonImportChainResolution;
use crate::python::module::PythonImportRequest;

const MAX_REEXPORT_DEPTH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
struct LazyFromImport {
    level: u32,
    module: Option<String>,
    member: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Binding {
    Module(PythonModule),
    ModuleMember(PythonModule, Box<Binding>),
    LazyFromImport(LazyFromImport),
    String(String),
    Function(PythonFunctionDefinition),
    Unknown,
}

#[derive(Clone, Debug)]
enum ResolvedValue {
    Module(PythonModule),
    String(String),
    Function(PythonFunctionDefinition),
}

#[derive(Clone, Debug)]
enum MemberLookup {
    Found(ResolvedValue),
    Absent,
    Uncertain,
}

/// Stable source identity for an undecorated module-level Python function.
///
/// The definition span, together with the file, distinguishes definitions that
/// reuse a name. Python owns the AST lookup so consumers never repeat that
/// identity check against a recovered parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PythonFunctionDefinition {
    file: File,
    definition_span: Span,
    name: String,
}

impl PythonFunctionDefinition {
    #[must_use]
    pub(crate) fn file(&self) -> File {
        self.file
    }

    #[must_use]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub(crate) fn statement<'db>(&self, db: &'db dyn ProjectDb) -> Option<&'db StmtFunctionDef> {
        let module = RecoveredPythonModule::from_file(db, self.file)
            .ok()
            .flatten()?;
        module.body(db).iter().find_map(|statement| {
            let Stmt::FunctionDef(function) = statement else {
                return None;
            };
            (function.span() == self.definition_span).then_some(function)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PythonOccurrenceValue {
    String(String),
    Function(PythonFunctionDefinition),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PythonSourceOccurrence {
    value: Option<PythonOccurrenceValue>,
    consulted_files: Vec<File>,
    recovered_source: bool,
}

/// On-demand access to exact Python facts at selected source occurrences.
///
/// Each lookup scans only as far as its target statement. The facade merges
/// evidence from the operands its consumer actually asks about.
pub(crate) struct PythonSourceLookup<'db> {
    db: &'db dyn ProjectDb,
    project: Option<Project>,
    module: Option<PythonSourceModule>,
    file: File,
    consulted_files: Vec<File>,
    recovered_source: bool,
}

impl<'db> PythonSourceLookup<'db> {
    pub(crate) fn for_file(db: &'db dyn ProjectDb, file: File) -> Self {
        Self {
            db,
            project: None,
            module: None,
            file,
            consulted_files: Vec::new(),
            recovered_source: false,
        }
    }

    pub(crate) fn for_module(
        db: &'db dyn ProjectDb,
        project: Project,
        module: PythonSourceModule,
    ) -> Self {
        Self {
            db,
            project: Some(project),
            file: module.file(),
            module: Some(module),
            consulted_files: Vec::new(),
            recovered_source: false,
        }
    }

    pub(crate) fn exact_string(&mut self, expression: &Expr) -> Option<String> {
        if let Some(value) = expression.string_literal() {
            return Some(value.to_string());
        }
        match self.lookup(expression) {
            Some(PythonOccurrenceValue::String(value)) => Some(value),
            Some(PythonOccurrenceValue::Function(_)) | None => None,
        }
    }

    pub(crate) fn function(&mut self, expression: &Expr) -> Option<PythonFunctionDefinition> {
        match self.lookup(expression) {
            Some(PythonOccurrenceValue::Function(definition)) => Some(definition),
            Some(PythonOccurrenceValue::String(_)) | None => None,
        }
    }

    #[must_use]
    pub(crate) fn consulted_files(&self) -> &[File] {
        &self.consulted_files
    }

    #[must_use]
    pub(crate) fn has_recovered_source(&self) -> bool {
        self.recovered_source
    }

    fn lookup(&mut self, expression: &Expr) -> Option<PythonOccurrenceValue> {
        let occurrence = python_source_occurrence(
            self.db,
            self.project,
            self.module.clone(),
            self.file,
            expression.span(),
        );
        self.recovered_source |= occurrence.recovered_source;
        for file in &occurrence.consulted_files {
            if !self.consulted_files.contains(file) {
                self.consulted_files.push(*file);
            }
        }
        occurrence.value.clone()
    }
}

#[salsa::tracked(returns(ref))]
fn python_source_occurrence(
    db: &dyn ProjectDb,
    project: Option<Project>,
    module: Option<PythonSourceModule>,
    file: File,
    target: Span,
) -> PythonSourceOccurrence {
    let Ok(Some(parsed)) = RecoveredPythonModule::from_file(db, file) else {
        return PythonSourceOccurrence::default();
    };
    let mut analysis = SourceOccurrenceAnalysis::new(db, project, module, file);
    analysis.recovered_source = parsed.has_ordinary_syntax_errors(db);
    for statement in parsed.body(db) {
        if span_contains(statement.span(), target) {
            if statement_contains_named_binding(statement) {
                return analysis.finish(None);
            }
            let expression = find_expression(statement, target);
            let value = expression
                .and_then(|expression| analysis.resolve_expression(expression, 0))
                .and_then(occurrence_value);
            return analysis.finish(value);
        }
        if statement_contains_named_binding(statement) {
            analysis.bindings.clear();
        } else {
            analysis.apply_statement_effects(statement);
        }
    }
    analysis.finish(None)
}

struct SourceOccurrenceAnalysis<'db> {
    db: &'db dyn ProjectDb,
    project: Option<Project>,
    importer: Option<PythonSourceModule>,
    file: File,
    bindings: BTreeMap<String, Binding>,
    consulted_files: Vec<File>,
    recovered_source: bool,
}

impl<'db> SourceOccurrenceAnalysis<'db> {
    fn new(
        db: &'db dyn ProjectDb,
        project: Option<Project>,
        importer: Option<PythonSourceModule>,
        file: File,
    ) -> Self {
        Self {
            db,
            project,
            importer,
            file,
            bindings: BTreeMap::new(),
            consulted_files: Vec::new(),
            recovered_source: false,
        }
    }

    fn finish(mut self, value: Option<PythonOccurrenceValue>) -> PythonSourceOccurrence {
        self.consulted_files
            .sort_by(|left, right| left.path(self.db).cmp(right.path(self.db)));
        self.consulted_files.dedup();
        PythonSourceOccurrence {
            value,
            consulted_files: self.consulted_files,
            recovered_source: self.recovered_source,
        }
    }

    fn apply_statement_effects(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Import(import) => {
                for clause in DirectImportClause::lower(import) {
                    let binding = if clause.binds_root() && clause.requested() != clause.target() {
                        Binding::Unknown
                    } else {
                        self.resolve_import(0, Some(clause.target()))
                            .map_or(Binding::Unknown, Binding::Module)
                    };
                    self.bindings.insert(clause.bound().to_string(), binding);
                }
            }
            Stmt::ImportFrom(import) => {
                let syntax = FromImportSyntax::lower(import);
                if syntax.has_star() {
                    self.bindings.clear();
                }
                for member in syntax.named_members() {
                    let binding = if self.project.is_some() && self.importer.is_some() {
                        Binding::LazyFromImport(LazyFromImport {
                            level: syntax.level(),
                            module: syntax.module().map(str::to_string),
                            member: member.imported().to_string(),
                        })
                    } else {
                        Binding::Unknown
                    };
                    self.bindings.insert(member.bound().to_string(), binding);
                }
            }
            Stmt::Assign(assign) => {
                self.invalidate_escaped_modules(&assign.value);
                let value = self.resolve_local_expr(&assign.value);
                for target in &assign.targets {
                    self.bind_target(target, value.clone());
                }
            }
            Stmt::AnnAssign(assign) => {
                if let Some(value) = assign.value.as_deref() {
                    self.invalidate_escaped_modules(value);
                }
                let value = assign
                    .value
                    .as_deref()
                    .and_then(|value| self.resolve_local_expr(value));
                self.bind_target(&assign.target, value);
            }
            Stmt::FunctionDef(function) => {
                let binding = if function.decorator_list.is_empty() {
                    Binding::Function(function_definition(self.file, function))
                } else {
                    Binding::Unknown
                };
                self.bindings.insert(function.name.to_string(), binding);
            }
            Stmt::ClassDef(class) => {
                self.bindings
                    .insert(class.name.to_string(), Binding::Unknown);
            }
            Stmt::AugAssign(assign) => self.bind_target(&assign.target, None),
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    self.bind_target(target, None);
                }
            }
            Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::Match(_)
            | Stmt::Try(_)
            | Stmt::TypeAlias(_) => self.bindings.clear(),
            Stmt::Expr(expression) => self.invalidate_escaped_modules(&expression.value),
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

    fn bind_target(&mut self, target: &Expr, value: Option<Binding>) {
        if let Some(name) = target.name_target() {
            self.bindings
                .insert(name.to_string(), value.unwrap_or(Binding::Unknown));
        } else if let Some(root) = mutation_root(target) {
            if let Some(binding) = self.bindings.get(root).cloned() {
                self.invalidate_binding(binding);
            }
        } else {
            self.bindings.clear();
        }
    }

    fn invalidate_escaped_modules(&mut self, expression: &Expr) {
        if expression.name_target().is_some() || expression.string_literal().is_some() {
            return;
        }
        let escaped = self
            .bindings
            .iter()
            .filter(|(name, _)| expression_escapes_name(expression, name))
            .map(|(_, binding)| binding.clone())
            .collect::<Vec<_>>();
        for binding in escaped {
            self.invalidate_binding(binding);
        }
    }

    fn invalidate_binding(&mut self, binding: Binding) {
        match binding {
            Binding::Module(module) | Binding::ModuleMember(module, _) => {
                self.invalidate_module(&module);
            }
            Binding::LazyFromImport(lazy) => {
                if let Some(materialized) =
                    self.materialize_binding(Binding::LazyFromImport(lazy.clone()), 0)
                {
                    self.invalidate_binding(materialized);
                } else {
                    for binding in self.bindings.values_mut() {
                        if matches!(binding, Binding::LazyFromImport(bound) if bound == &lazy) {
                            *binding = Binding::Unknown;
                        }
                    }
                }
            }
            Binding::String(_) | Binding::Function(_) | Binding::Unknown => {}
        }
    }

    fn invalidate_module(&mut self, module: &PythonModule) {
        for binding in self.bindings.values_mut() {
            if matches!(binding, Binding::Module(bound) | Binding::ModuleMember(bound, _) if bound == module)
            {
                *binding = Binding::Unknown;
            }
        }
    }

    fn resolve_local_expr(&mut self, expression: &Expr) -> Option<Binding> {
        if let Some(value) = expression.string_literal() {
            return Some(Binding::String(value.to_string()));
        }
        if let Some(name) = expression.name_target() {
            return self.bindings.get(name).cloned();
        }
        let path = expression.path_segments()?;
        let (root, tail) = path.split_first()?;
        let binding = self.bindings.get(root).cloned()?;
        let module = match self.materialize_binding(binding, 0)? {
            Binding::Module(module) => module,
            Binding::ModuleMember(_, _)
            | Binding::LazyFromImport(_)
            | Binding::String(_)
            | Binding::Function(_)
            | Binding::Unknown => return None,
        };
        let value = self.resolve_module_path(&module, tail, 1)?;
        Some(Binding::ModuleMember(
            module,
            Box::new(binding_from_value(value)),
        ))
    }

    fn resolve_expression(&mut self, expression: &Expr, depth: usize) -> Option<ResolvedValue> {
        if depth >= MAX_REEXPORT_DEPTH {
            return None;
        }
        if let Some(value) = expression.string_literal() {
            return Some(ResolvedValue::String(value.to_string()));
        }
        let path = expression.path_segments()?;
        let (root, tail) = path.split_first()?;
        let binding = self.bindings.get(root).cloned()?;
        self.resolve_binding(binding, tail, depth + 1)
    }

    fn resolve_binding(
        &mut self,
        binding: Binding,
        tail: &[String],
        depth: usize,
    ) -> Option<ResolvedValue> {
        match self.materialize_binding(binding, depth)? {
            Binding::Module(module) => {
                if tail.is_empty() {
                    Some(ResolvedValue::Module(module))
                } else {
                    self.resolve_module_path(&module, tail, depth)
                }
            }
            Binding::ModuleMember(_, value) => self.resolve_binding(*value, tail, depth),
            Binding::String(value) if tail.is_empty() => Some(ResolvedValue::String(value)),
            Binding::Function(function) if tail.is_empty() => {
                Some(ResolvedValue::Function(function))
            }
            Binding::LazyFromImport(_)
            | Binding::String(_)
            | Binding::Function(_)
            | Binding::Unknown => None,
        }
    }

    fn materialize_binding(&mut self, binding: Binding, depth: usize) -> Option<Binding> {
        let Binding::LazyFromImport(lazy) = binding else {
            return Some(binding);
        };
        let materialized = self.materialize_lazy_import(&lazy, depth)?;
        for binding in self.bindings.values_mut() {
            if matches!(binding, Binding::LazyFromImport(bound) if bound == &lazy) {
                *binding = materialized.clone();
            }
        }
        Some(materialized)
    }

    fn materialize_lazy_import(&mut self, lazy: &LazyFromImport, depth: usize) -> Option<Binding> {
        let source = self.resolve_import(lazy.level, lazy.module.as_deref())?;
        match self.lookup_member(&source, &lazy.member, depth + 1) {
            MemberLookup::Found(value) => Some(Binding::ModuleMember(
                source,
                Box::new(binding_from_value(value)),
            )),
            MemberLookup::Absent if source.is_package() => self
                .resolve_child(&source, &lazy.member)
                .map(Binding::Module),
            MemberLookup::Absent | MemberLookup::Uncertain => None,
        }
    }

    fn lookup_member(&mut self, module: &PythonModule, member: &str, depth: usize) -> MemberLookup {
        let PythonModule::Source(source) = module else {
            return MemberLookup::Absent;
        };
        if let Some(value) = self.resolve_module_path(module, &[member.to_string()], depth) {
            return MemberLookup::Found(value);
        }
        self.follow(source.file());
        let Ok(Some(parsed)) = RecoveredPythonModule::from_file(self.db, source.file()) else {
            return MemberLookup::Uncertain;
        };
        if parsed.has_ordinary_syntax_errors(self.db) {
            self.recovered_source = true;
            return MemberLookup::Uncertain;
        }
        if member_absence_is_proven(parsed.body(self.db), member) {
            MemberLookup::Absent
        } else {
            MemberLookup::Uncertain
        }
    }

    #[allow(clippy::too_many_lines)]
    fn resolve_module_path(
        &mut self,
        module: &PythonModule,
        path: &[String],
        depth: usize,
    ) -> Option<ResolvedValue> {
        if depth >= MAX_REEXPORT_DEPTH || path.is_empty() {
            return None;
        }
        let PythonModule::Source(source) = module else {
            let child = self.resolve_child(module, &path[0])?;
            return if path.len() == 1 {
                Some(ResolvedValue::Module(child))
            } else {
                self.resolve_module_path(&child, &path[1..], depth + 1)
            };
        };
        self.follow(source.file());
        let parsed = RecoveredPythonModule::from_file(self.db, source.file()).ok()??;
        let parse_is_exact = !parsed.has_ordinary_syntax_errors(self.db);
        self.recovered_source |= !parse_is_exact;
        let root = &path[0];
        let mut value = None;
        let mut enum_bindings = BTreeMap::new();

        for statement in parsed.body(self.db) {
            match statement {
                Stmt::Import(import) => {
                    for clause in DirectImportClause::lower(import) {
                        enum_bindings.remove(clause.bound());
                        if clause.bound() == root {
                            value = if clause.binds_root() && clause.requested() != clause.target()
                            {
                                None
                            } else {
                                self.resolve_import_from(source, 0, Some(clause.target()))
                                    .map(ResolvedValue::Module)
                            };
                        }
                        if clause.bound() == "enum" && clause.target() == "enum" {
                            enum_bindings.insert("enum".to_string(), "module".to_string());
                        }
                    }
                }
                Stmt::ImportFrom(import) => {
                    let syntax = FromImportSyntax::lower(import);
                    if syntax.has_star() {
                        value = None;
                        enum_bindings.clear();
                    }
                    for member in syntax.named_members() {
                        enum_bindings.remove(member.bound());
                        if syntax.level() == 0
                            && syntax.module() == Some("enum")
                            && member.imported() == "Enum"
                        {
                            enum_bindings.insert(member.bound().to_string(), "enum".to_string());
                        }
                        if member.bound() == root {
                            let imported =
                                self.resolve_import_from(source, syntax.level(), syntax.module());
                            value = imported.and_then(|imported| {
                                match self.lookup_member(&imported, member.imported(), depth + 1) {
                                    MemberLookup::Found(value) => Some(value),
                                    MemberLookup::Absent if imported.is_package() => self
                                        .resolve_child(&imported, member.imported())
                                        .map(ResolvedValue::Module),
                                    MemberLookup::Absent | MemberLookup::Uncertain => None,
                                }
                            });
                        }
                    }
                }
                Stmt::Assign(assign) => {
                    let mut complex_target = false;
                    for target in &assign.targets {
                        if let Some(name) = target.name_target() {
                            enum_bindings.remove(name);
                        } else {
                            complex_target = true;
                        }
                    }
                    if complex_target {
                        value = None;
                    } else if assign
                        .targets
                        .iter()
                        .any(|target| target.name_target().is_some_and(|name| name == root))
                    {
                        value = if path.len() == 1 {
                            assign
                                .value
                                .string_literal()
                                .map(|value| ResolvedValue::String(value.to_string()))
                        } else {
                            None
                        };
                    }
                }
                Stmt::AnnAssign(assign) => {
                    if let Some(name) = assign.target.name_target() {
                        enum_bindings.remove(name);
                    }
                    if assign.target.name_target() == Some(root) {
                        value = if path.len() == 1 {
                            assign
                                .value
                                .as_deref()
                                .and_then(ExprExt::string_literal)
                                .map(|value| ResolvedValue::String(value.to_string()))
                        } else {
                            None
                        };
                    }
                }
                Stmt::FunctionDef(function) => {
                    enum_bindings.remove(function.name.as_str());
                    if function.name.as_str() == root {
                        value = if path.len() == 1 && function.decorator_list.is_empty() {
                            Some(ResolvedValue::Function(function_definition(
                                source.file(),
                                function,
                            )))
                        } else {
                            None
                        };
                    }
                }
                Stmt::ClassDef(class) => {
                    enum_bindings.remove(class.name.as_str());
                    if class.name.as_str() == root {
                        value = Self::resolve_enum_member(class, &path[1..], &enum_bindings);
                    }
                }
                Stmt::AugAssign(assign)
                    if assign.target.name_target() == Some(root)
                        || assign.target.name_target().is_none() =>
                {
                    value = None;
                }
                Stmt::Delete(delete)
                    if delete.targets.iter().any(|target| {
                        target.name_target() == Some(root) || target.name_target().is_none()
                    }) =>
                {
                    value = None;
                }
                Stmt::For(_)
                | Stmt::While(_)
                | Stmt::If(_)
                | Stmt::With(_)
                | Stmt::Match(_)
                | Stmt::Try(_)
                | Stmt::TypeAlias(_)
                | Stmt::Return(_)
                | Stmt::Raise(_)
                | Stmt::Assert(_) => value = None,
                Stmt::Expr(expression) if expression.value.string_literal().is_none() => {
                    value = None;
                }
                Stmt::AugAssign(_)
                | Stmt::Delete(_)
                | Stmt::Expr(_)
                | Stmt::Global(_)
                | Stmt::Nonlocal(_)
                | Stmt::Pass(_)
                | Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::IpyEscapeCommand(_) => {}
            }
        }

        match value? {
            ResolvedValue::Module(module) if path.len() > 1 => {
                self.resolve_module_path(&module, &path[1..], depth + 1)
            }
            resolved @ ResolvedValue::String(_) => Some(resolved),
            resolved @ (ResolvedValue::Module(_) | ResolvedValue::Function(_))
                if path.len() == 1 =>
            {
                Some(resolved)
            }
            ResolvedValue::Module(_) | ResolvedValue::Function(_) => None,
        }
    }

    fn resolve_enum_member(
        class: &StmtClassDef,
        path: &[String],
        enum_bindings: &BTreeMap<String, String>,
    ) -> Option<ResolvedValue> {
        let [member, value] = path else {
            return None;
        };
        let arguments = class.arguments.as_deref()?;
        let [base] = arguments.args.as_ref() else {
            return None;
        };
        if value != "value"
            || !class.decorator_list.is_empty()
            || !arguments.keywords.is_empty()
            || !base.path_segments().is_some_and(|segments| {
                matches!(segments.as_slice(), [module, enum_name]
                    if enum_bindings.get(module).is_some_and(|kind| kind == "module")
                        && enum_name == "Enum")
                    || matches!(segments.as_slice(), [enum_name]
                        if enum_bindings.get(enum_name).is_some_and(|kind| kind == "enum"))
            })
        {
            return None;
        }

        let mut result = None;
        for statement in &class.body {
            match statement {
                Stmt::Assign(assign) => {
                    let [target] = assign.targets.as_slice() else {
                        return None;
                    };
                    let name = target.name_target()?;
                    let literal = assign.value.string_literal()?;
                    if name == member {
                        result = Some(literal.to_string());
                    }
                }
                Stmt::AnnAssign(assign) => {
                    let name = assign.target.name_target()?;
                    let literal = assign.value.as_deref().and_then(ExprExt::string_literal)?;
                    if name == member {
                        result = Some(literal.to_string());
                    }
                }
                Stmt::Expr(expression) if expression.value.string_literal().is_some() => {}
                Stmt::Pass(_) => {}
                Stmt::Import(_)
                | Stmt::ImportFrom(_)
                | Stmt::FunctionDef(_)
                | Stmt::ClassDef(_)
                | Stmt::AugAssign(_)
                | Stmt::Delete(_)
                | Stmt::For(_)
                | Stmt::While(_)
                | Stmt::If(_)
                | Stmt::With(_)
                | Stmt::Match(_)
                | Stmt::Try(_)
                | Stmt::TypeAlias(_)
                | Stmt::Expr(_)
                | Stmt::Return(_)
                | Stmt::Raise(_)
                | Stmt::Assert(_)
                | Stmt::Global(_)
                | Stmt::Nonlocal(_)
                | Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::IpyEscapeCommand(_) => return None,
            }
        }
        result.map(ResolvedValue::String)
    }

    fn resolve_import(&mut self, level: u32, module: Option<&str>) -> Option<PythonModule> {
        let importer = self.importer.clone()?;
        self.resolve_import_from(&importer, level, module)
    }

    fn resolve_import_from(
        &mut self,
        importer: &PythonSourceModule,
        level: u32,
        module: Option<&str>,
    ) -> Option<PythonModule> {
        let (_, resolution) = PythonSourceModule::resolve_import_chain(
            self.db,
            self.project?,
            PythonImportRequest {
                level,
                module,
                importer,
            },
        )
        .ok()?;
        let chain = match resolution {
            PythonImportChainResolution::Resolved(chain) => chain,
            PythonImportChainResolution::Failed { prefix, .. } => {
                self.follow_chain(prefix.into_components());
                return None;
            }
        };
        let components = chain.into_components();
        let module = components.last().cloned();
        self.follow_chain(components);
        module
    }

    fn resolve_child(&mut self, module: &PythonModule, member: &str) -> Option<PythonModule> {
        let name =
            PythonModuleName::parse(&format!("{}.{}", module.name().as_str(), member)).ok()?;
        let name_string = name.as_str().to_string();
        let importer = self.importer.clone()?;
        self.resolve_import_from(&importer, 0, Some(&name_string))
    }

    fn follow_chain(&mut self, components: Vec<PythonModule>) {
        for component in components {
            if let PythonModule::Source(source) = component {
                self.follow(source.file());
            }
        }
    }

    fn follow(&mut self, file: File) {
        if file != self.file && !self.consulted_files.contains(&file) {
            self.consulted_files.push(file);
        }
    }
}

struct ExpressionFinder<'a> {
    target: Span,
    found: Option<&'a Expr>,
}

impl<'a> Visitor<'a> for ExpressionFinder<'a> {
    fn visit_expr(&mut self, expression: &'a Expr) {
        if expression.span() == self.target {
            self.found = Some(expression);
            return;
        }
        visitor::walk_expr(self, expression);
    }
}

fn find_expression(statement: &Stmt, target: Span) -> Option<&Expr> {
    let mut finder = ExpressionFinder {
        target,
        found: None,
    };
    finder.visit_stmt(statement);
    finder.found
}

fn span_contains(outer: Span, inner: Span) -> bool {
    outer.start() <= inner.start() && inner.end() <= outer.end()
}

fn occurrence_value(value: ResolvedValue) -> Option<PythonOccurrenceValue> {
    match value {
        ResolvedValue::String(value) => Some(PythonOccurrenceValue::String(value)),
        ResolvedValue::Function(function) => Some(PythonOccurrenceValue::Function(function)),
        ResolvedValue::Module(_) => None,
    }
}

struct NamedBindingVisitor {
    found: bool,
}

impl<'a> Visitor<'a> for NamedBindingVisitor {
    fn visit_expr(&mut self, expression: &'a Expr) {
        if matches!(expression, Expr::Named(_)) {
            self.found = true;
            return;
        }
        visitor::walk_expr(self, expression);
    }
}

fn statement_contains_named_binding(statement: &Stmt) -> bool {
    let mut visitor = NamedBindingVisitor { found: false };
    visitor.visit_stmt(statement);
    visitor.found
}

fn binding_from_value(value: ResolvedValue) -> Binding {
    match value {
        ResolvedValue::Module(module) => Binding::Module(module),
        ResolvedValue::String(value) => Binding::String(value),
        ResolvedValue::Function(function) => Binding::Function(function),
    }
}

fn function_definition(file: File, function: &StmtFunctionDef) -> PythonFunctionDefinition {
    PythonFunctionDefinition {
        file,
        definition_span: function.span(),
        name: function.name.to_string(),
    }
}

fn expression_escapes_name(expression: &Expr, name: &str) -> bool {
    let mut visitor = ModuleEscapeVisitor { name, found: false };
    visitor.visit_expr(expression);
    visitor.found
}

struct ModuleEscapeVisitor<'a> {
    name: &'a str,
    found: bool,
}

impl<'a> Visitor<'a> for ModuleEscapeVisitor<'_> {
    fn visit_expr(&mut self, expression: &'a Expr) {
        if expression.name_target() == Some(self.name) {
            self.found = true;
            return;
        }
        if let Expr::Attribute(attribute) = expression
            && attribute
                .value
                .path_segments()
                .is_some_and(|path| path.first().is_some_and(|root| root == self.name))
        {
            return;
        }
        if let Expr::Call(call) = expression
            && (mutation_root(&call.func) == Some(self.name)
                || expression_contains_module_member(&call.func, self.name))
        {
            self.found = true;
            return;
        }
        visitor::walk_expr(self, expression);
    }
}

fn expression_contains_module_member(expression: &Expr, name: &str) -> bool {
    struct ModuleMemberVisitor<'a> {
        name: &'a str,
        found: bool,
    }

    impl<'a> Visitor<'a> for ModuleMemberVisitor<'a> {
        fn visit_expr(&mut self, expression: &'a Expr) {
            if let Expr::Attribute(attribute) = expression
                && mutation_root(&attribute.value) == Some(self.name)
            {
                self.found = true;
                return;
            }
            visitor::walk_expr(self, expression);
        }
    }

    let mut visitor = ModuleMemberVisitor { name, found: false };
    visitor.visit_expr(expression);
    visitor.found
}

fn mutation_root(expression: &Expr) -> Option<&str> {
    match expression {
        Expr::Name(_) => expression.name_target(),
        Expr::Attribute(attribute) => mutation_root(&attribute.value),
        Expr::Subscript(subscript) => mutation_root(&subscript.value),
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
        | Expr::List(_)
        | Expr::Tuple(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => None,
    }
}

fn member_absence_is_proven(body: &[Stmt], member: &str) -> bool {
    body.iter().all(|statement| match statement {
        Stmt::Import(import) => DirectImportClause::lower(import)
            .iter()
            .all(|clause| clause.bound() != member),
        Stmt::ImportFrom(import) => {
            let syntax = FromImportSyntax::lower(import);
            !syntax.has_star()
                && syntax
                    .named_members()
                    .iter()
                    .all(|clause| clause.bound() != member)
        }
        Stmt::Assign(assign) => assign
            .targets
            .iter()
            .all(|target| target.name_target() != Some(member)),
        Stmt::AnnAssign(assign) => assign.target.name_target() != Some(member),
        Stmt::AugAssign(assign) => assign.target.name_target() != Some(member),
        Stmt::Delete(delete) => delete
            .targets
            .iter()
            .all(|target| target.name_target() != Some(member)),
        Stmt::FunctionDef(function) => {
            function.name.as_str() != "__getattr__" && function.name.as_str() != member
        }
        Stmt::ClassDef(class) => class.name.as_str() != member,
        Stmt::Expr(expression) => expression.value.string_literal().is_some(),
        Stmt::Pass(_) => true,
        Stmt::For(_)
        | Stmt::While(_)
        | Stmt::If(_)
        | Stmt::With(_)
        | Stmt::Match(_)
        | Stmt::Try(_)
        | Stmt::TypeAlias(_)
        | Stmt::Return(_)
        | Stmt::Raise(_)
        | Stmt::Assert(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::IpyEscapeCommand(_) => false,
    })
}
