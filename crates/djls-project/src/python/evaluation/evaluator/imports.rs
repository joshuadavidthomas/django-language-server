use djls_source::File;
use djls_source::FileReadError;

use super::BranchConstraints;
use super::EvaluationState;
use super::Evaluator;
use super::Origin;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonImportOutcome;
use super::PythonModuleDependencies;
use super::PythonModuleObjects;
use super::PythonModuleValues;
use super::PythonNamespaceCause;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::ast;
use crate::python::PythonModuleName;
use crate::python::evaluation::PythonImportEdge;
use crate::python::evaluation::PythonImportEvaluationStatus;
use crate::python::evaluation::PythonModuleObjectId;
use crate::python::evaluation::query::evaluate_python_module;
use crate::python::evaluation::result::CycleMembership;
use crate::python::evaluation::result::PythonModuleEvaluation;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;
use crate::python::module::PythonImportChainResolution;
use crate::python::module::PythonImportRequest;
use crate::python::module::PythonImportResolutionError;
use crate::python::module::PythonModule;
use crate::python::module::ResolvedChainComponent;

impl Evaluator<'_> {
    pub(super) fn evaluate_import_statement(&mut self, statement: &ast::Stmt) {
        match statement {
            ast::Stmt::Import(statement) => {
                for clause in DirectImportClause::lower(statement) {
                    self.evaluate_direct_clause(&clause);
                }
            }
            ast::Stmt::ImportFrom(statement) => {
                self.evaluate_from_import(statement);
            }
            ast::Stmt::Assign(_)
            | ast::Stmt::AnnAssign(_)
            | ast::Stmt::AugAssign(_)
            | ast::Stmt::Delete(_)
            | ast::Stmt::TypeAlias(_)
            | ast::Stmt::Expr(_)
            | ast::Stmt::FunctionDef(_)
            | ast::Stmt::ClassDef(_)
            | ast::Stmt::For(_)
            | ast::Stmt::While(_)
            | ast::Stmt::If(_)
            | ast::Stmt::With(_)
            | ast::Stmt::Try(_)
            | ast::Stmt::Match(_)
            | ast::Stmt::Return(_)
            | ast::Stmt::Raise(_)
            | ast::Stmt::Assert(_)
            | ast::Stmt::Global(_)
            | ast::Stmt::Nonlocal(_)
            | ast::Stmt::Pass(_)
            | ast::Stmt::Break(_)
            | ast::Stmt::Continue(_)
            | ast::Stmt::IpyEscapeCommand(_) => {
                unreachable!("statement dispatcher passes only imports")
            }
        }
    }

    /// Policy-neutral projection of a module object's member. It reads an
    /// attached loaded child first, then falls back to the module's intrinsic
    /// source binding through the cycle-enabled core query, applying namespace
    /// remainder and syntax impacts. Residual absence is preserved as `Unbound`
    /// so each caller applies its own failure policy: expression reads translate
    /// it to a typed module-attribute unknown, while Plan 006's named import
    /// caller will instead attempt submodule fallback.
    pub(super) fn project_module_member(
        &self,
        object: &PythonModuleObjectId,
        member: &str,
        origin: Origin,
    ) -> PythonBinding {
        // Compose the loaded-child alternatives with the intrinsic source
        // fallback and object-scoped open causes. The recursive source query is
        // only issued when a residual `Unbound` remains, preserving direct
        // recursive-query legality when a child already covers the read.
        let mut binding = self
            .state
            .module_objects
            .child_binding(object, member, origin);
        if binding
            .alternatives()
            .any(|state| *state == PythonBindingState::Unbound)
        {
            let fallback = self.project_intrinsic_source_member(object, member, origin);
            binding = binding.replace_unbound_with(Some(fallback), origin);
        }
        binding = self
            .state
            .module_objects
            .apply_open_causes(object, binding, origin);
        binding.rebase_cycle_unknowns(origin);
        binding
    }

    /// The module's intrinsic source binding for `member`, via the cycle-enabled
    /// core query, with namespace remainder and syntax impacts applied. A
    /// namespace package has no intrinsic body, so its members are always a
    /// residual `Unbound`.
    fn project_intrinsic_source_member(
        &self,
        object: &PythonModuleObjectId,
        member: &str,
        origin: Origin,
    ) -> PythonBinding {
        match object {
            // A namespace package and an external (non-project) module have no
            // intrinsic body we evaluate, so their members are a residual
            // `Unbound` for the caller's open causes to interpret.
            PythonModuleObjectId::Source(module) if !module.search_path().is_project_code() => {
                PythonBinding::unbound()
            }
            PythonModuleObjectId::Source(module) => {
                match evaluate_python_module(self.db, self.project, module.clone()) {
                    PythonModuleEvaluation::CycleSeed => {
                        PythonBinding::unknown(&PythonUnknownCause::Cycle, origin)
                    }
                    PythonModuleEvaluation::Evaluated(evaluated) => {
                        let (values, _dependencies, _objects) = evaluated.into_parts();
                        match values {
                            Err(error) => PythonBinding::unknown(
                                &PythonUnknownCause::Unreadable(error),
                                origin,
                            ),
                            Ok(values) => self.project_source_member(&values, member, origin),
                        }
                    }
                }
            }
            PythonModuleObjectId::Namespace(_) => PythonBinding::unbound(),
        }
    }

    fn project_source_member(
        &self,
        values: &PythonModuleValues,
        member: &str,
        origin: Origin,
    ) -> PythonBinding {
        let mut binding = match values.bindings.get(member) {
            Some(imported) => imported.clone().rebase_binding_origin(origin),
            None => PythonBinding::unbound(),
        };
        binding.rebase_cycle_unknowns(origin);

        if let Some(remainder) = &values.namespace_remainder {
            let unbound_constraints = binding
                .alternatives_with_constraints()
                .filter_map(|(alternative, constraints)| {
                    (*alternative == PythonBindingState::Unbound).then_some(constraints.clone())
                })
                .collect::<Vec<_>>();
            for unbound in &unbound_constraints {
                for cause in remainder.as_slice() {
                    let constraints = unbound.intersection(&cause.constraints);
                    if let Some(unknown) = PythonBinding::constrained_unknown(
                        &cause.unknown.cause,
                        origin,
                        &constraints,
                    ) {
                        binding = binding.join(unknown, origin);
                    }
                }
            }
        }

        let syntax_errors = values
            .syntax_impacts
            .iter()
            .filter(|impact| impact.affects(member))
            .map(|impact| impact.error.clone())
            .collect::<Vec<_>>();
        if !syntax_errors.is_empty() {
            binding = binding.join(
                PythonBinding::unknown(&PythonUnknownCause::SyntaxErrors(syntax_errors), origin),
                origin,
            );
        }

        binding
    }

    /// Evaluate and bind one direct-import clause through the shared chain
    /// loader. Unaliased clauses bind the top-level package root; aliased
    /// clauses bind the leaf. Definite failures bind a typed unknown without
    /// erasing prior clause effects.
    fn evaluate_direct_clause(&mut self, clause: &DirectImportClause<'_>) {
        // Component edge/outcome effects are attributed to the full clause span;
        // the local name binding is attributed to its exact target span.
        let clause_origin = self.origin_at(clause.clause_span());
        let binding_origin = self.origin_at(clause.binding_span());
        let request = PythonImportRequest {
            level: 0,
            module: Some(clause.requested()),
            importer: &self.module,
        };
        match PythonModule::resolve_import_chain(self.db, self.project, request) {
            Err(error) => {
                self.state
                    .record_import(PythonImportOutcome::InvalidImport {
                        origin: clause_origin,
                        reason: error.clone(),
                    });
                self.state.bind_unknown(
                    clause.bound(),
                    &PythonUnknownCause::InvalidImport(error),
                    binding_origin,
                );
            }
            Ok((name, resolution)) => {
                let load = self.load_import_chain(&name, resolution, clause_origin);
                self.bind_direct_clause(clause, &load, binding_origin);
            }
        }
    }

    /// Bind a direct-import clause's local name from a loaded chain.
    ///
    /// Unaliased clauses bind the top-level package root; aliased clauses bind
    /// the requested leaf. A definite not-found/unreadable terminal binds a
    /// typed unknown even for a root binding, because the statement as a whole
    /// failed. An intermediate cycle binds the module handle only when the
    /// selected component (root for unaliased, leaf for aliased) was actually
    /// reached; otherwise it binds a typed cycle unknown.
    fn bind_direct_clause(
        &mut self,
        clause: &DirectImportClause<'_>,
        load: &ChainLoad,
        binding_origin: Origin,
    ) {
        if clause.binds_root() {
            match (load.leaf.is_definite_failure(), &load.root) {
                (false, Some(root)) => {
                    self.state.assign_value(
                        clause.bound(),
                        PythonValue::module(root.clone(), binding_origin),
                        binding_origin,
                    );
                }
                // A definite failure or a root that was never reached (a root
                // resolution failure with an empty prefix) binds a typed unknown.
                (true, _) | (false, None) => {
                    self.state.bind_unknown(
                        clause.bound(),
                        &load.leaf.failure_cause(),
                        binding_origin,
                    );
                }
            }
        } else if load.leaf_reached
            && let Some(object) = load.leaf.object()
        {
            self.state.assign_value(
                clause.bound(),
                PythonValue::module(object, binding_origin),
                binding_origin,
            );
        } else {
            self.state
                .bind_unknown(clause.bound(), &load.leaf.failure_cause(), binding_origin);
        }
    }

    /// Evaluate a `from ... import ...` statement: load the source chain (with
    /// its new parent-package effects) then freeze named/star selection on the
    /// terminal source module's values, exactly as before.
    fn evaluate_from_import(&mut self, statement: &ast::StmtImportFrom) {
        let import = FromImport::lower(self, statement);
        let request = PythonImportRequest {
            level: import.level,
            module: import.module,
            importer: &self.module,
        };
        match PythonModule::resolve_import_chain(self.db, self.project, request) {
            Err(error) => {
                self.state
                    .record_import(PythonImportOutcome::InvalidImport {
                        origin: import.origin,
                        reason: error.clone(),
                    });
                self.state
                    .apply_from_failure(&import, &PythonUnknownCause::InvalidImport(error));
            }
            Ok((name, resolution)) => {
                let load = self.load_import_chain(&name, resolution, import.origin);
                match load.leaf {
                    ChainOutcome::Source { values, .. } => {
                        self.state.apply_from_selection(&import, &name, &values);
                    }
                    ChainOutcome::Namespace { .. } => {
                        // A namespace terminal has no loadable source module, so
                        // for member selection it is a not-found source. The
                        // loader records no outcome for a namespace terminal, so
                        // record the canonical not-found here.
                        self.state.record_import(PythonImportOutcome::NotFound {
                            origin: import.origin,
                            module: name.clone(),
                        });
                        self.state
                            .apply_from_failure(&import, &PythonUnknownCause::ImportNotFound(name));
                    }
                    ChainOutcome::NotFound { module } => {
                        // The loader already recorded the not-found outcome.
                        self.state.apply_from_failure(
                            &import,
                            &PythonUnknownCause::ImportNotFound(module),
                        );
                    }
                    ChainOutcome::External { module, .. } => {
                        self.state.apply_from_failure(
                            &import,
                            &PythonUnknownCause::SkippedExternal(module),
                        );
                    }
                    ChainOutcome::Cycle { .. } => {
                        self.state
                            .apply_from_failure(&import, &PythonUnknownCause::Cycle);
                    }
                    ChainOutcome::Unreadable { error, .. } => {
                        self.state
                            .apply_from_failure(&import, &PythonUnknownCause::Unreadable(error));
                    }
                }
            }
        }
    }

    /// Load a resolved import chain root-to-leaf, recording an
    /// importer -> component edge, dependency file, and outcome for every
    /// evaluated project source component, merging each component's own child
    /// effects, and attaching successful components under their parent. Failed
    /// components are never attached; earlier prefix effects always survive.
    fn load_import_chain(
        &mut self,
        name: &PythonModuleName,
        resolution: PythonImportChainResolution,
        origin: Origin,
    ) -> ChainLoad {
        let (components, failure) = match resolution {
            PythonImportChainResolution::Resolved(chain) => (chain.into_components(), None),
            PythonImportChainResolution::Failed { prefix, failure } => {
                (prefix.into_components(), Some(failure))
            }
        };
        let resolved_len = components.len();

        let mut root: Option<PythonModuleObjectId> = None;
        let mut parent: Option<PythonModuleObjectId> = None;
        let mut terminal: Option<ChainOutcome> = None;
        let mut leaf_reached = false;
        let mut external = false;

        for (index, component) in components.into_iter().enumerate() {
            let is_last = failure.is_none() && index + 1 == resolved_len;
            let (attribute, object) = component_identity(&component);

            if external {
                self.state.attach_external_component(
                    parent.as_ref(),
                    &attribute,
                    &object,
                    name,
                    origin,
                );
                root.get_or_insert_with(|| object.clone());
                parent = Some(object.clone());
                if is_last {
                    leaf_reached = true;
                    terminal = Some(ChainOutcome::External {
                        object,
                        module: name.clone(),
                    });
                }
                continue;
            }

            match component {
                ResolvedChainComponent::Namespace(_) => {
                    self.state
                        .attach_component(parent.as_ref(), &attribute, &object, origin);
                    root.get_or_insert_with(|| object.clone());
                    parent = Some(object.clone());
                    if is_last {
                        leaf_reached = true;
                        terminal = Some(ChainOutcome::Namespace { object });
                    }
                }
                ResolvedChainComponent::Source(module)
                    if !module.search_path().is_project_code() =>
                {
                    // First external component: one skipped outcome for the full
                    // requested leaf, then the resolved suffix is constructed
                    // without evaluation.
                    external = true;
                    self.state
                        .record_import(PythonImportOutcome::SkippedExternal {
                            origin,
                            module: name.clone(),
                        });
                    self.state.attach_external_component(
                        parent.as_ref(),
                        &attribute,
                        &object,
                        name,
                        origin,
                    );
                    root.get_or_insert_with(|| object.clone());
                    parent = Some(object.clone());
                    if is_last {
                        leaf_reached = true;
                        terminal = Some(ChainOutcome::External {
                            object,
                            module: name.clone(),
                        });
                    }
                }
                ResolvedChainComponent::Source(module)
                    if !is_last && self.is_importer_self(&module) =>
                {
                    // The importer's own module is already being initialized on
                    // the import stack; re-evaluating exactly itself would be a
                    // spurious self-cycle. Create its handle and continue so
                    // descendant components still resolve and attach. Parent
                    // packages (which are distinct modules) are evaluated
                    // normally so their `__init__.py` effects load.
                    self.state
                        .attach_component(parent.as_ref(), &attribute, &object, origin);
                    root.get_or_insert_with(|| object.clone());
                    parent = Some(object.clone());
                }
                ResolvedChainComponent::Source(module) => {
                    match self.evaluate_source_component(&module, origin) {
                        SourceComponent::Cycle => {
                            self.state.attach_component(
                                parent.as_ref(),
                                &attribute,
                                &object,
                                origin,
                            );
                            self.state.open_component_cycle(&object, origin);
                            root.get_or_insert_with(|| object.clone());
                            // A cycle at the last requested component still
                            // reaches the requested leaf as a partial handle;
                            // an intermediate cycle does not.
                            leaf_reached = is_last;
                            terminal = Some(ChainOutcome::Cycle { object });
                            break;
                        }
                        SourceComponent::Unreadable(error) => {
                            leaf_reached = is_last;
                            terminal = Some(ChainOutcome::Unreadable { error });
                            break;
                        }
                        SourceComponent::Evaluated(values, objects) => {
                            self.state.module_objects_merge(objects, origin);
                            self.state.attach_component(
                                parent.as_ref(),
                                &attribute,
                                &object,
                                origin,
                            );
                            root.get_or_insert_with(|| object.clone());
                            parent = Some(object.clone());
                            if is_last {
                                leaf_reached = true;
                                terminal = Some(ChainOutcome::Source { object, values });
                            }
                        }
                    }
                }
            }
        }

        let leaf = match terminal {
            Some(terminal) => terminal,
            None => self.record_chain_not_found(failure, name, origin),
        };
        ChainLoad {
            root,
            leaf,
            leaf_reached,
        }
    }

    /// Record the canonical not-found outcome for a chain that never reached a
    /// terminal, returning the not-found classification.
    fn record_chain_not_found(
        &mut self,
        failure: Option<PythonImportResolutionError>,
        name: &PythonModuleName,
        origin: Origin,
    ) -> ChainOutcome {
        let module = match failure {
            Some(PythonImportResolutionError::NotFound(module)) => module,
            Some(PythonImportResolutionError::Invalid(_)) | None => name.clone(),
        };
        self.state.record_import(PythonImportOutcome::NotFound {
            origin,
            module: module.clone(),
        });
        ChainOutcome::NotFound { module }
    }

    /// Whether `module` is exactly the importer's own file. Only the importer's
    /// own source file is already being initialized on the import stack, so
    /// re-evaluating it would be a spurious self-cycle. This is a file identity
    /// check, not a name check, so a package importer reached both as `pkg` and
    /// as its `pkg.__init__` file alias is recognized as the same self. Ancestor
    /// packages are distinct files whose `__init__.py` effects must still load,
    /// so they are not matched here.
    fn is_importer_self(&self, module: &PythonModule) -> bool {
        self.module.file() == module.file()
    }

    /// Evaluate one project source component through the recursive core query,
    /// recording its edge, dependency file, and outcome.
    fn evaluate_source_component(
        &mut self,
        module: &PythonModule,
        origin: Origin,
    ) -> SourceComponent {
        let edge = PythonImportEdge {
            origin,
            importer: self.module.clone(),
            imported: module.clone(),
        };
        match evaluate_python_module(self.db, self.project, module.clone()) {
            PythonModuleEvaluation::CycleSeed => {
                self.state.record_component_edge(
                    module.file(),
                    None,
                    PythonImportOutcome::Evaluated {
                        edge,
                        status: PythonImportEvaluationStatus::Cycle {
                            syntax_errors: Vec::new(),
                        },
                    },
                );
                SourceComponent::Cycle
            }
            PythonModuleEvaluation::Evaluated(evaluated) => {
                let (values, dependencies, objects) = evaluated.into_parts();
                match values {
                    Ok(values) => {
                        let status = PythonImportEvaluationStatus::from_syntax_errors(
                            values.syntax_errors.clone(),
                            CycleMembership::Acyclic,
                        );
                        self.state.record_component_edge(
                            module.file(),
                            Some(&dependencies),
                            PythonImportOutcome::Evaluated { edge, status },
                        );
                        SourceComponent::Evaluated(values, objects)
                    }
                    Err(error) => {
                        self.state.record_component_edge(
                            module.file(),
                            Some(&dependencies),
                            PythonImportOutcome::Unreadable {
                                edge,
                                error: error.clone(),
                            },
                        );
                        SourceComponent::Unreadable(error)
                    }
                }
            }
        }
    }
}

