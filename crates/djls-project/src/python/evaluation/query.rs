use djls_source::FileReadError;
use salsa::Cycle;
use salsa::Id;

use super::PythonModuleDependencies;
use super::PythonModuleObjects;
use super::PythonModuleValues;
use super::evaluator::evaluate_body;
use super::result::EvaluatedPythonModule;
use super::result::PythonModuleEvaluation;
use super::touched_names::collect_syntax_impacts;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::RecoveredPythonModule;

// Salsa tracked-query keys are by-value; `module` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked(
    cycle_initial=evaluate_python_module_cycle_initial,
    cycle_fn=evaluate_python_module_cycle_recover,
)]
pub(super) fn evaluate_python_module(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> PythonModuleEvaluation {
    let file = module.file();
    let parsed = match RecoveredPythonModule::from_file(db, file) {
        Ok(Some(parsed)) => parsed,
        Err(error) => {
            return PythonModuleEvaluation::evaluated(EvaluatedPythonModule::new(
                Err(error),
                PythonModuleDependencies::rooted(file),
                PythonModuleObjects::default(),
                &module,
            ));
        }
        Ok(None) => {
            return PythonModuleEvaluation::evaluated(EvaluatedPythonModule::new(
                Ok(PythonModuleValues::default()),
                PythonModuleDependencies::rooted(file),
                PythonModuleObjects::default(),
                &module,
            ));
        }
    };
    let body = parsed.body(db);
    let (mut module_values, dependencies, module_objects) =
        evaluate_body(db, project, module.clone(), body);
    module_values.syntax_errors = parsed.syntax_errors(db).to_vec();
    module_values.syntax_impacts = collect_syntax_impacts(body, &module_values.syntax_errors);
    PythonModuleEvaluation::evaluated(EvaluatedPythonModule::new(
        Ok(module_values),
        dependencies,
        module_objects,
        &module,
    ))
}

// This projection gives value consumers an independent red-green cutoff when only dependencies
// change.
#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_values(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> Result<PythonModuleValues, FileReadError> {
    match evaluate_python_module(db, project, module) {
        PythonModuleEvaluation::CycleSeed => {
            unreachable!("cycle seed escaped Python module evaluation")
        }
        PythonModuleEvaluation::Evaluated(evaluated) => evaluated.values().clone(),
    }
}

// This projection gives dependency consumers an independent red-green cutoff when only values
// change.
#[salsa::tracked(returns(ref))]
pub(crate) fn python_module_dependencies(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
) -> PythonModuleDependencies {
    match evaluate_python_module(db, project, module) {
        PythonModuleEvaluation::CycleSeed => {
            unreachable!("cycle seed escaped Python module evaluation")
        }
        PythonModuleEvaluation::Evaluated(evaluated) => evaluated.dependencies().clone(),
    }
}

fn evaluate_python_module_cycle_initial(
    _db: &dyn ProjectDb,
    _id: Id,
    _project: Project,
    _module: PythonModule,
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
    module: PythonModule,
) -> PythonModuleEvaluation {
    assert!(
        cycle.iteration() < 12,
        "Python module cycle should converge within twelve iterations"
    );
    let unchanged = previous == &computed;
    let PythonModuleEvaluation::Evaluated(computed) = computed else {
        unreachable!("cycle seed cannot be a computed evaluation")
    };
    let computed = *computed;
    let evaluated = match previous {
        PythonModuleEvaluation::CycleSeed => computed,
        PythonModuleEvaluation::Evaluated(_) if unchanged => computed,
        PythonModuleEvaluation::Evaluated(previous) => computed.widened(previous, &module),
    };
    PythonModuleEvaluation::evaluated(evaluated)
}
