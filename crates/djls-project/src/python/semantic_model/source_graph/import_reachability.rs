use camino::Utf8Path;
use djls_source::File;
use ruff_python_ast as ast;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use super::PythonImportEdge;
use super::PythonImportKey;
use super::PythonImportKind;
use super::PythonModuleRecord;
use super::PythonSourceGraph;
use crate::ast::ExprExt;
use crate::python::ImportSourceResolution;
use crate::python::PythonImport;
use crate::python::PythonImportResolver;
use crate::python::semantic_model::control_flow::BranchPath;
use crate::python::semantic_model::control_flow::Truthiness;
use crate::python::semantic_model::control_flow::evaluate_test_with;
use crate::python::semantic_model::control_flow::is_irrefutable_match_case;
use crate::python::semantic_model::statement_walk;
use crate::python::semantic_model::statement_walk::StatementInterpreter;
use crate::python::semantic_model::touched_names::expr_read_names;
use crate::python::semantic_model::touched_names::first_import_segment;
use crate::python::semantic_model::touched_names::pattern_bound_names;
use crate::python::semantic_model::touched_names::target_write_names;

// Conservative guard evaluation for import reachability only.
//
// This pass decides which `from ... import ...` statements may execute before full semantic
// evaluation has loaded imported modules. It tracks only boolean guard bindings. Everything
// outside that narrow fact model becomes unknown so the graph includes rather than hides imports.
pub(super) fn collect_imports(
    graph: &mut PythonSourceGraph,
    resolver: &mut dyn PythonImportResolver,
) {
    PythonSourceGraphBuilder::new(graph, resolver).collect();
}

struct PythonSourceGraphBuilder<'graph, 'resolver> {
    graph: &'graph mut PythonSourceGraph,
    resolver: &'resolver mut dyn PythonImportResolver,
    collecting: FxHashSet<File>,
    collected: FxHashSet<File>,
    summaries: FxHashMap<File, ImportCollectionState>,
}

impl<'graph, 'resolver> PythonSourceGraphBuilder<'graph, 'resolver> {
    fn new(
        graph: &'graph mut PythonSourceGraph,
        resolver: &'resolver mut dyn PythonImportResolver,
    ) -> Self {
        Self {
            graph,
            resolver,
            collecting: FxHashSet::default(),
            collected: FxHashSet::default(),
            summaries: FxHashMap::default(),
        }
    }

    fn collect(&mut self) {
        self.collect_file(self.graph.root);
    }

    fn collect_file(&mut self, file: File) {
        if self.collected.contains(&file) || !self.collecting.insert(file) {
            return;
        }

        let Some(record) = self.graph.modules.remove(&file) else {
            self.collecting.remove(&file);
            self.collected.insert(file);
            return;
        };

        let summary = match &record {
            PythonModuleRecord::Parsed { source, module } => Some(self.collect_body(
                file,
                source.path(),
                ImportCollectionState::default(),
                &module.body,
            )),
            PythonModuleRecord::Unparseable { .. } | PythonModuleRecord::ReadFailed { .. } => None,
        };
        self.graph.modules.insert(file, record);
        if let Some(summary) = summary {
            self.summaries.insert(file, summary);
        }
        self.collecting.remove(&file);
        self.collected.insert(file);
    }

    fn collect_body(
        &mut self,
        file: File,
        importer: &Utf8Path,
        state: ImportCollectionState,
        body: &[ast::Stmt],
    ) -> ImportCollectionState {
        let mut collector = ImportReachabilityCollector {
            builder: self,
            file,
            importer,
        };
        statement_walk::walk_body(&mut collector, state, body)
    }

    fn collect_import_from(
        &mut self,
        file: File,
        importer: &Utf8Path,
        state: &mut ImportCollectionState,
        import: &ast::StmtImportFrom,
    ) {
        let key = PythonImportKey::from_import(importer, import);
        let python_import = PythonImport {
            level: import.level,
            module: import
                .module
                .as_ref()
                .map(ruff_python_ast::Identifier::as_str),
            importer,
        };
        let kind = if import.names.iter().any(|alias| alias.name.as_str() == "*") {
            PythonImportKind::Star
        } else {
            PythonImportKind::Named
        };

        let resolution = match kind {
            PythonImportKind::Star => self.resolver.resolve_star_import(python_import),
            PythonImportKind::Named => self.resolver.resolve_named_import(python_import),
        };
        let edge = match resolution {
            ImportSourceResolution::Resolved(source) => {
                let imported_file = source.file();
                if !self.collecting.contains(&imported_file) {
                    self.graph
                        .modules
                        .entry(imported_file)
                        .or_insert_with(|| PythonModuleRecord::parse(source));
                }
                self.collect_file(imported_file);
                let summary = self.summaries.get(&imported_file).map_or(
                    ResolvedImportSummary::Unavailable,
                    ResolvedImportSummary::Available,
                );
                state.apply_resolved_import(import, kind, summary);
                PythonImportEdge::Resolved {
                    import: key,
                    file: imported_file,
                    kind,
                }
            }
            ImportSourceResolution::Unresolved => {
                state.apply_unresolved_import(import, kind);
                PythonImportEdge::Unresolved { import: key, kind }
            }
            ImportSourceResolution::SkippedExternal => {
                state.apply_unresolved_import(import, kind);
                PythonImportEdge::SkippedExternal { import: key, kind }
            }
            ImportSourceResolution::ReadFailed { file, path } => {
                self.graph
                    .modules
                    .entry(file)
                    .or_insert_with(|| PythonModuleRecord::ReadFailed {
                        file,
                        path: path.clone(),
                    });
                state.apply_unresolved_import(import, kind);
                PythonImportEdge::ReadFailed {
                    import: key,
                    file,
                    path,
                    kind,
                }
            }
        };
        self.graph.imports.entry(file).or_default().push(edge);
    }
}