/// The child attribute name and nominal object identity of a chain component.
fn component_identity(component: &ResolvedChainComponent) -> (String, PythonModuleObjectId) {
    let attribute = component
        .name()
        .as_str()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_string();
    let object = match component {
        ResolvedChainComponent::Source(module) => PythonModuleObjectId::Source(module.clone()),
        ResolvedChainComponent::Namespace(package) => {
            PythonModuleObjectId::Namespace(package.clone())
        }
    };
    (attribute, object)
}

/// One component's evaluation classification, returned to the chain walker.
enum SourceComponent {
    Evaluated(PythonModuleValues, PythonModuleObjects),
    Cycle,
    Unreadable(FileReadError),
}

/// The terminal classification of a loaded chain, used for direct-import
/// binding and from-import member selection.
enum ChainOutcome {
    Source {
        object: PythonModuleObjectId,
        values: PythonModuleValues,
    },
    Namespace {
        object: PythonModuleObjectId,
    },
    Cycle {
        object: PythonModuleObjectId,
    },
    External {
        object: PythonModuleObjectId,
        module: PythonModuleName,
    },
    Unreadable {
        error: FileReadError,
    },
    NotFound {
        module: PythonModuleName,
    },
}

impl ChainOutcome {
    /// The bindable module object for a direct import, if the terminal resolved
    /// to a module identity. Hard failures have no object.
    fn object(&self) -> Option<PythonModuleObjectId> {
        match self {
            Self::Source { object, .. }
            | Self::Namespace { object }
            | Self::Cycle { object }
            | Self::External { object, .. } => Some(object.clone()),
            Self::Unreadable { .. } | Self::NotFound { .. } => None,
        }
    }

