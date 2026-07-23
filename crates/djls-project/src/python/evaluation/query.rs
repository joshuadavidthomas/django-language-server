use djls_source::FileReadError;
use salsa::Cycle;
use salsa::Id;

use super::PythonImportTrace;
use super::PythonModuleEffects;
use super::PythonModuleFacts;
use super::evaluator::evaluate_body;
use super::module_object::IntrinsicContamination;
use super::result::EvaluatedPythonModule;
use super::result::PythonModuleEvaluation;
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
    let (module_facts, import_trace, module_effects) = evaluate_body(
        db,
        project,
        module.clone(),
        body,
        syntax_errors,
        syntax_impacts,
        intrinsic_contamination,
    );
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
) -> Result<PythonModuleFacts, FileReadError> {
    match evaluate_python_module(db, project, module, IntrinsicContamination::default()) {
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
) -> PythonImportTrace {
    let file = module.file();
    match evaluate_python_module(db, project, module, IntrinsicContamination::default()) {
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
) -> PythonModuleEvaluation {
    PythonModuleEvaluation::CycleSeed
}

// Salsa requires cycle recovery callbacks to accept the tracked-query keys by value.
#[allow(clippy::needless_pass_by_value)]
fn evaluate_python_module_cycle_recover(
    _db: &dyn ProjectDb,
    cycle: &Cycle,
    previous: &PythonModuleEvaluation,
    computed: PythonModuleEvaluation,
    _project: Project,
    module: PythonSourceModule,
    _intrinsic_contamination: IntrinsicContamination,
) -> PythonModuleEvaluation {
    assert!(
        cycle.iteration() < 12,
        "Python module cycle should converge within twelve iterations"
    );
    let unchanged = previous == &computed;
    match computed {
        PythonModuleEvaluation::CycleSeed => PythonModuleEvaluation::CycleSeed,
        PythonModuleEvaluation::Evaluated(computed) => {
            let computed = *computed;
            let evaluated = match previous {
                PythonModuleEvaluation::CycleSeed => computed,
                PythonModuleEvaluation::Evaluated(_) if unchanged => computed,
                PythonModuleEvaluation::Evaluated(previous) => computed.widened(previous, &module),
            };
            PythonModuleEvaluation::evaluated(evaluated)
        }
    }
}
