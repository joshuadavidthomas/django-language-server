use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;
use ruff_python_parser::parse_module;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use serde::Serialize;

use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::python::ImportSourceResolution;
use crate::python::PythonImport;
use crate::python::PythonImportResolver;
use crate::python::PythonPathBindings;
use crate::python::PythonSource;
use crate::python::Truthiness;
use crate::python::evaluate_path;
use crate::python::pattern_bound_names;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ParseStatus {
    #[default]
    Parsed,
    Unparseable,
}

impl ParseStatus {
    const fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unparseable, _) | (_, Self::Unparseable) => Self::Unparseable,
            (Self::Parsed, Self::Parsed) => Self::Parsed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonSemanticModel {
    bindings: PythonBindings,
    files_read: Vec<File>,
    source_paths: FxHashMap<File, Utf8PathBuf>,
    mutations: Vec<PythonMutation>,
    status: ParseStatus,
}

impl PythonSemanticModel {
    pub(crate) fn analyze(source: &PythonSource, resolver: &mut dyn PythonImportResolver) -> Self {
        PythonSemanticModelAnalysis::default().analyze_source(source, resolver)
    }

    pub(crate) fn binding(&self, name: &str) -> Option<&PythonBinding> {
        self.bindings.get(name)
    }

    pub(crate) fn files_read(&self) -> &[File] {
        &self.files_read
    }

    pub(crate) fn mutations(&self) -> &[PythonMutation] {
        &self.mutations
    }

    pub(crate) fn source_path(&self, file: File) -> Option<&Utf8Path> {
        self.source_paths.get(&file).map(Utf8PathBuf::as_path)
    }