    /// Whether this terminal is a definite hard failure (not-found or
    /// unreadable). Such a failure means the import statement failed as a whole,
    /// so even an unaliased dotted import that resolved a root prefix must bind a
    /// typed unknown rather than the resolved root handle.
    fn is_definite_failure(&self) -> bool {
        matches!(self, Self::Unreadable { .. } | Self::NotFound { .. })
    }

    /// The typed unknown cause bound when a direct import's selected component
    /// could not resolve to a module identity.
    fn failure_cause(&self) -> PythonUnknownCause {
        match self {
            Self::Cycle { .. } => PythonUnknownCause::Cycle,
            Self::External { module, .. } => PythonUnknownCause::SkippedExternal(module.clone()),
            Self::Unreadable { error } => PythonUnknownCause::Unreadable(error.clone()),
            Self::NotFound { module } => PythonUnknownCause::ImportNotFound(module.clone()),
            Self::Source { .. } | Self::Namespace { .. } => {
                PythonUnknownCause::UnsupportedExpression
            }
        }
    }
}

/// The result of loading a chain: the root component object (for unaliased
/// direct imports) and the terminal classification.
struct ChainLoad {
    root: Option<PythonModuleObjectId>,
    leaf: ChainOutcome,
    /// Whether the full requested leaf component was actually reached. A chain
    /// broken by an intermediate cycle or failure reaches a terminal that is not
    /// the requested leaf, so an aliased import that targets the leaf must bind a
    /// typed unknown rather than the intermediate handle.
    leaf_reached: bool,
}

