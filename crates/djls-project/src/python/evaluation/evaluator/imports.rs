use djls_source::FileReadError;

use super::BranchConstraints;
use super::EvaluationState;
use super::Evaluator;
use super::Origin;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonImportOutcome;
use super::PythonModuleDependencies;
use super::PythonModuleValues;
use super::PythonNamespaceCause;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::ast;
use crate::python::PythonImportError;
use crate::python::PythonModuleName;
use crate::python::evaluation::PythonImportEdge;
use crate::python::evaluation::PythonImportEvaluationStatus;
use crate::python::evaluation::PythonModuleObjectId;
use crate::python::evaluation::query::evaluate_python_module;
use crate::python::evaluation::result::CycleMembership;
use crate::python::evaluation::result::PythonModuleEvaluation;
use crate::python::import::DirectImportClause;
use crate::python::import::FromImportSyntax;
use crate::python::module::PythonImportRequest;
use crate::python::module::PythonImportResolutionError;
use crate::python::module::PythonModule;

impl Evaluator<'_> {
    pub(super) fn evaluate_import_statement(&mut self, statement: &ast::Stmt) {
        match statement {
            ast::Stmt::Import(statement) => {
                for clause in DirectImportClause::lower(statement) {
                    self.state.bind_unknown(
                        clause.bound(),
                        &PythonUnknownCause::UnsupportedExpression,
                        self.origin_at(clause.binding_span()),
                    );
                }
            }
            ast::Stmt::ImportFrom(statement) => {
                let import = FromImport::lower(self, statement);
                let result = self
                    .resolve_import(&import)
                    .and_then(|module| self.evaluate_imported_module(module));
                self.state.apply_import(&import, result);
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
        let mut binding = self.state.module_objects.child_binding(object, member, origin);
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
            PythonModuleObjectId::Source(module) => {
                match evaluate_python_module(self.db, self.project, module.clone()) {
                    PythonModuleEvaluation::CycleSeed => {
                        PythonBinding::unknown(&PythonUnknownCause::Cycle, origin)
                    }
                    PythonModuleEvaluation::Evaluated(evaluated) => {
                        let (values, _dependencies, _objects) = evaluated.into_parts();
                        match values {
                            Err(error) => {
                                PythonBinding::unknown(&PythonUnknownCause::Unreadable(error), origin)
                            }
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

    fn resolve_import(&self, import: &FromImport<'_>) -> Result<PythonModule, ImportFailure> {
        let request = PythonImportRequest {
            level: import.level,
            module: import.module,
            importer: &import.importer,
        };
        let module = match PythonModule::resolve_import(self.db, self.project, request) {
            Ok(module) => module,
            Err(PythonImportResolutionError::Invalid(error)) => {
                return Err(ImportFailure::Invalid(error));
            }
            Err(PythonImportResolutionError::NotFound(module)) => {
                return Err(ImportFailure::NotFound(module));
            }
        };
        if !module.search_path().is_project_code() {
            return Err(ImportFailure::SkippedExternal(module.name().clone()));
        }
        Ok(module)
    }

    fn evaluate_imported_module(
        &self,
        module: PythonModule,
    ) -> Result<LoadedImport, ImportFailure> {
        match evaluate_python_module(self.db, self.project, module.clone()) {
            PythonModuleEvaluation::CycleSeed => Err(ImportFailure::Cycle { module }),
            PythonModuleEvaluation::Evaluated(evaluated) => {
                let (values, dependencies, _module_objects) = evaluated.into_parts();
                match values {
                    Ok(values) => Ok(LoadedImport {
                        module,
                        values,
                        dependencies,
                    }),
                    Err(error) => Err(ImportFailure::Unreadable(Box::new(UnreadableImport {
                        module,
                        error,
                        dependencies,
                    }))),
                }
            }
        }
    }
}

struct FromImport<'ast> {
    origin: Origin,
    importer: PythonModule,
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
            importer: evaluator.module.clone(),
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

struct LoadedImport {
    module: PythonModule,
    values: PythonModuleValues,
    dependencies: PythonModuleDependencies,
}

struct UnreadableImport {
    module: PythonModule,
    error: FileReadError,
    dependencies: PythonModuleDependencies,
}

enum ImportFailure {
    Invalid(PythonImportError),
    NotFound(PythonModuleName),
    SkippedExternal(PythonModuleName),
    Cycle { module: PythonModule },
    Unreadable(Box<UnreadableImport>),
}

impl EvaluationState {
    fn apply_import(
        &mut self,
        import: &FromImport<'_>,
        result: Result<LoadedImport, ImportFailure>,
    ) {
        match result {
            Err(failure) => self.apply_import_failure(import, failure),
            Ok(LoadedImport {
                module,
                values,
                dependencies,
            }) => {
                self.dependencies.files.insert(module.file());
                self.absorb_dependencies(&dependencies);
                let module_name = module.name().clone();
                let edge = PythonImportEdge {
                    origin: import.origin,
                    importer: import.importer.clone(),
                    imported: module,
                };
                let status = PythonImportEvaluationStatus::from_syntax_errors(
                    values.syntax_errors.clone(),
                    CycleMembership::Acyclic,
                );
                let outcome = PythonImportOutcome::Evaluated { edge, status };
                self.record_import(outcome);
                match &import.selection {
                    ImportSelection::Star => self.apply_star_import(&values, import.origin),
                    ImportSelection::Named(bindings) => {
                        for binding in bindings {
                            self.bind_named_import(
                                &module_name,
                                &values,
                                binding.imported,
                                binding.bound,
                                binding.origin,
                            );
                        }
                    }
                }
            }
        }
    }

    fn apply_import_failure(&mut self, import: &FromImport<'_>, failure: ImportFailure) {
        let (outcome, cause) = match failure {
            ImportFailure::Invalid(error) => (
                PythonImportOutcome::InvalidImport {
                    origin: import.origin,
                    reason: error.clone(),
                },
                PythonUnknownCause::InvalidImport(error),
            ),
            ImportFailure::NotFound(module) => (
                PythonImportOutcome::NotFound {
                    origin: import.origin,
                    module: module.clone(),
                },
                PythonUnknownCause::ImportNotFound(module),
            ),
            ImportFailure::SkippedExternal(module) => (
                PythonImportOutcome::SkippedExternal {
                    origin: import.origin,
                    module: module.clone(),
                },
                PythonUnknownCause::SkippedExternal(module),
            ),
            ImportFailure::Cycle { module } => {
                self.dependencies.files.insert(module.file());
                (
                    PythonImportOutcome::Evaluated {
                        edge: PythonImportEdge {
                            origin: import.origin,
                            importer: import.importer.clone(),
                            imported: module,
                        },
                        status: PythonImportEvaluationStatus::Cycle {
                            syntax_errors: Vec::new(),
                        },
                    },
                    PythonUnknownCause::Cycle,
                )
            }
            ImportFailure::Unreadable(unreadable) => {
                self.dependencies.files.insert(unreadable.module.file());
                self.absorb_dependencies(&unreadable.dependencies);
                (
                    PythonImportOutcome::Unreadable {
                        edge: PythonImportEdge {
                            origin: import.origin,
                            importer: import.importer.clone(),
                            imported: unreadable.module,
                        },
                        error: unreadable.error.clone(),
                    },
                    PythonUnknownCause::Unreadable(unreadable.error),
                )
            }
        };
        self.record_import(outcome);
        match &import.selection {
            ImportSelection::Star => {
                self.degrade_all_bindings(
                    &cause,
                    import.origin,
                    &BranchConstraints::unconstrained(),
                );
                self.namespace_causes
                    .push(PythonNamespaceCause::unconstrained(PythonUnknown::new(
                        cause,
                        [import.origin],
                    )));
            }
            ImportSelection::Named(bindings) => {
                for binding in bindings {
                    self.bind_unknown(binding.bound, &cause, import.origin);
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