struct ImportReachabilityCollector<'builder, 'graph, 'resolver, 'importer> {
    builder: &'builder mut PythonSourceGraphBuilder<'graph, 'resolver>,
    file: File,
    importer: &'importer Utf8Path,
}

impl StatementInterpreter for ImportReachabilityCollector<'_, '_, '_, '_> {
    type State = ImportCollectionState;

    fn walk_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAssign) {
        state.record_assign(&assign.targets, &assign.value);
    }

    fn walk_ann_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAnnAssign) {
        if let Some(value) = &assign.value {
            state.record_assign(std::slice::from_ref(assign.target.as_ref()), value);
        } else {
            state.degrade_targets(std::slice::from_ref(assign.target.as_ref()));
        }
    }

    fn walk_aug_assign(&mut self, state: &mut Self::State, assign: &ast::StmtAugAssign) {
        state.degrade_targets(std::slice::from_ref(assign.target.as_ref()));
    }

    fn walk_import(&mut self, state: &mut Self::State, import: &ast::StmtImport) {
        state.record_import(&import.names);
    }

    fn walk_import_from(&mut self, state: &mut Self::State, import: &ast::StmtImportFrom) {
        self.builder
            .collect_import_from(self.file, self.importer, state, import);
    }

    fn walk_expr(&mut self, state: &mut Self::State, expr: &ast::StmtExpr) {
        state.degrade_names(expr_read_names(&expr.value));
    }

    fn bind_for_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        state.degrade_targets(std::slice::from_ref(target));
    }

    fn bind_with_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        state.degrade_targets(std::slice::from_ref(target));
    }

    fn bind_function(&mut self, state: &mut Self::State, function: &ast::StmtFunctionDef) {
        state.bind_unknown(function.name.as_str());
    }

    fn bind_class(&mut self, state: &mut Self::State, class: &ast::StmtClassDef) {
        state.bind_unknown(class.name.as_str());
    }

    fn bind_delete_target(&mut self, state: &mut Self::State, target: &ast::Expr) {
        state.degrade_targets(std::slice::from_ref(target));
    }

    fn bind_type_alias(&mut self, state: &mut Self::State, alias: &ast::StmtTypeAlias) {
        state.degrade_targets(std::slice::from_ref(alias.name.as_ref()));
    }

    fn bind_pattern_names(&mut self, state: &mut Self::State, pattern: &ast::Pattern) {
        state.bind_pattern_names(pattern);
    }

    fn evaluate_test(&self, state: &Self::State, expr: &ast::Expr) -> Truthiness {
        state.evaluate_test(expr)
    }

    fn degrade_loop_bodies(
        &mut self,
        mut state: Self::State,
        bodies: &[&[ast::Stmt]],
    ) -> Self::State {
        let base = state.clone();
        let mut changed_names = FxHashSet::default();
        for body in bodies {
            let body_state = statement_walk::walk_body(self, base.clone(), body);
            changed_names.extend(ImportCollectionState::changed_names_from(
                &base,
                &body_state,
            ));
        }
        state.degrade_names(changed_names);
        state
    }

    fn join_ambiguous_paths(
        &mut self,
        state: Self::State,
        paths: &[BranchPath<'_>],
    ) -> Self::State {
        let mut branches = Vec::with_capacity(paths.len());
        for path in paths {
            let mut branch = state.clone();
            for segment in path.segments() {
                branch = statement_walk::walk_body(self, branch, segment);
            }
            branches.push(branch);
        }
        ImportCollectionState::join(&branches)
    }

    fn join_match_cases(&mut self, state: Self::State, cases: &[ast::MatchCase]) -> Self::State {
        let mut branches = Vec::with_capacity(cases.len() + 1);
        for case in cases {
            let mut branch = state.clone();
            branch.bind_pattern_names(&case.pattern);
            branches.push(statement_walk::walk_body(self, branch, &case.body));
        }
        if !cases.iter().any(is_irrefutable_match_case) {
            branches.push(state);
        }
        ImportCollectionState::join(&branches)
    }
}