struct FromImport<'ast> {
    origin: Origin,
    level: u32,
    module: Option<&'ast str>,
    selection: ImportSelection<'ast>,
}

impl<'ast> FromImport<'ast> {
    fn lower(evaluator: &Evaluator<'_>, statement: &'ast ast::StmtImportFrom) -> Self {
        let syntax = FromImportSyntax::lower(statement);
        let selection = if syntax.has_star() {
            ImportSelection::Star
        } else {
            ImportSelection::Named(
                syntax
                    .named_members()
                    .iter()
                    .map(|clause| ImportedBinding {
                        imported: clause.imported(),
                        bound: clause.bound(),
                        origin: evaluator.origin_at(clause.binding_span()),
                    })
                    .collect(),
            )
        };
        Self {
            origin: evaluator.origin(statement),
            level: syntax.level(),
            module: syntax.module(),
            selection,
        }
    }
}

enum ImportSelection<'ast> {
    Star,
    Named(Vec<ImportedBinding<'ast>>),
}

struct ImportedBinding<'ast> {
    imported: &'ast str,
    bound: &'ast str,
    origin: Origin,
}

impl EvaluationState {
    /// Record one evaluated/unreadable source component: its dependency file,
    /// absorbed transitive dependencies, and its import outcome.
    fn record_component_edge(
        &mut self,
        file: File,
        dependencies: Option<&PythonModuleDependencies>,
        outcome: PythonImportOutcome,
    ) {
        // Record the immediate component's file and outcome in first-seen
        // root-to-leaf order *before* absorbing its transitive dependencies, so
        // the directly-imported edge precedes anything it pulls in.
        self.dependencies.files.insert(file);
        self.record_import(outcome);
        if let Some(dependencies) = dependencies {
            self.absorb_dependencies(dependencies);
        }
    }

