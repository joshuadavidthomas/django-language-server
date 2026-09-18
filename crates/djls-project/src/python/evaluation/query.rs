use std::sync::Arc;

use djls_source::FileReadError;
use salsa::Cycle;
use salsa::Id;

use super::PythonImportOutcome;
use super::PythonImportTrace;
use super::PythonModuleEffects;
use super::PythonModuleFacts;
use super::evaluator::PythonModuleEvaluator;
use super::module_object::IntrinsicContamination;
use super::result::EvaluatedPythonModule;
use super::result::PythonModuleEvaluation;
use super::slice::EvaluationDemand;
use super::slice::selected_statements;
use super::touched_names::collect_syntax_impacts;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonSourceModule;
use crate::python::RecoveredPythonModule;

// Salsa tracked-query keys are by-value; `module` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked(
    returns(clone),
    cycle_initial=evaluate_python_module_cycle_initial,
    cycle_fn=evaluate_python_module_cycle_recover,
)]
pub(super) fn evaluate_python_module(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonSourceModule,
    intrinsic_contamination: IntrinsicContamination,
    demand: EvaluationDemand,
) -> PythonModuleEvaluation {
    let file = module.file();
    let parsed = match RecoveredPythonModule::from_file(db, file) {
        Ok(Some(parsed)) => parsed,
        Err(error) => {
            return PythonModuleEvaluation::evaluated(EvaluatedPythonModule::new(
                Err(error),
                PythonImportTrace::rooted(file),
                PythonModuleEffects::default(),
                &module,
            ));
        }
        Ok(None) => {
            return PythonModuleEvaluation::evaluated(EvaluatedPythonModule::new(
                Ok(PythonModuleFacts::default()),
                PythonImportTrace::rooted(file),
                PythonModuleEffects::default(),
                &module,
            ));
        }
    };
    let body = parsed.body(db);
    let syntax_errors = parsed.syntax_errors(db).to_vec();
    let syntax_impacts = collect_syntax_impacts(body, &syntax_errors);
    let (module_facts, import_trace, module_effects) =
        PythonModuleEvaluator::new(db, project, module.clone(), intrinsic_contamination.clone())
            .evaluate(
                selected_statements(body, demand, !syntax_errors.is_empty()),
                syntax_errors,
                syntax_impacts,
            );
    // Imports still request Full. If they reach this root again, use that same fixed point rather
    // than interpreting a finalized cyclic namespace as a fresh, acyclic Settings invocation.
    if demand == EvaluationDemand::Settings
        && import_trace.imports().any(|outcome| {
            matches!(outcome, PythonImportOutcome::Evaluated { edge, .. } if edge.imported == module)
        })
    {
        return evaluate_python_module(db, project, module, intrinsic_contamination, EvaluationDemand::Full);
    }
    PythonModuleEvaluation::evaluated(EvaluatedPythonModule::new(
        Ok(module_facts),
        import_trace,
        module_effects,
        &module,
    ))
}

// This projection gives value consumers an independent red-green cutoff when only import_trace
// change.
#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_facts(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonSourceModule,
    demand: EvaluationDemand,
) -> Result<PythonModuleFacts, FileReadError> {
    match evaluate_python_module(
        db,
        project,
        module,
        IntrinsicContamination::default(),
        demand,
    ) {
        PythonModuleEvaluation::CycleSeed => Ok(PythonModuleFacts::cycle_seed()),
        PythonModuleEvaluation::Evaluated(evaluated) => evaluated.facts().clone(),
    }
}

// This projection gives dependency consumers an independent red-green cutoff when only facts
// change.
#[salsa::tracked(returns(ref))]
pub(crate) fn python_import_trace(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonSourceModule,
    demand: EvaluationDemand,
) -> PythonImportTrace {
    let file = module.file();
    match evaluate_python_module(
        db,
        project,
        module,
        IntrinsicContamination::default(),
        demand,
    ) {
        PythonModuleEvaluation::CycleSeed => PythonImportTrace::rooted(file),
        PythonModuleEvaluation::Evaluated(evaluated) => evaluated.import_trace().clone(),
    }
}

fn evaluate_python_module_cycle_initial(
    _db: &dyn ProjectDb,
    _id: Id,
    _project: Project,
    _module: PythonSourceModule,
    _intrinsic_contamination: IntrinsicContamination,
    _demand: EvaluationDemand,
) -> PythonModuleEvaluation {
    PythonModuleEvaluation::CycleSeed
}

// Salsa requires this callback signature, including each tracked-query key by value.
#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
fn evaluate_python_module_cycle_recover(
    _db: &dyn ProjectDb,
    cycle: &Cycle,
    previous: &PythonModuleEvaluation,
    computed: PythonModuleEvaluation,
    _project: Project,
    module: PythonSourceModule,
    _intrinsic_contamination: IntrinsicContamination,
    _demand: EvaluationDemand,
) -> PythonModuleEvaluation {
    // This is a defensive work budget, not a property of Python import cycles. Widening normally
    // converges in a few passes; twelve preserves the existing budget while staying well below
    // Salsa's own 200-iteration panic.
    const ITERATION_BUDGET: u32 = 12;
    if cycle.iteration() >= ITERATION_BUDGET {
        tracing::warn!(
            iteration = cycle.iteration(),
            "Python module cycle did not converge; using the previous conservative approximation"
        );
        // Returning the previous value makes Salsa recognize this iteration as converged.
        return previous.clone();
    }
    let unchanged = previous == &computed;
    match computed {
        PythonModuleEvaluation::CycleSeed => PythonModuleEvaluation::CycleSeed,
        PythonModuleEvaluation::Evaluated(computed) => {
            let computed = Arc::unwrap_or_clone(computed);
            let evaluated = match previous {
                PythonModuleEvaluation::CycleSeed => computed,
                PythonModuleEvaluation::Evaluated(_) if unchanged => computed,
                PythonModuleEvaluation::Evaluated(previous) => computed.widened(previous, &module),
            };
            PythonModuleEvaluation::evaluated(evaluated)
        }
    }
}
