mod evaluator;
mod model;
mod mutation_target;
mod state;
mod touched_names;

use camino::Utf8PathBuf;
pub(crate) use model::ParseStatus;
pub(crate) use model::PythonDict;
pub(crate) use model::PythonMutationAccess;
pub(crate) use model::PythonSemanticModel;
pub(crate) use model::PythonValue;
pub(crate) use model::PythonValueKind;
use ruff_python_parser::parse_module;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use self::evaluator::PythonSemanticEvaluator;
use self::model::PythonBindings;
use self::state::PythonSemanticState;
use crate::python::PythonImportResolver;
use crate::python::PythonSource;

impl PythonSemanticModel {
    pub(crate) fn analyze(source: &PythonSource, resolver: &mut dyn PythonImportResolver) -> Self {
        PythonSemanticModelAnalysis::default().analyze_source(source, resolver)
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