    /// Attach a successfully-loaded component under its parent object, if any.
    /// A root component has no parent and is only bound as a local name.
    fn attach_component(
        &mut self,
        parent: Option<&PythonModuleObjectId>,
        attribute: &str,
        object: &PythonModuleObjectId,
        origin: Origin,
    ) {
        if let Some(parent) = parent {
            self.module_objects.attach_child(
                parent.clone(),
                attribute.to_string(),
                object.clone(),
                origin,
            );
        }
    }

    /// Attach an external suffix component and mark it open with a
    /// `SkippedExternal` cause for the full requested leaf; its body is never
    /// evaluated.
    fn attach_external_component(
        &mut self,
        parent: Option<&PythonModuleObjectId>,
        attribute: &str,
        object: &PythonModuleObjectId,
        module: &PythonModuleName,
        origin: Origin,
    ) {
        self.attach_component(parent, attribute, object, origin);
        self.module_objects.open_cause(
            object.clone(),
            PythonNamespaceCause::unconstrained(PythonUnknown::new(
                PythonUnknownCause::SkippedExternal(module.clone()),
                [origin],
            )),
        );
    }

    /// Mark a cycle-seed component's object open with a `Cycle` cause so reads of
    /// its attributes become cycle unknowns.
    fn open_component_cycle(&mut self, object: &PythonModuleObjectId, origin: Origin) {
        self.module_objects.open_cause(
            object.clone(),
            PythonNamespaceCause::unconstrained(PythonUnknown::new(
                PythonUnknownCause::Cycle,
                [origin],
            )),
        );
    }