#[derive(Debug, Clone, Copy)]
enum ResolvedImportSummary<'a> {
    Available(&'a ImportCollectionState),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardBinding {
    Known(bool),
    Unknown,
}

impl GuardBinding {
    const fn bool_value(self) -> Option<bool> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ImportCollectionState {
    bool_bindings: FxHashMap<String, GuardBinding>,
}

impl ImportCollectionState {
    fn record_assign(&mut self, targets: &[ast::Expr], value: &ast::Expr) {
        if targets.len() == 1
            && let Some(name) = targets[0].name_target()
        {
            let value = self.assigned_bool(value);
            self.bool_bindings.insert(name.to_string(), value);
            return;
        }
        self.degrade_targets(targets);
    }

    fn record_import(&mut self, aliases: &[ast::Alias]) {
        for alias in aliases {
            let bound_name = alias.asname.as_ref().map_or_else(
                || first_import_segment(alias.name.as_str()),
                |asname| asname.as_str(),
            );
            self.bind_unknown(bound_name);
        }
    }

    fn assigned_bool(&self, value: &ast::Expr) -> GuardBinding {
        if let Some(value) = value.bool_literal() {
            return GuardBinding::Known(value);
        }
        value
            .name_target()
            .and_then(|name| self.bool_bindings.get(name).copied())
            .unwrap_or(GuardBinding::Unknown)
    }

    fn degrade_targets(&mut self, targets: &[ast::Expr]) {
        for target in targets {
            for name in target_write_names(target) {
                self.bind_unknown(name);
            }
        }
    }

    fn bind_unknown(&mut self, name: &str) {
        self.bool_bindings
            .insert(name.to_string(), GuardBinding::Unknown);
    }

    fn degrade_names(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            self.bind_unknown(&name);
        }
    }

    fn bind_pattern_names(&mut self, pattern: &ast::Pattern) {
        for name in pattern_bound_names(pattern) {
            self.bind_unknown(name);
        }
    }

    fn apply_resolved_import(
        &mut self,
        import: &ast::StmtImportFrom,
        kind: PythonImportKind,
        imported: ResolvedImportSummary<'_>,
    ) {
        let ResolvedImportSummary::Available(imported) = imported else {
            self.apply_unresolved_import(import, kind);
            return;
        };

        match kind {
            PythonImportKind::Star => {
                self.bool_bindings.extend(imported.bool_bindings.clone());
            }
            PythonImportKind::Named => {
                for alias in &import.names {
                    let imported_name = alias.name.as_str();
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| imported_name, |asname| asname.as_str());
                    let value = imported
                        .bool_bindings
                        .get(imported_name)
                        .copied()
                        .unwrap_or(GuardBinding::Unknown);
                    self.bool_bindings.insert(bound_name.to_string(), value);
                }
            }
        }
    }

    fn apply_unresolved_import(&mut self, import: &ast::StmtImportFrom, kind: PythonImportKind) {
        match kind {
            PythonImportKind::Star => {
                for value in self.bool_bindings.values_mut() {
                    *value = GuardBinding::Unknown;
                }
            }
            PythonImportKind::Named => {
                for alias in &import.names {
                    let bound_name = alias
                        .asname
                        .as_ref()
                        .map_or_else(|| alias.name.as_str(), |asname| asname.as_str());
                    self.bind_unknown(bound_name);
                }
            }
        }
    }

    fn evaluate_test(&self, expr: &ast::Expr) -> Truthiness {
        evaluate_test_with(expr, |name| {
            self.bool_bindings
                .get(name)
                .copied()
                .and_then(GuardBinding::bool_value)
        })
    }

    fn changed_names_from(base: &Self, changed: &Self) -> FxHashSet<String> {
        let mut names = FxHashSet::default();
        for (name, value) in &changed.bool_bindings {
            if base.bool_bindings.get(name) != Some(value) {
                names.insert(name.clone());
            }
        }
        for name in base.bool_bindings.keys() {
            if !changed.bool_bindings.contains_key(name) {
                names.insert(name.clone());
            }
        }
        names
    }

    fn join(branches: &[Self]) -> Self {
        let Some(first) = branches.first() else {
            return Self::default();
        };
        let mut joined = first.clone();
        for branch in &branches[1..] {
            let names: Vec<String> = joined.bool_bindings.keys().cloned().collect();
            for name in names {
                if joined.bool_bindings.get(&name) != branch.bool_bindings.get(&name) {
                    joined.bool_bindings.insert(name, GuardBinding::Unknown);
                }
            }
            for name in branch.bool_bindings.keys() {
                if !joined.bool_bindings.contains_key(name) {
                    joined
                        .bool_bindings
                        .insert(name.clone(), GuardBinding::Unknown);
                }
            }
        }
        joined
    }
}
