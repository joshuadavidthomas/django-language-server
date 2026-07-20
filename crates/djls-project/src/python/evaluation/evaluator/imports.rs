use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;
use djls_source::FileReadError;

use super::BranchConstraints;
use super::EvaluationState;
use super::Evaluator;
use super::Origin;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonImportFallback;
use super::PythonImportOutcome;
use super::PythonModuleDependencies;
use super::PythonModuleObjects;
use super::PythonModuleValues;
use super::PythonNamespaceCause;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ast;
use crate::python::PythonModuleName;
use crate::python::evaluation::PythonImportEdge;
use crate::python::evaluation::PythonImportEvaluationStatus;
use crate::python::evaluation::PythonModuleObjectId;
use crate::python::evaluation::PythonSequenceAlternativeRef;
use crate::python::evaluation::PythonSequenceItem;
use crate::python::evaluation::StructuralOrd;
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
        self.project_loaded_module_member(object, None, member, origin)
    }

    /// Project a member of a module whose terminal source values may already be
    /// available from the chain loader. Reusing those values avoids a recursive
    /// query while the same module is still being applied; expression reads pass
    /// `None` and use the canonical core query instead.
    fn project_loaded_module_member(
        &self,
        object: &PythonModuleObjectId,
        values: Option<&PythonModuleValues>,
        member: &str,
        origin: Origin,
    ) -> PythonBinding {
        let mut binding = self
            .state
            .module_objects
            .child_binding(object, member, origin);
        if binding
            .alternatives()
            .any(|state| *state == PythonBindingState::Unbound)
        {
            let fallback = values.map_or_else(
                || self.project_intrinsic_source_member(object, member, origin),
                |values| Self::project_source_member(values, member, origin),
            );
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
                            Ok(values) => Self::project_source_member(&values, member, origin),
                        }
                    }
                }
            }
            PythonModuleObjectId::Namespace(_) => PythonBinding::unbound(),
        }
    }

    fn project_source_member(
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
                let load =
                    self.load_import_chain(&name, resolution, clause_origin, ChainLoadMode::Full);
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

    /// Evaluate a `from ... import ...` statement through the shared chain, then
    /// apply named or star selection against the loaded terminal object.
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
                let load =
                    self.load_import_chain(&name, resolution, import.origin, ChainLoadMode::Full);
                match load.leaf {
                    ChainOutcome::Source { object, values } => self.apply_from_selection(
                        &import,
                        FromImportSource {
                            object: &object,
                            values: Some(&values),
                        },
                    ),
                    ChainOutcome::Namespace { object }
                    | ChainOutcome::Cycle { object }
                    | ChainOutcome::External { object, .. } => self.apply_from_selection(
                        &import,
                        FromImportSource {
                            object: &object,
                            values: None,
                        },
                    ),
                    ChainOutcome::NotFound { module } => self
                        .state
                        .apply_from_failure(&import, &PythonUnknownCause::ImportNotFound(module)),
                    ChainOutcome::Unreadable { error, .. } => self
                        .state
                        .apply_from_failure(&import, &PythonUnknownCause::Unreadable(error)),
                }
            }
        }
    }

    fn apply_from_selection(&mut self, import: &FromImport<'_>, source: FromImportSource<'_>) {
        match &import.selection {
            ImportSelection::Star => self.apply_star_from_selection(source, import.origin),
            ImportSelection::Named(bindings) => {
                for imported in bindings {
                    let binding = self.project_import_member(
                        source,
                        imported.imported,
                        imported.origin,
                        import.origin,
                        ImportMemberMode::Named,
                    );
                    self.state
                        .assign_binding(imported.bound, binding, imported.origin);
                    if let Some(values) = source.values {
                        self.state.copy_imported_mutations(
                            values,
                            imported.imported,
                            imported.bound,
                            None,
                        );
                    }
                }
            }
        }
    }

    /// Project one explicitly selected member and apply the existing exact-child
    /// fallback. The selection mode keeps named and branch-correlated star paths
    /// coherent instead of passing independent optional constraints.
    fn project_import_member(
        &mut self,
        source: FromImportSource<'_>,
        member: &str,
        binding_origin: Origin,
        import_origin: Origin,
        mode: ImportMemberMode<'_>,
    ) -> PythonBinding {
        let projected =
            self.project_loaded_module_member(source.object, source.values, member, binding_origin);
        let (selected, preserved) = match mode {
            ImportMemberMode::Named => (None, None),
            ImportMemberMode::Star {
                selected,
                preserved,
            } => (Some(selected), preserved),
        };
        let mut binding = selected
            .map_or(Some(projected.clone()), |constraints| {
                projected.intersect_constraints(constraints)
            })
            .unwrap_or_else(PythonBinding::unbound);

        if source.object.is_package()
            && let Some(fallback_policy) = PythonImportFallback::new(&binding, preserved)
            && let Some(child_name) = source.object.name().exact_child(member)
        {
            let request = PythonImportRequest {
                level: 0,
                module: Some(child_name.as_str()),
                importer: &self.module,
            };
            let (_resolved_name, resolution) =
                PythonModule::resolve_import_chain(self.db, self.project, request)
                    .expect("an exact child request is a valid absolute import");
            let child = self.load_import_chain(
                &child_name,
                resolution,
                import_origin,
                ChainLoadMode::ChildFallback {
                    parent: source.object,
                    fallback: &fallback_policy,
                },
            );
            let fallback = match child.leaf {
                ChainOutcome::Source { object, .. }
                | ChainOutcome::Namespace { object }
                | ChainOutcome::Cycle { object }
                | ChainOutcome::External { object, .. }
                    if child.leaf_reached =>
                {
                    Some(PythonBinding::bound(
                        PythonValue::module(object, binding_origin),
                        binding_origin,
                    ))
                }
                ChainOutcome::Unreadable { error } => Some(PythonBinding::unknown(
                    &PythonUnknownCause::Unreadable(error),
                    binding_origin,
                )),
                ChainOutcome::NotFound { .. }
                | ChainOutcome::Source { .. }
                | ChainOutcome::Namespace { .. }
                | ChainOutcome::Cycle { .. }
                | ChainOutcome::External { .. } => None,
            };
            if let Some(fallback) = fallback {
                binding = binding
                    .replace_unbound_with(Some(fallback.clone()), binding_origin)
                    .join_fallback_on_cycle(&fallback, binding_origin);
            }
        }

        if matches!(mode, ImportMemberMode::Named) {
            let missing = PythonUnknownCause::MissingImportMember {
                module: source.object.name().clone(),
                member: member.to_string(),
            };
            binding.replace_unbound_with(
                Some(PythonBinding::unknown(&missing, binding_origin)),
                binding_origin,
            )
        } else {
            binding
        }
    }

    fn apply_star_from_selection(&mut self, source: FromImportSource<'_>, import_origin: Origin) {
        let all = self.project_loaded_module_member(
            source.object,
            source.values,
            "__all__",
            import_origin,
        );
        let alternatives = classify_star_all(&all);
        let caller_bindings = self.state.bindings.clone();
        let current_names = source
            .values
            .into_iter()
            .flat_map(|values| values.bindings.keys().map(String::as_str))
            .chain(self.state.module_objects.child_names(source.object))
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let plan = StarSelectionPlan::new(&alternatives, &current_names);

        // Resolve each exact alternative in its own declared order. Child
        // effects are constrained to that arm while every other alternative
        // preserves the pre-import object coordinates.
        let mut exact_bindings = BTreeMap::<String, PythonBinding>::new();
        for arm in &plan.exact_arms {
            let preserved = plan.preserved_for_exact(arm);
            for name in &arm.names {
                let binding = self.project_import_member(
                    source,
                    name,
                    import_origin,
                    import_origin,
                    ImportMemberMode::Star {
                        selected: &arm.constraints,
                        preserved: preserved.as_ref(),
                    },
                );
                if let Some(existing) = exact_bindings.get_mut(name) {
                    *existing = existing.clone().join(binding, import_origin);
                } else {
                    exact_bindings.insert(name.clone(), binding);
                }
            }
        }

        for (name, name_paths) in &plan.paths {
            self.apply_star_name(
                source,
                name,
                name_paths,
                caller_bindings.get(name),
                exact_bindings.get(name),
                import_origin,
            );
        }

        // Dynamic alternatives can export additional names that are not in the
        // current finite namespace. Preserve every caller binding, add the
        // constrained overwrite possibility, and retain the same rebased cause
        // as the namespace remainder.
        for alternative in alternatives {
            let StarAllAlternative::Dynamic { cause, constraints } = alternative else {
                continue;
            };
            self.state
                .degrade_all_bindings(&cause, import_origin, &constraints);
            self.state
                .namespace_causes
                .push(PythonNamespaceCause::constrained(
                    PythonUnknown::new(cause, [import_origin]),
                    constraints,
                ));
        }
    }

    fn apply_star_name(
        &mut self,
        source: FromImportSource<'_>,
        name: &str,
        paths: &StarNamePaths,
        prior: Option<&PythonBinding>,
        exact: Option<&PythonBinding>,
        origin: Origin,
    ) {
        let caller_had_name = prior.is_some();
        let prior = prior.cloned().unwrap_or_else(PythonBinding::unbound);
        let mut joined = None;
        let mut selected_imported = None;
        let mut exact_preserves_prior = false;
        let mut absent_preserves_prior = false;
        if let Some(exact) = exact {
            exact_preserves_prior = exact.import_fallback_constraints().is_some();
            join_star_binding(&mut selected_imported, exact.clone(), origin);
            let missing = PythonBinding::unknown(
                &PythonUnknownCause::MissingImportMember {
                    module: source.object.name().clone(),
                    member: name.to_string(),
                },
                origin,
            );
            let fallback = prior
                .clone()
                .replace_unbound_with(Some(missing.clone()), origin);
            let exact = exact
                .clone()
                .replace_unbound_with(Some(fallback.clone()), origin)
                .join_fallback_on_cycle(&fallback, origin);
            join_star_binding(&mut joined, exact, origin);
        }
        if let Some(constraints) = &paths.absent
            && let Some(projected) = self
                .project_loaded_module_member(source.object, source.values, name, origin)
                .intersect_constraints(constraints)
        {
            absent_preserves_prior = projected
                .alternatives()
                .any(|state| *state == PythonBindingState::Unbound);
            join_star_binding(&mut selected_imported, projected.clone(), origin);
            let selected = projected.replace_unbound_with(Some(prior.clone()), origin);
            join_star_binding(&mut joined, selected, origin);
        }
        if let Some(constraints) = &paths.dynamic {
            if let Some(projected) = self
                .project_loaded_module_member(source.object, source.values, name, origin)
                .intersect_constraints(constraints)
            {
                join_star_binding(&mut joined, projected, origin);
            }
            if let Some(prior) = prior.clone().intersect_constraints(constraints) {
                join_star_binding(&mut joined, prior, origin);
            }
        }
        if let Some(constraints) = &paths.omitted
            && let Some(prior) = prior.clone().intersect_constraints(constraints)
        {
            join_star_binding(&mut joined, prior, origin);
        }

        let selected = paths.exact.is_some() || paths.absent.is_some() || paths.dynamic.is_some();
        if let Some(binding) = joined
            && (selected || caller_had_name)
        {
            self.state.bindings.insert(name.to_string(), binding);
        }

        let selected_on_some_path = paths.exact.is_some() || paths.absent.is_some();
        let prior_survives = exact_preserves_prior
            || absent_preserves_prior
            || paths.dynamic.is_some()
            || paths.omitted.is_some();
        if selected_on_some_path && !prior_survives {
            self.state
                .mutations
                .retain(|mutation| mutation.binding != name);
        }
        if let Some(values) = source.values
            && let Some(selected_imported) = selected_imported.as_ref()
        {
            self.state
                .copy_imported_mutations(values, name, name, Some(selected_imported));
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
        mode: ChainLoadMode<'_>,
    ) -> ChainLoad {
        let (mut components, failure) = match resolution {
            PythonImportChainResolution::Resolved(chain) => (chain.into_components(), None),
            PythonImportChainResolution::Failed { prefix, failure } => {
                (prefix.into_components(), Some(failure))
            }
        };
        let mut progress = ChainLoadProgress::start(&mut components, mode);
        let resolved_len = components.len();

        for (index, component) in components.into_iter().enumerate() {
            let is_last = failure.is_none() && index + 1 == resolved_len;
            let (attribute, object) = component_identity(&component);

            if progress.external {
                self.load_external_suffix_component(
                    name,
                    &attribute,
                    object,
                    is_last,
                    origin,
                    &mut progress,
                );
                continue;
            }

            match component {
                ResolvedChainComponent::Namespace(_) => {
                    self.state.attach_component(
                        progress.parent.as_ref(),
                        &attribute,
                        &object,
                        origin,
                        progress.terminal_fallback(is_last),
                    );
                    progress.root.get_or_insert_with(|| object.clone());
                    progress.parent = Some(object.clone());
                    if is_last {
                        progress.leaf_reached = true;
                        progress.terminal = Some(ChainOutcome::Namespace { object });
                    }
                }
                ResolvedChainComponent::Source(module)
                    if !module.search_path().is_project_code() =>
                {
                    progress.external = true;
                    self.record_external_outcome(name, origin, &mut progress);
                    self.state.attach_external_component(
                        progress.parent.as_ref(),
                        &attribute,
                        &object,
                        name,
                        origin,
                        progress.terminal_fallback(is_last),
                    );
                    progress.root.get_or_insert_with(|| object.clone());
                    progress.parent = Some(object.clone());
                    if is_last {
                        progress.leaf_reached = true;
                        progress.terminal = Some(ChainOutcome::External {
                            object,
                            module: name.clone(),
                        });
                    }
                }
                ResolvedChainComponent::Source(module)
                    if !is_last && self.is_importer_self(&module) =>
                {
                    self.state.attach_component(
                        progress.parent.as_ref(),
                        &attribute,
                        &object,
                        origin,
                        None,
                    );
                    progress.root.get_or_insert_with(|| object.clone());
                    progress.parent = Some(object);
                }
                ResolvedChainComponent::Source(module) => {
                    if self.load_project_source_component(
                        &module,
                        &attribute,
                        object,
                        is_last,
                        origin,
                        &mut progress,
                    ) {
                        break;
                    }
                }
            }
        }

        let leaf = progress
            .terminal
            .unwrap_or_else(|| self.record_chain_not_found(failure, name, origin));
        ChainLoad {
            root: progress.root,
            leaf,
            leaf_reached: progress.leaf_reached,
        }
    }

    fn load_external_suffix_component(
        &mut self,
        name: &PythonModuleName,
        attribute: &str,
        object: PythonModuleObjectId,
        is_last: bool,
        origin: Origin,
        progress: &mut ChainLoadProgress,
    ) {
        self.record_external_outcome(name, origin, progress);
        self.state.attach_external_component(
            progress.parent.as_ref(),
            attribute,
            &object,
            name,
            origin,
            progress.terminal_fallback(is_last),
        );
        progress.root.get_or_insert_with(|| object.clone());
        progress.parent = Some(object.clone());
        if is_last {
            progress.leaf_reached = true;
            progress.terminal = Some(ChainOutcome::External {
                object,
                module: name.clone(),
            });
        }
    }

    fn record_external_outcome(
        &mut self,
        name: &PythonModuleName,
        origin: Origin,
        progress: &mut ChainLoadProgress,
    ) {
        if !progress.external_outcome_recorded {
            self.state
                .record_import(PythonImportOutcome::SkippedExternal {
                    origin,
                    module: name.clone(),
                });
            progress.external_outcome_recorded = true;
        }
    }

    /// Apply one project-source component to the chain cursor. Returns `true`
    /// when the component terminates traversal.
    fn load_project_source_component(
        &mut self,
        module: &PythonModule,
        attribute: &str,
        object: PythonModuleObjectId,
        is_last: bool,
        origin: Origin,
        progress: &mut ChainLoadProgress,
    ) -> bool {
        match self.evaluate_source_component(module, origin) {
            SourceComponent::Cycle => {
                self.state.attach_component(
                    progress.parent.as_ref(),
                    attribute,
                    &object,
                    origin,
                    progress.terminal_fallback(is_last),
                );
                self.state.open_component_cycle(
                    &object,
                    origin,
                    progress.terminal_fallback(is_last),
                );
                progress.root.get_or_insert_with(|| object.clone());
                progress.leaf_reached = is_last;
                progress.terminal = Some(ChainOutcome::Cycle { object });
                true
            }
            SourceComponent::Unreadable(error) => {
                progress.leaf_reached = is_last;
                progress.terminal = Some(ChainOutcome::Unreadable { error });
                true
            }
            SourceComponent::Evaluated(values, objects) => {
                self.state.module_objects_merge(
                    objects,
                    origin,
                    progress.terminal_fallback(is_last),
                );
                self.state.attach_component(
                    progress.parent.as_ref(),
                    attribute,
                    &object,
                    origin,
                    progress.terminal_fallback(is_last),
                );
                progress.root.get_or_insert_with(|| object.clone());
                progress.parent = Some(object.clone());
                if is_last {
                    progress.leaf_reached = true;
                    progress.terminal = Some(ChainOutcome::Source { object, values });
                }
                false
            }
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

#[derive(Clone)]
enum StarAllAlternative {
    Exact {
        names: Vec<String>,
        constraints: BranchConstraints,
    },
    Absent {
        constraints: BranchConstraints,
    },
    Dynamic {
        cause: PythonUnknownCause,
        constraints: BranchConstraints,
    },
}

impl StarAllAlternative {
    fn constraints(&self) -> &BranchConstraints {
        match self {
            Self::Exact { constraints, .. }
            | Self::Absent { constraints }
            | Self::Dynamic { constraints, .. } => constraints,
        }
    }
}

#[derive(Default)]
struct StarNamePaths {
    exact: Option<BranchConstraints>,
    absent: Option<BranchConstraints>,
    dynamic: Option<BranchConstraints>,
    omitted: Option<BranchConstraints>,
}

struct StarExactArm {
    alternative_index: usize,
    names: Vec<String>,
    constraints: BranchConstraints,
}

struct StarSelectionPlan {
    exact_arms: Vec<StarExactArm>,
    paths: BTreeMap<String, StarNamePaths>,
    alternative_constraints: Vec<BranchConstraints>,
}

impl StarSelectionPlan {
    fn new(alternatives: &[StarAllAlternative], current_names: &BTreeSet<String>) -> Self {
        let mut names = current_names.clone();
        let mut exact_arms = alternatives
            .iter()
            .enumerate()
            .filter_map(|(alternative_index, alternative)| match alternative {
                StarAllAlternative::Exact { names, constraints } => Some(StarExactArm {
                    alternative_index,
                    names: names.clone(),
                    constraints: constraints.clone(),
                }),
                StarAllAlternative::Absent { .. } | StarAllAlternative::Dynamic { .. } => None,
            })
            .collect::<Vec<_>>();
        exact_arms.sort_by(|left, right| left.constraints.structural_cmp(&right.constraints));
        for arm in &exact_arms {
            names.extend(arm.names.iter().cloned());
        }

        let mut paths = BTreeMap::new();
        for name in names {
            let mut name_paths = StarNamePaths::default();
            for alternative in alternatives {
                match alternative {
                    StarAllAlternative::Exact {
                        names: listed,
                        constraints,
                    } if listed.contains(&name) => name_paths.merge_exact(constraints.clone()),
                    StarAllAlternative::Absent { constraints }
                        if current_names.contains(&name) && !name.starts_with('_') =>
                    {
                        name_paths.merge_absent(constraints.clone());
                    }
                    StarAllAlternative::Dynamic { constraints, .. }
                        if current_names.contains(&name) =>
                    {
                        name_paths.merge_dynamic(constraints.clone());
                    }
                    StarAllAlternative::Exact { constraints, .. }
                    | StarAllAlternative::Absent { constraints }
                    | StarAllAlternative::Dynamic { constraints, .. } => {
                        name_paths.merge_omitted(constraints.clone());
                    }
                }
            }
            paths.insert(name, name_paths);
        }
        let alternative_constraints = alternatives
            .iter()
            .map(StarAllAlternative::constraints)
            .cloned()
            .collect();
        Self {
            exact_arms,
            paths,
            alternative_constraints,
        }
    }

    fn preserved_for_exact(&self, exact: &StarExactArm) -> Option<BranchConstraints> {
        let mut preserved = None;
        for (index, constraints) in self.alternative_constraints.iter().enumerate() {
            if index != exact.alternative_index {
                merge_star_constraints(&mut preserved, constraints.clone());
            }
        }
        preserved
    }
}

impl StarNamePaths {
    fn merge_exact(&mut self, constraints: BranchConstraints) {
        merge_star_constraints(&mut self.exact, constraints);
    }

    fn merge_absent(&mut self, constraints: BranchConstraints) {
        merge_star_constraints(&mut self.absent, constraints);
    }

    fn merge_dynamic(&mut self, constraints: BranchConstraints) {
        merge_star_constraints(&mut self.dynamic, constraints);
    }

    fn merge_omitted(&mut self, constraints: BranchConstraints) {
        merge_star_constraints(&mut self.omitted, constraints);
    }
}

fn merge_star_constraints(merged: &mut Option<BranchConstraints>, constraints: BranchConstraints) {
    if constraints.is_impossible() {
        return;
    }
    if let Some(merged) = merged {
        merged.merge(constraints);
    } else {
        *merged = Some(constraints);
    }
}

fn join_star_binding(joined: &mut Option<PythonBinding>, binding: PythonBinding, origin: Origin) {
    *joined = Some(match joined.take() {
        Some(current) => current.join(binding, origin),
        None => binding,
    });
}

fn classify_star_all(binding: &PythonBinding) -> Vec<StarAllAlternative> {
    let mut alternatives = Vec::new();
    for (state, binding_constraints) in binding.alternatives_with_constraints() {
        match state {
            PythonBindingState::Unbound => {
                alternatives.push(StarAllAlternative::Absent {
                    constraints: binding_constraints.clone(),
                });
            }
            PythonBindingState::Bound(bound) => {
                let Some(sequence) = bound
                    .value
                    .sequence()
                    .and_then(super::super::sequence::PythonSequence::materialized)
                else {
                    alternatives.push(StarAllAlternative::Dynamic {
                        cause: bound
                            .value
                            .unknown_value()
                            .map_or(PythonUnknownCause::UnsupportedExpression, |unknown| {
                                unknown.cause.clone()
                            }),
                        constraints: binding_constraints.clone(),
                    });
                    continue;
                };
                for alternative in sequence.alternatives() {
                    match alternative {
                        PythonSequenceAlternativeRef::Exact { items, constraints } => {
                            let constraints = binding_constraints.intersection(constraints);
                            if constraints.is_impossible() {
                                continue;
                            }
                            let mut names = Vec::new();
                            let mut dynamic = None;
                            for item in items {
                                match item {
                                    PythonSequenceItem::Value(PythonValue {
                                        kind: PythonValueKind::Str(name),
                                        ..
                                    }) => {
                                        if !names.iter().any(|existing| existing == name) {
                                            names.push(name.clone());
                                        }
                                    }
                                    PythonSequenceItem::UnknownElement(unknown)
                                    | PythonSequenceItem::UnknownUnpack(unknown) => {
                                        dynamic = Some(unknown.cause.clone());
                                        break;
                                    }
                                    PythonSequenceItem::Value(_) => {
                                        dynamic = Some(PythonUnknownCause::UnsupportedExpression);
                                        break;
                                    }
                                }
                            }
                            alternatives.push(match dynamic {
                                Some(cause) => StarAllAlternative::Dynamic { cause, constraints },
                                None => StarAllAlternative::Exact { names, constraints },
                            });
                        }
                        PythonSequenceAlternativeRef::Remainder { constraints, .. } => {
                            let constraints = binding_constraints.intersection(constraints);
                            if !constraints.is_impossible() {
                                alternatives.push(StarAllAlternative::Dynamic {
                                    cause: PythonUnknownCause::AlternativeLimitExceeded,
                                    constraints,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    let dynamic_constraints = alternatives
        .iter()
        .filter_map(|alternative| match alternative {
            StarAllAlternative::Dynamic { constraints, .. } => Some(constraints.clone()),
            StarAllAlternative::Exact { .. } | StarAllAlternative::Absent { .. } => None,
        })
        .collect::<Vec<_>>();
    alternatives.retain(|alternative| match alternative {
        StarAllAlternative::Absent { constraints } => !dynamic_constraints
            .iter()
            .any(|dynamic| dynamic == constraints),
        StarAllAlternative::Exact { .. } | StarAllAlternative::Dynamic { .. } => true,
    });
    alternatives
}

fn component_object(component: &ResolvedChainComponent) -> PythonModuleObjectId {
    match component {
        ResolvedChainComponent::Source(module) => PythonModuleObjectId::Source(module.clone()),
        ResolvedChainComponent::Namespace(package) => {
            PythonModuleObjectId::Namespace(package.clone())
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
    let object = component_object(component);
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

#[derive(Clone, Copy)]
enum ChainLoadMode<'a> {
    Full,
    ChildFallback {
        parent: &'a PythonModuleObjectId,
        fallback: &'a PythonImportFallback,
    },
}

#[derive(Default)]
struct ChainLoadProgress {
    root: Option<PythonModuleObjectId>,
    parent: Option<PythonModuleObjectId>,
    terminal: Option<ChainOutcome>,
    terminal_fallback: Option<PythonImportFallback>,
    leaf_reached: bool,
    external: bool,
    external_outcome_recorded: bool,
}

impl ChainLoadProgress {
    fn start(components: &mut Vec<ResolvedChainComponent>, mode: ChainLoadMode<'_>) -> Self {
        let ChainLoadMode::ChildFallback { parent, fallback } = mode else {
            return Self::default();
        };
        let root = components.first().map(component_object);
        let prefix_index = components
            .iter()
            .position(|component| component_object(component) == *parent)
            .expect("an exact child chain retains its already-loaded parent prefix");
        components.drain(..=prefix_index);
        Self {
            root,
            parent: Some(parent.clone()),
            terminal_fallback: Some(fallback.clone()),
            external: matches!(
                parent,
                PythonModuleObjectId::Source(module) if !module.search_path().is_project_code()
            ),
            ..Self::default()
        }
    }

    fn terminal_fallback(&self, is_last: bool) -> Option<&PythonImportFallback> {
        is_last.then_some(self.terminal_fallback.as_ref()).flatten()
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

#[derive(Clone, Copy)]
struct FromImportSource<'a> {
    object: &'a PythonModuleObjectId,
    values: Option<&'a PythonModuleValues>,
}

#[derive(Clone, Copy)]
enum ImportMemberMode<'a> {
    Named,
    Star {
        selected: &'a BranchConstraints,
        preserved: Option<&'a BranchConstraints>,
    },
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
        fallback: Option<&PythonImportFallback>,
    ) {
        if let Some(parent) = parent {
            if let Some(fallback) = fallback {
                self.module_objects.attach_child_for_import_fallback(
                    parent.clone(),
                    attribute.to_string(),
                    object,
                    fallback,
                    origin,
                );
            } else {
                self.module_objects.attach_child(
                    parent.clone(),
                    attribute.to_string(),
                    object.clone(),
                    origin,
                );
            }
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
        fallback: Option<&PythonImportFallback>,
    ) {
        self.attach_component(parent, attribute, object, origin, fallback);
        let unknown = PythonUnknown::new(
            PythonUnknownCause::SkippedExternal(module.clone()),
            [origin],
        );
        let cause = Self::object_cause(unknown, fallback);
        self.module_objects.open_cause(object.clone(), cause);
    }

    /// Mark a cycle-seed component's object open with a `Cycle` cause so reads of
    /// its attributes become cycle unknowns.
    fn open_component_cycle(
        &mut self,
        object: &PythonModuleObjectId,
        origin: Origin,
        fallback: Option<&PythonImportFallback>,
    ) {
        let unknown = PythonUnknown::new(PythonUnknownCause::Cycle, [origin]);
        let cause = Self::object_cause(unknown, fallback);
        self.module_objects.open_cause(object.clone(), cause);
    }

    fn object_cause(
        unknown: PythonUnknown,
        fallback: Option<&PythonImportFallback>,
    ) -> PythonNamespaceCause {
        match fallback {
            Some(fallback) => {
                PythonNamespaceCause::constrained(unknown, fallback.constraints().clone())
            }
            None => PythonNamespaceCause::unconstrained(unknown),
        }
    }

    /// Merge a loaded component's own child effects into this importer's object
    /// state in source order, restricting a fallback module to its selected
    /// branch paths while preserving all paths carried by the fallback value.
    fn module_objects_merge(
        &mut self,
        objects: PythonModuleObjects,
        origin: Origin,
        fallback: Option<&PythonImportFallback>,
    ) {
        if let Some(fallback) = fallback {
            self.module_objects
                .merge_for_import_fallback(objects, fallback, origin);
        } else {
            self.module_objects.merge(objects, origin);
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

    fn copy_imported_mutations(
        &mut self,
        values: &PythonModuleValues,
        imported_name: &str,
        bound_name: &str,
        selected: Option<&PythonBinding>,
    ) {
        self.mutations.extend(
            values
                .mutations
                .iter()
                .filter(|mutation| {
                    mutation.binding == imported_name
                        && selected
                            .is_none_or(|binding| binding.contains_feasible_origin(mutation.origin))
                })
                .cloned()
                .map(|mut mutation| {
                    mutation.binding = bound_name.to_string();
                    mutation
                }),
        );
    }
}