    /// Merge a loaded component's own child effects into this importer's object
    /// state in source order.
    fn module_objects_merge(&mut self, objects: PythonModuleObjects, origin: Origin) {
        self.module_objects.merge(objects, origin);
    }

    /// Freeze named/star selection on a loaded source module, unchanged from the
    /// pre-chain behavior. Files, dependencies, and outcomes were already
    /// recorded by the chain loader.
    fn apply_from_selection(
        &mut self,
        import: &FromImport<'_>,
        module: &PythonModuleName,
        values: &PythonModuleValues,
    ) {
        match &import.selection {
            ImportSelection::Star => self.apply_star_import(values, import.origin),
            ImportSelection::Named(bindings) => {
                for binding in bindings {
                    self.bind_named_import(
                        module,
                        values,
                        binding.imported,
                        binding.bound,
                        binding.origin,
                    );
                }
            }
        }
    }

    /// Translate a source-chain failure through the existing named/star failure
    /// policy. The failure's import outcome was already recorded by the loader
    /// (or the invalid-import caller); this only rebinds selected members.
    fn apply_from_failure(&mut self, import: &FromImport<'_>, cause: &PythonUnknownCause) {
        match &import.selection {
            ImportSelection::Star => {
                self.degrade_all_bindings(
                    cause,
                    import.origin,
                    &BranchConstraints::unconstrained(),
                );
                self.namespace_causes
                    .push(PythonNamespaceCause::unconstrained(PythonUnknown::new(
                        cause.clone(),
                        [import.origin],
                    )));
            }
            ImportSelection::Named(bindings) => {
                for binding in bindings {
                    self.bind_unknown(binding.bound, cause, import.origin);
                }
            }
        }
    }