    pub(crate) fn parse_status(&self) -> ParseStatus {
        self.status
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonMutation {
    root: String,
    access: Vec<PythonMutationAccess>,
    method: String,
}

impl PythonMutation {
    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn access(&self) -> &[PythonMutationAccess] {
        &self.access
    }

    pub(crate) fn method(&self) -> &str {
        &self.method
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonMutationAccess {
    Index(usize),
    Key(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBinding {
    name: String,
    values: Vec<PythonBoundValue>,
    completeness: PythonCompleteness,
}

impl PythonBinding {
    fn unknown(name: &str, origin: Origin) -> Self {
        Self {
            name: name.to_string(),
            values: vec![PythonBoundValue::unknown(origin)],
            completeness: PythonCompleteness::Partial,
        }
    }

    fn full(name: &str, value: PythonValue, binding_origin: Origin) -> Self {
        let completeness = value.completeness();
        Self {
            name: name.to_string(),
            values: vec![PythonBoundValue {
                value,
                binding_origin,
            }],
            completeness,
        }
    }

    pub(crate) fn values(&self) -> &[PythonBoundValue] {
        &self.values
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.completeness == PythonCompleteness::Full
            && self.values.iter().all(PythonBoundValue::is_complete)
    }

    fn mark_partial(&mut self) {
        self.completeness = PythonCompleteness::Partial;
    }

    fn joined(name: &str, bindings: impl IntoIterator<Item = Option<Self>>) -> Option<Self> {
        let mut values: Vec<PythonBoundValue> = Vec::new();
        let mut completeness = PythonCompleteness::Full;
        let mut saw_binding = false;

        for binding in bindings {
            let Some(binding) = binding else {
                completeness = PythonCompleteness::Partial;
                continue;
            };
            saw_binding = true;
            if !binding.is_complete() {
                completeness = PythonCompleteness::Partial;
            }
            for value in binding.values {
                if !values
                    .iter()
                    .any(|existing| existing.semantically_eq(&value))
                {
                    values.push(value);
                }
            }
        }

        if !saw_binding {
            return None;
        }

        if values.len() != 1 {
            completeness = PythonCompleteness::Partial;
        }

        Some(Self {
            name: name.to_string(),
            values,
            completeness,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonBoundValue {
    value: PythonValue,
    binding_origin: Origin,
}

impl PythonBoundValue {
    fn unknown(origin: Origin) -> Self {
        Self {
            value: PythonValue::unknown(origin),
            binding_origin: origin,
        }
    }

    pub(crate) fn value(&self) -> &PythonValue {
        &self.value
    }

    pub(crate) fn value_origin(&self) -> Origin {
        self.value.origin()
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.value.is_complete()
    }

    fn semantically_eq(&self, other: &Self) -> bool {
        self.value.semantically_eq(&other.value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonValue {
    kind: PythonValueKind,
    origin: Origin,
    completeness: PythonCompleteness,
}

impl PythonValue {
    fn new(kind: PythonValueKind, origin: Origin, completeness: PythonCompleteness) -> Self {
        Self {
            kind,
            origin,
            completeness,
        }
    }

    fn full(kind: PythonValueKind, origin: Origin) -> Self {
        Self::new(kind, origin, PythonCompleteness::Full)
    }

    fn partial(kind: PythonValueKind, origin: Origin) -> Self {
        Self::new(kind, origin, PythonCompleteness::Partial)
    }

    fn unknown(origin: Origin) -> Self {
        Self::partial(PythonValueKind::Unknown, origin)
    }

    pub(crate) fn kind(&self) -> &PythonValueKind {
        &self.kind
    }

    pub(crate) fn origin(&self) -> Origin {
        self.origin
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.completeness == PythonCompleteness::Full
    }

    pub(crate) fn completeness(&self) -> PythonCompleteness {
        self.completeness
    }

    fn mark_partial(&mut self) {
        self.completeness = PythonCompleteness::Partial;
    }

    fn semantically_eq(&self, other: &Self) -> bool {
        self.completeness == other.completeness
            && match (&self.kind, &other.kind) {
                (PythonValueKind::Str(left), PythonValueKind::Str(right)) => {
                    left == right && self.origin.file == other.origin.file
                }
                _ => self.kind.semantically_eq(&other.kind),
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonValueKind {
    Str(String),
    Bool(bool),
    List(Vec<PythonValue>),
    Dict(PythonDict),
    Path(Utf8PathBuf),
    Unknown,
}

impl PythonValueKind {
    fn semantically_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Str(left), Self::Str(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::List(left), Self::List(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| left.semantically_eq(right))
            }
            (Self::Dict(left), Self::Dict(right)) => left.semantically_eq(right),
            (Self::Path(left), Self::Path(right)) => left == right,
            (Self::Unknown, Self::Unknown) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonDict {
    entries: Vec<PythonDictEntry>,
}

impl PythonDict {
    pub(crate) fn entries(&self) -> &[PythonDictEntry] {
        &self.entries
    }

    fn semantically_eq(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(left, right)| left.semantically_eq(right))
    }

    fn get_string_key_mut(&mut self, key: &str) -> Option<&mut PythonValue> {
        self.entries.iter_mut().find_map(|entry| {
            if matches!(entry.key.kind(), PythonValueKind::Str(candidate) if candidate == key) {
                Some(&mut entry.value)
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonDictEntry {
    key: PythonValue,
    value: PythonValue,
}

impl PythonDictEntry {
    pub(crate) fn key(&self) -> &PythonValue {
        &self.key
    }

    pub(crate) fn value(&self) -> &PythonValue {
        &self.value
    }

    fn semantically_eq(&self, other: &Self) -> bool {
        self.key.semantically_eq(&other.key) && self.value.semantically_eq(&other.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PythonCompleteness {
    Full,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PythonSemanticState {
    bindings: PythonBindings,
    mutations: Vec<PythonMutation>,
    effects: PythonSemanticEffects,
}

impl PythonSemanticState {
    fn degrade_names(&mut self, names: impl IntoIterator<Item = String>, origin: Origin) {
        for name in names {
            if let Some(binding) = self.bindings.by_name.get_mut(&name) {
                binding.mark_partial();
            } else {
                self.bindings
                    .bind(&name, PythonBinding::unknown(&name, origin));
            }
        }
    }

    pub(super) fn merge_only_effects_from_state(&mut self, other: Self) {
        self.effects.merge(other.effects);
    }

    pub(super) fn changed_writes_from(base: &Self, changed: &Self) -> TouchedNames {
        let mut writes = TouchedNames::default();
        for (name, binding) in &changed.bindings.by_name {
            if base.bindings.get(name) != Some(binding) {
                writes.record(name);
            }
        }
        for name in base.bindings.by_name.keys() {
            if !changed.bindings.by_name.contains_key(name) {
                writes.record(name);
            }
        }
        for mutation in &changed.mutations {
            if !base.mutations.contains(mutation) {
                writes.record(&mutation.root);
            }
        }
        writes
    }

    pub(super) fn join_branches(mut base: Self, branches: &[Self], writes: &TouchedNames) -> Self {
        let mut names = writes.names.clone();
        for branch in branches {
            for (name, binding) in &branch.bindings.by_name {
                if base.bindings.get(name) != Some(binding) {
                    names.insert(name.clone());
                }
            }
        }

        for name in &names {
            let branch_values = branches
                .iter()
                .map(|branch| branch.bindings.get(name).cloned());
            if let Some(binding) = PythonBinding::joined(name, branch_values) {
                base.bindings.bind(name, binding);
            } else {
                base.bindings.remove(name);
            }
        }

        base.mutations.clear();
        base.effects = PythonSemanticEffects::default();
        for branch in branches {
            for mutation in &branch.mutations {
                if !base.mutations.contains(mutation) {
                    base.mutations.push(mutation.clone());
                }
            }
            base.effects.merge(branch.effects.clone());
        }

        base
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct PythonSemanticEffects {
    imported_models: Vec<PythonSemanticModel>,
    read_failures: Vec<(File, Utf8PathBuf)>,
}

impl PythonSemanticEffects {
    fn add_imported_model(&mut self, imported_model: PythonSemanticModel) {
        let Some(root_file) = imported_model.files_read.first().copied() else {
            return;
        };
        if !self
            .imported_models
            .iter()
            .any(|existing| existing.files_read.first().copied() == Some(root_file))
        {
            self.imported_models.push(imported_model);
        }
    }

    fn add_read_failure(&mut self, file: File, path: Utf8PathBuf) {
        if !self
            .read_failures
            .iter()
            .any(|(existing_file, existing_path)| *existing_file == file && *existing_path == path)
        {
            self.read_failures.push((file, path));
        }
    }

    fn merge(&mut self, other: Self) {
        for imported_model in other.imported_models {
            self.add_imported_model(imported_model);
        }
        for (file, path) in other.read_failures {
            self.add_read_failure(file, path);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct PythonBindings {
    by_name: FxHashMap<String, PythonBinding>,
}

impl PythonBindings {
    fn get(&self, name: &str) -> Option<&PythonBinding> {
        self.by_name.get(name)
    }

    fn bind(&mut self, name: &str, binding: PythonBinding) {
        self.by_name.insert(name.to_string(), binding);
    }

    fn remove(&mut self, name: &str) {
        self.by_name.remove(name);
    }

    fn mark_all_partial(&mut self) {
        for binding in self.by_name.values_mut() {
            binding.mark_partial();
        }
    }

    fn merge_star_import(&mut self, imported: &PythonSemanticModel) {
        self.by_name.extend(imported.bindings.by_name.clone());
    }

    fn path_bindings(&self) -> PythonPathBindings {
        let mut paths = PythonPathBindings::default();
        for (name, binding) in &self.by_name {
            let [bound] = binding.values.as_slice() else {
                continue;
            };
            let PythonValueKind::Path(path) = bound.value.kind() else {
                continue;
            };
            if bound.is_complete() && binding.is_complete() {
                paths.set(name.clone(), path.clone());
            }
        }
        paths
    }

    fn bool_value(&self, name: &str) -> Option<bool> {
        let binding = self.by_name.get(name)?;
        if !binding.is_complete() {
            return None;
        }
        let [bound] = binding.values.as_slice() else {
            return None;
        };
        match bound.value.kind() {
            PythonValueKind::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
struct PythonSemanticModelAnalysis {
    active: FxHashSet<Utf8PathBuf>,
    cache: FxHashMap<Utf8PathBuf, PythonSemanticModel>,
}

impl PythonSemanticModelAnalysis {
    fn analyze_source(
        &mut self,
        source: &PythonSource,
        resolver: &mut dyn PythonImportResolver,
    ) -> PythonSemanticModel {
        let path = source.path().to_path_buf();
        if let Some(cached) = self.cache.get(&path) {
            return cached.clone();
        }
        if !self.active.insert(path.clone()) {
            return PythonSemanticModel {
                bindings: PythonBindings::default(),
                files_read: Vec::new(),
                source_paths: FxHashMap::default(),
                mutations: Vec::new(),
                status: ParseStatus::Parsed,
            };
        }

        let parsed = parse_module(source.source());
        let status = if parsed.is_ok() {
            ParseStatus::Parsed
        } else {
            ParseStatus::Unparseable
        };
        let mut evaluator = PythonSemanticEvaluator::new(source, self, resolver);
        let mut state = PythonSemanticState::default();
        if let Ok(parsed) = parsed {
            let module = parsed.into_syntax();
            state = evaluator.walk_body(state, &module.body);
        }

        let model = finish_model(source, status, state);
        self.active.remove(&path);
        self.cache.insert(path, model.clone());
        model
    }
}

fn finish_model(
    source: &PythonSource,
    current_status: ParseStatus,
    state: PythonSemanticState,
) -> PythonSemanticModel {
    let mut files_read = vec![source.file()];
    let mut source_paths = FxHashMap::from_iter([(source.file(), source.path().to_path_buf())]);
    let mut status = current_status;

    for (file, path) in state.effects.read_failures {
        files_read.push(file);
        source_paths.insert(file, path);
    }

    for imported_model in state.effects.imported_models {
        files_read.extend(imported_model.files_read.clone());
        source_paths.extend(imported_model.source_paths.clone());
        status = status.join(imported_model.status);
    }

    PythonSemanticModel {
        bindings: state.bindings,
        files_read,
        source_paths,
        mutations: state.mutations,
        status,
    }
}

pub(super) struct PythonSemanticEvaluator<'a> {
    source: &'a PythonSource,
    analysis: &'a mut PythonSemanticModelAnalysis,
    resolver: &'a mut dyn PythonImportResolver,
}

impl<'a> PythonSemanticEvaluator<'a> {
    fn new(
        source: &'a PythonSource,
        analysis: &'a mut PythonSemanticModelAnalysis,
        resolver: &'a mut dyn PythonImportResolver,
    ) -> Self {
        Self {
            source,
            analysis,
            resolver,
        }
    }

    fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.source.file(), ranged.span())
    }

    pub(super) fn walk_body(
        &mut self,
        mut state: PythonSemanticState,
        body: &[ast::Stmt],
    ) -> PythonSemanticState {
        for stmt in body {
            state = self.walk_stmt(state, stmt);
        }
        state
    }

    fn walk_stmt(
        &mut self,
        mut state: PythonSemanticState,
        stmt: &ast::Stmt,
    ) -> PythonSemanticState {
        match stmt {
            ast::Stmt::Assign(assign) => self.walk_assign(&mut state, assign),
            ast::Stmt::AnnAssign(assign) => self.walk_ann_assign(&mut state, assign),
            ast::Stmt::AugAssign(assign) => self.walk_aug_assign(&mut state, assign),
            ast::Stmt::Expr(expr) => self.walk_expr(&mut state, &expr.value),
            ast::Stmt::Import(import) => self.walk_import(&mut state, &import.names),
            ast::Stmt::ImportFrom(import) => self.walk_import_from(&mut state, import),
            ast::Stmt::If(stmt_if) => {
                return crate::python::branches::walk_if(self, state, stmt_if);
            }
            ast::Stmt::For(stmt_for) => {
                self.bind_unknown_targets(&mut state, &stmt_for.target);
                return crate::python::branches::degrade_loop_bodies(
                    self,
                    state,
                    &[&stmt_for.body, &stmt_for.orelse],
                );
            }
            ast::Stmt::While(stmt_while) => {
                return match Self::evaluate_test(&state, &stmt_while.test) {
                    Truthiness::AlwaysFalse => self.walk_body(state, &stmt_while.orelse),
                    Truthiness::AlwaysTrue | Truthiness::Ambiguous => {
                        crate::python::branches::degrade_loop_bodies(
                            self,
                            state,
                            &[&stmt_while.body, &stmt_while.orelse],
                        )
                    }
                };
            }
            ast::Stmt::With(stmt_with) => {
                for item in &stmt_with.items {
                    if let Some(optional_vars) = &item.optional_vars {
                        self.bind_unknown_targets(&mut state, optional_vars);
                    }
                }
                return self.walk_body(state, &stmt_with.body);
            }
            ast::Stmt::Try(stmt_try) => {
                return crate::python::branches::walk_try(self, state, stmt_try);
            }
            ast::Stmt::FunctionDef(function) => {
                Self::bind_unknown_name(&mut state, function.name.as_str(), self.origin(function));
            }
            ast::Stmt::ClassDef(class) => {
                Self::bind_unknown_name(&mut state, class.name.as_str(), self.origin(class));
            }
            ast::Stmt::Delete(delete) => {
                for target in &delete.targets {
                    self.bind_unknown_targets(&mut state, target);
                }
            }
            ast::Stmt::TypeAlias(type_alias) => {
                self.bind_unknown_targets(&mut state, &type_alias.name);
            }
            ast::Stmt::Match(stmt_match) => {
                return crate::python::branches::walk_match(self, state, stmt_match);
            }
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

    fn walk_assign(&mut self, state: &mut PythonSemanticState, assign: &ast::StmtAssign) {
        if assign.targets.len() != 1 {
            for target in &assign.targets {
                self.bind_unknown_targets(state, target);
            }
            return;
        }

        self.assign_target(state, &assign.targets[0], &assign.value);
    }

    fn walk_ann_assign(&mut self, state: &mut PythonSemanticState, assign: &ast::StmtAnnAssign) {
        let Some(value) = &assign.value else {
            self.bind_unknown_targets(state, &assign.target);
            return;
        };
        self.assign_target(state, &assign.target, value);
    }

    fn assign_target(
        &mut self,
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        value: &ast::Expr,
    ) {
        if let Some(name) = target.name_target() {
            self.assign_name(state, name, value);
        } else {
            self.bind_unknown_targets(state, target);
        }
    }

    fn assign_name(&mut self, state: &mut PythonSemanticState, name: &str, value: &ast::Expr) {
        let binding_origin = self.origin(value);
        state.mutations.retain(|mutation| mutation.root != name);

        if let Some(source_name) = value.name_target()
            && let Some(binding) = state.bindings.get(source_name).cloned()
        {
            let mut imported_binding = binding;
            imported_binding.name = name.to_string();
            for bound in &mut imported_binding.values {
                bound.binding_origin = binding_origin;
            }
            state.bindings.bind(name, imported_binding);
            return;
        }

        let value = self.evaluate_value(state, value);
        state
            .bindings
            .bind(name, PythonBinding::full(name, value, binding_origin));
    }

    fn walk_aug_assign(&mut self, state: &mut PythonSemanticState, assign: &ast::StmtAugAssign) {
        if assign.op != ast::Operator::Add {
            self.bind_unknown_targets(state, &assign.target);
            return;
        }

        if !self.apply_extend_mutation(state, &assign.target, &assign.value, self.origin(assign)) {
            self.bind_unknown_targets(state, &assign.target);
        }
    }

    fn walk_expr(&mut self, state: &mut PythonSemanticState, expr: &ast::Expr) {
        let ast::Expr::Call(call) = expr else {
            self.record_unsupported_touches(state, expr_read_names(expr));
            return;
        };
        let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
            self.record_unsupported_touches(state, expr_read_names(expr));
            return;
        };

        Self::record_mutation(state, &attribute.value, attribute.attr.as_str());

        match attribute.attr.as_str() {
            "append" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
                if !self.apply_append_mutation(
                    state,
                    &attribute.value,
                    &call.arguments.args[0],
                    self.origin(call),
                ) {
                    self.record_unsupported_touches(state, expr_read_names(expr));
                }
            }
            "extend" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
                if !self.apply_extend_mutation(
                    state,
                    &attribute.value,
                    &call.arguments.args[0],
                    self.origin(call),
                ) {
                    self.record_unsupported_touches(state, expr_read_names(expr));
                }
            }
            "insert" if call.arguments.args.len() == 2 && call.arguments.keywords.is_empty() => {
                if !self.apply_insert_mutation(
                    state,
                    &attribute.value,
                    &call.arguments.args[0],
                    &call.arguments.args[1],
                    self.origin(call),
                ) {
                    self.record_unsupported_touches(state, expr_read_names(expr));
                }
            }
            "remove" if call.arguments.args.len() == 1 && call.arguments.keywords.is_empty() => {
                if !Self::apply_remove_mutation(state, &attribute.value, &call.arguments.args[0]) {
                    self.record_unsupported_touches(state, expr_read_names(expr));
                }
            }
            _ => self.record_unsupported_touches(state, expr_read_names(expr)),
        }
    }

    fn walk_import(&mut self, state: &mut PythonSemanticState, aliases: &[ast::Alias]) {
        for alias in aliases {
            let bound_name = alias.asname.as_ref().map_or_else(
                || first_import_segment(alias.name.as_str()),
                |asname| asname.as_str(),
            );
            Self::bind_unknown_name(state, bound_name, self.origin(alias));
        }
    }

    fn analyze_imported_model(
        &mut self,
        state: &mut PythonSemanticState,
        imported_source: &PythonSource,
    ) -> PythonSemanticModel {
        let imported_model = self.analysis.analyze_source(imported_source, self.resolver);
        state.mutations.extend(imported_model.mutations.clone());
        state.effects.add_imported_model(imported_model.clone());
        imported_model
    }

    fn walk_import_from(&mut self, state: &mut PythonSemanticState, import: &ast::StmtImportFrom) {
        let python_import = PythonImport {
            level: import.level,
            module: import
                .module
                .as_ref()
                .map(ruff_python_ast::Identifier::as_str),
            importer: self.source.path(),
        };
        let is_star_import = import.names.iter().any(|alias| alias.name.as_str() == "*");
        if is_star_import {
            match self.resolver.resolve_star_import(python_import) {
                ImportSourceResolution::Resolved(imported_source) => {
                    let imported_model = self.analyze_imported_model(state, &imported_source);
                    state.bindings.merge_star_import(&imported_model);
                }
                ImportSourceResolution::ReadFailed { file, path } => {
                    state.effects.add_read_failure(file, path);
                    state.bindings.mark_all_partial();
                }
                ImportSourceResolution::Unresolved | ImportSourceResolution::SkippedExternal => {
                    state.bindings.mark_all_partial();
                }
            }
            return;
        }

        match self.resolver.resolve_named_import(python_import) {
            ImportSourceResolution::Resolved(imported_source) => {
                let imported_model = self.analyze_imported_model(state, &imported_source);
                for alias in &import.names {
                    let imported_name = alias.name.as_str();
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| imported_name, |asname| asname.as_str());
                    if let Some(binding) = imported_model.binding(imported_name).cloned() {
                        state.bindings.bind(
                            bound_name,
                            PythonBinding {
                                name: bound_name.to_string(),
                                values: binding.values,
                                completeness: binding.completeness,
                            },
                        );
                    } else {
                        Self::bind_unknown_name(state, bound_name, self.origin(alias));
                    }
                }
            }
            ImportSourceResolution::ReadFailed { file, path } => {
                state.effects.add_read_failure(file, path);
                for alias in &import.names {
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                    Self::bind_unknown_name(state, bound_name, self.origin(alias));
                }
            }
            ImportSourceResolution::Unresolved | ImportSourceResolution::SkippedExternal => {
                for alias in &import.names {
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                    Self::bind_unknown_name(state, bound_name, self.origin(alias));
                }
            }
        }
    }

    fn evaluate_test_expr(state: &PythonSemanticState, expr: &ast::Expr) -> Truthiness {
        if let Some(name) = expr.name_target() {
            return state
                .bindings
                .bool_value(name)
                .map_or(Truthiness::Ambiguous, Truthiness::from_bool);
        }

        match expr {
            ast::Expr::BooleanLiteral(literal) => Truthiness::from_bool(literal.value),
            ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => {
                Self::evaluate_test_expr(state, &unary.operand).negate()
            }
            _ => Truthiness::Ambiguous,
        }
    }

    fn evaluate_value(&self, state: &PythonSemanticState, expr: &ast::Expr) -> PythonValue {
        let origin = self.origin(expr);
        let path = evaluate_path(expr, self.source.path(), &state.bindings.path_bindings());

        if let Some(name) = expr.name_target()
            && let Some(binding) = state.bindings.get(name)
            && let [bound] = binding.values()
        {
            let mut value = bound.value().clone();
            if !binding.is_complete() || !bound.is_complete() {
                value.mark_partial();
            }
            return value;
        }

        if let Some(value) = expr.string_literal() {
            return PythonValue::full(PythonValueKind::Str(value.to_string()), origin);
        }
        if let Some(value) = expr.bool_literal() {
            return PythonValue::full(PythonValueKind::Bool(value), origin);
        }
        if let Some(path) = path {
            return PythonValue::full(PythonValueKind::Path(path), origin);
        }

        match expr {
            ast::Expr::List(list) => self.evaluate_list(state, &list.elts, origin),
            ast::Expr::Tuple(tuple) => self.evaluate_list(state, &tuple.elts, origin),
            ast::Expr::Dict(dict) => self.evaluate_dict(state, dict, origin),
            ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
                self.evaluate_addition(state, &bin_op.left, &bin_op.right, origin)
            }
            _ => PythonValue::unknown(origin),
        }
    }

    fn evaluate_list(
        &self,
        state: &PythonSemanticState,
        elements: &[ast::Expr],
        origin: Origin,
    ) -> PythonValue {
        let mut values = Vec::new();
        let mut completeness = PythonCompleteness::Full;
        for element in elements {
            if let ast::Expr::Starred(starred) = element {
                let starred = self.evaluate_value(state, &starred.value);
                match starred.kind() {
                    PythonValueKind::List(items) => values.extend(items.clone()),
                    _ => completeness = PythonCompleteness::Partial,
                }
                if !starred.is_complete() {
                    completeness = PythonCompleteness::Partial;
                }
                continue;
            }

            let value = self.evaluate_value(state, element);
            if !value.is_complete() {
                completeness = PythonCompleteness::Partial;
            }
            values.push(value);
        }
        PythonValue::new(PythonValueKind::List(values), origin, completeness)
    }

    fn evaluate_addition(
        &self,
        state: &PythonSemanticState,
        left: &ast::Expr,
        right: &ast::Expr,
        origin: Origin,
    ) -> PythonValue {
        let left = self.evaluate_value(state, left);
        let right = self.evaluate_value(state, right);
        let operands_complete = left.is_complete() && right.is_complete();
        match (left.kind, right.kind) {
            (PythonValueKind::List(mut left), PythonValueKind::List(right)) => {
                left.extend(right);
                let completeness = if operands_complete && left.iter().all(PythonValue::is_complete)
                {
                    PythonCompleteness::Full
                } else {
                    PythonCompleteness::Partial
                };
                PythonValue::new(PythonValueKind::List(left), origin, completeness)
            }
            _ => PythonValue::unknown(origin),
        }
    }

    fn evaluate_dict(
        &self,
        state: &PythonSemanticState,
        dict: &ast::ExprDict,
        origin: Origin,
    ) -> PythonValue {
        let mut entries: Vec<PythonDictEntry> = Vec::new();
        let mut seen_string_keys = FxHashSet::default();
        let mut completeness = PythonCompleteness::Full;
        let mut earlier_entries_uncertain = false;

        for item in dict.items.iter().rev() {
            let Some(key_expr) = &item.key else {
                completeness = PythonCompleteness::Partial;
                earlier_entries_uncertain = true;
                continue;
            };

            let key = self.evaluate_value(state, key_expr);
            if let PythonValueKind::Str(key_text) = key.kind()
                && !seen_string_keys.insert(key_text.clone())
            {
                continue;
            }

            let mut value = self.evaluate_value(state, &item.value);
            if earlier_entries_uncertain {
                value.mark_partial();
            }
            if !key.is_complete() || !value.is_complete() {
                completeness = PythonCompleteness::Partial;
            }

            entries.push(PythonDictEntry { key, value });
        }

        entries.reverse();
        PythonValue::new(
            PythonValueKind::Dict(PythonDict { entries }),
            origin,
            completeness,
        )
    }

    fn bind_unknown_targets(&mut self, state: &mut PythonSemanticState, target: &ast::Expr) {
        for name in target_write_names(target) {
            Self::bind_unknown_name(state, name, self.origin(target));
        }
    }

    fn bind_unknown_name(state: &mut PythonSemanticState, name: &str, origin: Origin) {
        state
            .bindings
            .bind(name, PythonBinding::unknown(name, origin));
    }

    fn record_mutation(state: &mut PythonSemanticState, target: &ast::Expr, method: &str) {
        if let Some(target) = MutationTarget::from_expr(target) {
            state.mutations.push(PythonMutation {
                root: target.root.to_string(),
                access: target
                    .access
                    .iter()
                    .map(MutationAccess::to_public)
                    .collect(),
                method: method.to_string(),
            });
        }
    }

    fn record_unsupported_touches(
        &mut self,
        state: &mut PythonSemanticState,
        names: impl IntoIterator<Item = String>,
    ) {
        for name in names {
            Self::bind_unknown_name(
                state,
                &name,
                Origin::new(self.source.file(), Span::new(0, 0)),
            );
        }
    }

    fn apply_append_mutation(
        &mut self,
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        value: &ast::Expr,
        _origin: Origin,
    ) -> bool {
        let value = self.evaluate_value(state, value);
        let value_complete = value.is_complete();
        Self::mutate_target(state, target, |target_value| match &mut target_value.kind {
            PythonValueKind::List(values) => {
                values.push(value);
                if !value_complete {
                    target_value.mark_partial();
                }
                true
            }
            _ => false,
        })
    }

    fn apply_extend_mutation(
        &mut self,
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        value: &ast::Expr,
        _origin: Origin,
    ) -> bool {
        let value = self.evaluate_value(state, value);
        let value_complete = value.is_complete();
        Self::mutate_target(state, target, |target_value| {
            match (&mut target_value.kind, value.kind) {
                (PythonValueKind::List(values), PythonValueKind::List(mut extension)) => {
                    values.append(&mut extension);
                    if !value_complete {
                        target_value.mark_partial();
                    }
                    true
                }
                (PythonValueKind::List(_), _) => {
                    target_value.mark_partial();
                    true
                }
                _ => false,
            }
        })
    }

    fn apply_insert_mutation(
        &mut self,
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        index: &ast::Expr,
        value: &ast::Expr,
        _origin: Origin,
    ) -> bool {
        let Some(index) = index.non_negative_integer() else {
            return Self::mark_target_partial(state, target);
        };
        let value = self.evaluate_value(state, value);
        let value_complete = value.is_complete();
        Self::mutate_target(state, target, |target_value| match &mut target_value.kind {
            PythonValueKind::List(values) => {
                let index = index.min(values.len());
                values.insert(index, value);
                if !value_complete {
                    target_value.mark_partial();
                }
                true
            }
            _ => false,
        })
    }

    fn apply_remove_mutation(
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        value: &ast::Expr,
    ) -> bool {
        let Some(value) = value.string_literal() else {
            return Self::mark_target_partial(state, target);
        };
        Self::mutate_target(state, target, |target_value| match &mut target_value.kind {
            PythonValueKind::List(values) => {
                if let Some(position) = values.iter().position(|item| {
                    matches!(item.kind(), PythonValueKind::Str(candidate) if candidate == value)
                }) {
                    values.remove(position);
                }
                true
            }
            _ => false,
        })
    }

    fn mark_target_partial(state: &mut PythonSemanticState, target: &ast::Expr) -> bool {
        Self::mutate_target(state, target, |value| {
            value.mark_partial();
            true
        })
    }

    fn mutate_target(
        state: &mut PythonSemanticState,
        target: &ast::Expr,
        mutate: impl FnOnce(&mut PythonValue) -> bool,
    ) -> bool {
        let Some(target) = MutationTarget::from_expr(target) else {
            return false;
        };
        let Some(binding) = state.bindings.by_name.get_mut(target.root) else {
            return false;
        };
        let [bound] = binding.values.as_mut_slice() else {
            binding.mark_partial();
            return true;
        };
        let Some(value) = target.resolve_mut(&mut bound.value) else {
            binding.mark_partial();
            return true;
        };
        let mutated = mutate(value);
        if mutated && !value.is_complete() {
            binding.mark_partial();
        }
        mutated
    }
}

impl PythonSemanticEvaluator<'_> {
    pub(super) fn evaluate_test(state: &PythonSemanticState, expr: &ast::Expr) -> Truthiness {
        Self::evaluate_test_expr(state, expr)
    }

    pub(super) fn collect_writes(body: &[ast::Stmt]) -> TouchedNames {
        collect_touched_names(body)
    }

    pub(super) fn degrade_writes(&self, state: &mut PythonSemanticState, writes: TouchedNames) {
        if writes.all {
            state.bindings.mark_all_partial();
        }
        state.degrade_names(
            writes.names,
            Origin::new(self.source.file(), Span::new(0, 0)),
        );
    }

    pub(super) fn bind_pattern_name(&self, state: &mut PythonSemanticState, name: &str) {
        state.bindings.bind(
            name,
            PythonBinding::unknown(name, Origin::new(self.source.file(), Span::new(0, 0))),
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct TouchedNames {
    names: FxHashSet<String>,
    all: bool,
}

impl TouchedNames {
    pub(super) fn record(&mut self, name: &str) {
        self.names.insert(name.to_string());
    }

    pub(super) fn record_all(&mut self) {
        self.all = true;
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.names.extend(other.names);
        self.all |= other.all;
    }
}

// This must mirror every statement effect that can alter bindings or mutation
// state. Branch joins and loop degradation use it to decide which names lose
// straight-line certainty.
fn collect_touched_names(body: &[ast::Stmt]) -> TouchedNames {
    let mut names = TouchedNames::default();
    for stmt in body {
        collect_stmt_touched_names(stmt, &mut names);
    }
    names
}

fn collect_stmt_touched_names(stmt: &ast::Stmt, names: &mut TouchedNames) {
    if collect_control_flow_stmt_touched_names(stmt, names) {
        return;
    }

    match stmt {
        ast::Stmt::Assign(assign) => {
            for target in &assign.targets {
                for name in target_write_names(target) {
                    names.record(name);
                }
            }
        }
        ast::Stmt::AnnAssign(assign) => {
            for name in target_write_names(&assign.target) {
                names.record(name);
            }
        }
        ast::Stmt::AugAssign(assign) => {
            for name in target_write_names(&assign.target) {
                names.record(name);
            }
        }
        ast::Stmt::Delete(delete) => {
            for target in &delete.targets {
                for name in target_write_names(target) {
                    names.record(name);
                }
            }
        }
        ast::Stmt::Expr(expr) => {
            if let ast::Expr::Call(call) = expr.value.as_ref()
                && let ast::Expr::Attribute(attribute) = call.func.as_ref()
                && let Some(target) = MutationTarget::from_expr(&attribute.value)
            {
                names.record(target.root);
            }
            for name in expr_read_names(&expr.value) {
                names.record(&name);
            }
        }
        ast::Stmt::Import(import) => {
            for alias in &import.names {
                let bound_name = alias.asname.as_ref().map_or_else(
                    || first_import_segment(alias.name.as_str()),
                    |asname| asname.as_str(),
                );
                names.record(bound_name);
            }
        }
        ast::Stmt::ImportFrom(import) => {
            if import.names.iter().any(|alias| alias.name.as_str() == "*") {
                names.record_all();
            } else {
                for alias in &import.names {
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                    names.record(bound_name);
                }
            }
        }
        ast::Stmt::FunctionDef(function) => names.record(function.name.as_str()),
        ast::Stmt::ClassDef(class) => names.record(class.name.as_str()),
        ast::Stmt::TypeAlias(type_alias) => {
            for name in target_write_names(&type_alias.name) {
                names.record(name);
            }
        }
        ast::Stmt::For(_)
        | ast::Stmt::While(_)
        | ast::Stmt::With(_)
        | ast::Stmt::Try(_)
        | ast::Stmt::If(_)
        | ast::Stmt::Match(_)
        | ast::Stmt::Return(_)
        | ast::Stmt::Raise(_)
        | ast::Stmt::Assert(_)
        | ast::Stmt::Global(_)
        | ast::Stmt::Nonlocal(_)
        | ast::Stmt::Pass(_)
        | ast::Stmt::Break(_)
        | ast::Stmt::Continue(_)
        | ast::Stmt::IpyEscapeCommand(_) => {}
    }
}

fn collect_control_flow_stmt_touched_names(stmt: &ast::Stmt, names: &mut TouchedNames) -> bool {
    match stmt {
        ast::Stmt::For(stmt_for) => {
            for name in target_write_names(&stmt_for.target) {
                names.record(name);
            }
            names.merge(collect_touched_names(&stmt_for.body));
            names.merge(collect_touched_names(&stmt_for.orelse));
        }
        ast::Stmt::While(stmt_while) => {
            names.merge(collect_touched_names(&stmt_while.body));
            names.merge(collect_touched_names(&stmt_while.orelse));
        }
        ast::Stmt::With(stmt_with) => {
            for item in &stmt_with.items {
                if let Some(optional_vars) = &item.optional_vars {
                    for name in target_write_names(optional_vars) {
                        names.record(name);
                    }
                }
            }
            names.merge(collect_touched_names(&stmt_with.body));
        }
        ast::Stmt::Try(stmt_try) => {
            names.merge(collect_touched_names(&stmt_try.body));
            for handler in &stmt_try.handlers {
                let ast::ExceptHandler::ExceptHandler(handler) = handler;
                names.merge(collect_touched_names(&handler.body));
            }
            names.merge(collect_touched_names(&stmt_try.orelse));
            names.merge(collect_touched_names(&stmt_try.finalbody));
        }
        ast::Stmt::If(stmt_if) => {
            names.merge(collect_touched_names(&stmt_if.body));
            for clause in &stmt_if.elif_else_clauses {
                names.merge(collect_touched_names(&clause.body));
            }
        }
        ast::Stmt::Match(stmt_match) => {
            for case in &stmt_match.cases {
                for name in pattern_bound_names(&case.pattern) {
                    names.record(name);
                }
                names.merge(collect_touched_names(&case.body));
            }
        }
        _ => return false,
    }
    true
}

fn target_write_names(target: &ast::Expr) -> Vec<&str> {
    let mut names = Vec::new();
    collect_target_write_names(target, &mut names);
    names
}

fn collect_target_write_names<'a>(target: &'a ast::Expr, names: &mut Vec<&'a str>) {
    if let Some(name) = target.name_target() {
        names.push(name);
        return;
    }

    match target {
        ast::Expr::Attribute(attribute) => collect_target_write_names(&attribute.value, names),
        ast::Expr::Subscript(subscript) => collect_target_write_names(&subscript.value, names),
        ast::Expr::Tuple(tuple) => {
            for expr in &tuple.elts {
                collect_target_write_names(expr, names);
            }
        }
        ast::Expr::List(list) => {
            for expr in &list.elts {
                collect_target_write_names(expr, names);
            }
        }
        ast::Expr::Starred(starred) => collect_target_write_names(&starred.value, names),
        _ => {}
    }
}

fn expr_read_names(expr: &ast::Expr) -> FxHashSet<String> {
    let mut names = FxHashSet::default();
    collect_expr_read_names(expr, &mut names);
    names
}

fn collect_expr_read_names(expr: &ast::Expr, names: &mut FxHashSet<String>) {
    if let Some(name) = expr.name_target() {
        names.insert(name.to_string());
    }
    if collect_simple_expr_read_names(expr, names) {
        return;
    }

    match expr {
        ast::Expr::If(if_expr) => {
            collect_expr_read_names(&if_expr.test, names);
            collect_expr_read_names(&if_expr.body, names);
            collect_expr_read_names(&if_expr.orelse, names);
        }
        ast::Expr::Lambda(lambda) => collect_expr_read_names(&lambda.body, names),
        ast::Expr::ListComp(comp) => collect_expr_read_names(&comp.elt, names),
        ast::Expr::SetComp(comp) => collect_expr_read_names(&comp.elt, names),
        ast::Expr::DictComp(comp) => {
            collect_expr_read_names(&comp.key, names);
            collect_expr_read_names(&comp.value, names);
        }
        ast::Expr::Generator(generator) => collect_expr_read_names(&generator.elt, names),
        ast::Expr::Await(await_expr) => collect_expr_read_names(&await_expr.value, names),
        ast::Expr::Yield(yield_expr) => {
            if let Some(value) = &yield_expr.value {
                collect_expr_read_names(value, names);
            }
        }
        ast::Expr::YieldFrom(yield_from) => collect_expr_read_names(&yield_from.value, names),
        ast::Expr::Named(named_expr) => {
            collect_target_write_names(&named_expr.target, &mut Vec::new());
            collect_expr_read_names(&named_expr.value, names);
        }
        ast::Expr::Slice(slice) => collect_slice_read_names(slice, names),
        ast::Expr::Attribute(_)
        | ast::Expr::Subscript(_)
        | ast::Expr::Call(_)
        | ast::Expr::BinOp(_)
        | ast::Expr::UnaryOp(_)
        | ast::Expr::BoolOp(_)
        | ast::Expr::Compare(_)
        | ast::Expr::Tuple(_)
        | ast::Expr::List(_)
        | ast::Expr::Set(_)
        | ast::Expr::Dict(_)
        | ast::Expr::Starred(_)
        | ast::Expr::FString(_)
        | ast::Expr::TString(_)
        | ast::Expr::Name(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::BytesLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NoneLiteral(_)
        | ast::Expr::EllipsisLiteral(_)
        | ast::Expr::IpyEscapeCommand(_) => {}
    }
}

fn collect_simple_expr_read_names(expr: &ast::Expr, names: &mut FxHashSet<String>) -> bool {
    match expr {
        ast::Expr::Attribute(attribute) => collect_expr_read_names(&attribute.value, names),
        ast::Expr::Subscript(subscript) => {
            collect_expr_read_names(&subscript.value, names);
            collect_expr_read_names(&subscript.slice, names);
        }
        ast::Expr::Call(call) => {
            collect_expr_read_names(&call.func, names);
            for arg in &call.arguments.args {
                collect_expr_read_names(arg, names);
            }
            for keyword in &call.arguments.keywords {
                collect_expr_read_names(&keyword.value, names);
            }
        }
        ast::Expr::BinOp(bin_op) => {
            collect_expr_read_names(&bin_op.left, names);
            collect_expr_read_names(&bin_op.right, names);
        }
        ast::Expr::UnaryOp(unary) => collect_expr_read_names(&unary.operand, names),
        ast::Expr::BoolOp(bool_op) => {
            for value in &bool_op.values {
                collect_expr_read_names(value, names);
            }
        }
        ast::Expr::Compare(compare) => {
            collect_expr_read_names(&compare.left, names);
            for comparator in &compare.comparators {
                collect_expr_read_names(comparator, names);
            }
        }
        ast::Expr::Tuple(tuple) => collect_elements_read_names(&tuple.elts, names),
        ast::Expr::List(list) => collect_elements_read_names(&list.elts, names),
        ast::Expr::Set(set) => collect_elements_read_names(&set.elts, names),
        ast::Expr::Dict(dict) => collect_dict_read_names(dict, names),
        ast::Expr::Starred(starred) => collect_expr_read_names(&starred.value, names),
        _ => return false,
    }
    true
}

fn collect_elements_read_names(elements: &[ast::Expr], names: &mut FxHashSet<String>) {
    for expr in elements {
        collect_expr_read_names(expr, names);
    }
}

fn collect_dict_read_names(dict: &ast::ExprDict, names: &mut FxHashSet<String>) {
    for item in &dict.items {
        if let Some(key) = &item.key {
            collect_expr_read_names(key, names);
        }
        collect_expr_read_names(&item.value, names);
    }
}

fn collect_slice_read_names(slice: &ast::ExprSlice, names: &mut FxHashSet<String>) {
    if let Some(lower) = &slice.lower {
        collect_expr_read_names(lower, names);
    }
    if let Some(upper) = &slice.upper {
        collect_expr_read_names(upper, names);
    }
    if let Some(step) = &slice.step {
        collect_expr_read_names(step, names);
    }
}

struct MutationTarget<'a> {
    root: &'a str,
    access: Vec<MutationAccess>,
}

impl<'a> MutationTarget<'a> {
    fn from_expr(expr: &'a ast::Expr) -> Option<Self> {
        let mut access = Vec::new();
        let root = collect_mutation_target(expr, &mut access)?;
        access.reverse();
        Some(Self { root, access })
    }

    fn resolve_mut<'b>(&self, value: &'b mut PythonValue) -> Option<&'b mut PythonValue> {
        let mut current = value;
        for access in &self.access {
            match access {
                MutationAccess::Index(index) => {
                    let PythonValueKind::List(values) = &mut current.kind else {
                        return None;
                    };
                    current = values.get_mut(*index)?;
                }
                MutationAccess::Key(key) => {
                    let PythonValueKind::Dict(dict) = &mut current.kind else {
                        return None;
                    };
                    current = dict.get_string_key_mut(key)?;
                }
            }
        }
        Some(current)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MutationAccess {
    Index(usize),
    Key(String),
}

impl MutationAccess {
    fn to_public(&self) -> PythonMutationAccess {
        match self {
            Self::Index(index) => PythonMutationAccess::Index(*index),
            Self::Key(key) => PythonMutationAccess::Key(key.clone()),
        }
    }
}

fn collect_mutation_target<'a>(
    expr: &'a ast::Expr,
    access: &mut Vec<MutationAccess>,
) -> Option<&'a str> {
    if let Some(name) = expr.name_target() {
        return Some(name);
    }

    let ast::Expr::Subscript(subscript) = expr else {
        return None;
    };

    if let Some(index) = subscript.slice.non_negative_integer() {
        access.push(MutationAccess::Index(index));
    } else if let Some(key) = subscript.slice.string_literal() {
        access.push(MutationAccess::Key(key.to_string()));
    } else {
        return None;
    }

    collect_mutation_target(&subscript.value, access)
}

fn first_import_segment(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}