    fn apply_star_import(&mut self, values: &PythonModuleValues, import_origin: Origin) {
        if let Some(remainder) = &values.namespace_remainder {
            for cause in remainder.as_slice() {
                self.degrade_all_bindings(&cause.unknown.cause, import_origin, &cause.constraints);
            }
        }
        for (name, binding) in &values.bindings {
            let prior = self.bindings.get(name).cloned();
            let mut binding = binding.clone();
            binding.rebase_cycle_unknowns(import_origin);
            self.bindings.insert(
                name.clone(),
                binding.replace_unbound_with(prior, import_origin),
            );
        }
        let mut namespace_errors = Vec::new();
        for impact in &values.syntax_impacts {
            let affected = self
                .bindings
                .keys()
                .filter(|name| impact.affects(name))
                .cloned()
                .collect::<Vec<_>>();
            if !affected.is_empty() {
                self.degrade_names(
                    affected,
                    &PythonUnknownCause::SyntaxErrors(vec![impact.error.clone()]),
                    import_origin,
                );
            }
            if impact.namespace_open {
                namespace_errors.push(impact.error.clone());
            }
        }
        if !namespace_errors.is_empty() {
            self.namespace_causes
                .push(PythonNamespaceCause::unconstrained(PythonUnknown::new(
                    PythonUnknownCause::SyntaxErrors(namespace_errors),
                    [import_origin],
                )));
        }
        self.mutations.extend(values.mutations.iter().cloned());
        if let Some(remainder) = &values.namespace_remainder {
            self.namespace_causes
                .extend(remainder.as_slice().iter().cloned().map(|mut cause| {
                    cause.unknown.replace_origins([import_origin]);
                    cause
                }));
        }
    }

    fn bind_named_import(
        &mut self,
        module: &PythonModuleName,
        values: &PythonModuleValues,
        imported_name: &str,
        bound_name: &str,
        origin: Origin,
    ) {
        let syntax_errors = values
            .syntax_impacts
            .iter()
            .filter(|impact| impact.affects(imported_name))
            .map(|impact| impact.error.clone())
            .collect::<Vec<_>>();
        let missing_member = PythonUnknownCause::MissingImportMember {
            module: module.clone(),
            member: imported_name.to_string(),
        };
        let (mut binding, unbound_constraints) = match values.bindings.get(imported_name) {
            Some(imported) => {
                let imported = imported.clone().rebase_binding_origin(origin);
                let constraints = imported
                    .alternatives_with_constraints()
                    .filter_map(|(alternative, constraints)| {
                        (*alternative == PythonBindingState::Unbound).then_some(constraints.clone())
                    })
                    .collect::<Vec<_>>();
                (imported, constraints)
            }
            None => (
                PythonBinding::unknown(&missing_member, origin),
                vec![BranchConstraints::unconstrained()],
            ),
        };
        binding.rebase_cycle_unknowns(origin);
        if let Some(remainder) = &values.namespace_remainder {
            for unbound in &unbound_constraints {
                for cause in remainder.as_slice() {
                    let constraints = unbound.intersection(&cause.constraints);
                    if let Some(unknown) = PythonBinding::constrained_unknown(
                        &cause.unknown.cause,
                        origin,
                        &constraints,
                    ) {
                        binding = binding.join(unknown, origin);
                    }
                }
            }
        }
        binding = binding.replace_unbound_with(
            Some(PythonBinding::unknown(&missing_member, origin)),
            origin,
        );
        if !syntax_errors.is_empty() {
            let unknown =
                PythonBinding::unknown(&PythonUnknownCause::SyntaxErrors(syntax_errors), origin);
            binding = binding.join(unknown, origin);
        }
        self.bindings.insert(bound_name.to_string(), binding);
        let copied = values
            .mutations
            .iter()
            .filter(|mutation| mutation.binding == imported_name)
            .cloned()
            .map(|mut mutation| {
                mutation.binding = bound_name.to_string();
                mutation
            })
            .collect::<Vec<_>>();
        self.mutations.extend(copied);
    }
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::super::PythonNamespaceRemainder;
    use super::super::PythonValue;
    use super::super::PythonValueKind;
    use super::BranchConstraints;
    use super::EvaluationState;
    use super::Origin;
    use super::PythonBinding;
    use super::PythonBindingState;
    use super::PythonModuleValues;
    use super::PythonNamespaceCause;
    use super::PythonUnknown;
    use super::PythonUnknownCause;
    use crate::python::PythonModuleName;

    fn test_file(index: u64) -> File {
        File::from_id(Id::from_bits(index + 1))
    }

    fn origin(start: usize) -> Origin {
        Origin::new(test_file(0), Span::saturating_from_parts_usize(start, 1))
    }

    #[test]
    fn named_import_replaces_only_unbound_alternatives_and_preserves_constraints() {
        let branch_origin = origin(10);
        let import_origin = origin(20);
        let mut known_constraints = BranchConstraints::unconstrained();
        known_constraints.select(branch_origin, 0);
        let mut missing_constraints = BranchConstraints::unconstrained();
        missing_constraints.select(branch_origin, 1);

        let mut known = PythonBinding::bound(
            PythonValue::string("known".to_string(), origin(1)),
            origin(1),
        );
        known.select_branch(branch_origin, 0);
        let mut unbound = PythonBinding::unbound();
        unbound.select_branch(branch_origin, 1);

        let mut namespace_cause = PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::ImportNotFound(PythonModuleName::parse("missing").unwrap()),
            [origin(2)],
        ));
        namespace_cause.select_branch(branch_origin, 1);

        let mut values = PythonModuleValues::default();
        values
            .bindings
            .insert("MEMBER".to_string(), known.join(unbound, branch_origin));
        values.namespace_remainder = Some(PythonNamespaceRemainder::new(vec![namespace_cause]));

        let module = PythonModuleName::parse("plugin").unwrap();
        let mut state = EvaluationState::new(test_file(0));
        state.bind_named_import(&module, &values, "MEMBER", "ALIAS", import_origin);

        let binding = state.binding("ALIAS").expect("the alias should be bound");
        assert!(
            !binding
                .alternatives()
                .any(|alternative| *alternative == PythonBindingState::Unbound)
        );

        let mut known_actual = None;
        let mut missing_actual = None;
        let mut namespace_actual = None;
        for (alternative, constraints) in binding.alternatives_with_constraints() {
            let PythonBindingState::Bound(bound) = alternative else {
                continue;
            };
            match &bound.value.kind {
                PythonValueKind::Str(value) if value == "known" => {
                    known_actual = Some(constraints.clone());
                }
                PythonValueKind::Unknown(unknown) => match &unknown.cause {
                    PythonUnknownCause::MissingImportMember { module, member }
                        if module.as_str() == "plugin" && member == "MEMBER" =>
                    {
                        missing_actual = Some(constraints.clone());
                    }
                    PythonUnknownCause::ImportNotFound(module) if module.as_str() == "missing" => {
                        namespace_actual = Some(constraints.clone());
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        assert_eq!(known_actual, Some(known_constraints));
        assert_eq!(missing_actual, Some(missing_constraints.clone()));
        assert_eq!(namespace_actual, Some(missing_constraints));
    }
}
