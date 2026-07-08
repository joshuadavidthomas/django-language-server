mod control_flow;
mod evaluator;
mod model;
mod mutation_target;
mod source_graph;
mod state;
mod statement_walk;
mod touched_names;

use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use self::evaluator::EvaluationContext;
use self::evaluator::evaluate_body;
pub(crate) use self::model::ParseStatus;
use self::model::PythonBindings;
pub(crate) use self::model::PythonDict;
pub(crate) use self::model::PythonMutationAccess;
use self::model::PythonMutations;
pub(crate) use self::model::PythonSemanticModel;
pub(crate) use self::model::PythonValue;
pub(crate) use self::model::PythonValueKind;
use self::source_graph::PythonImportEdge;
use self::source_graph::PythonModuleRecord;
use self::source_graph::PythonSourceGraph;
use self::state::PythonSemanticState;
use crate::python::PythonImportResolver;
use crate::python::PythonSource;

impl PythonSemanticModel {
    pub(crate) fn analyze(source: &PythonSource, resolver: &mut dyn PythonImportResolver) -> Self {
        let graph = PythonSourceGraph::collect(source, resolver);
        evaluate_root(&graph)
    }
}

#[derive(Debug, Default)]
pub(super) struct EvaluatedModules {
    visiting: FxHashSet<djls_source::File>,
    completed: FxHashMap<djls_source::File, PythonSemanticModel>,
    cycles: FxHashSet<djls_source::File>,
}

fn evaluate_root(graph: &PythonSourceGraph) -> PythonSemanticModel {
    let mut evaluated = EvaluatedModules::default();
    evaluate_file(graph, &mut evaluated, graph.root())
}

fn evaluate_file(
    graph: &PythonSourceGraph,
    evaluated: &mut EvaluatedModules,
    file: djls_source::File,
) -> PythonSemanticModel {
    if let Some(model) = evaluated.completed.get(&file) {
        return model.clone();
    }
    if !evaluated.visiting.insert(file) {
        evaluated.cycles.insert(file);
        return cycle_edge_model();
    }

    for edge in graph.imports(file) {
        if let Some(imported_file) = edge.resolved_file() {
            evaluate_file(graph, evaluated, imported_file);
        }
    }

    let state = match graph.module(file) {
        Some(PythonModuleRecord::Parsed { source, module }) => {
            let context =
                EvaluationContext::new(source, graph, &evaluated.completed, &evaluated.cycles);
            evaluate_body(&context, PythonSemanticState::default(), &module.body)
        }
        Some(PythonModuleRecord::Unparseable { .. } | PythonModuleRecord::ReadFailed { .. })
        | None => PythonSemanticState::default(),
    };

    let model = finish_model(graph, file, state, evaluated);
    evaluated.visiting.remove(&file);
    evaluated.completed.insert(file, model.clone());
    model
}

fn cycle_edge_model() -> PythonSemanticModel {
    PythonSemanticModel {
        bindings: PythonBindings::default(),
        files_read: Vec::new(),
        source_paths: FxHashMap::default(),
        read_failures: Vec::new(),
        mutations: PythonMutations::default(),
        status: ParseStatus::Unparseable,
    }
}

fn finish_model(
    graph: &PythonSourceGraph,
    file: djls_source::File,
    state: PythonSemanticState,
    evaluated: &EvaluatedModules,
) -> PythonSemanticModel {
    let mut files_read = Vec::new();
    let mut seen_files = FxHashSet::default();
    let mut source_paths = FxHashMap::default();
    let mut read_failures = Vec::new();
    let mut status = ParseStatus::Parsed;

    if let Some(record) = graph.module(file) {
        push_file_once(&mut files_read, &mut seen_files, record.file());
        source_paths.insert(record.file(), record.path().to_path_buf());
        if let Some(record_status) = record.parse_status() {
            status = status.join(record_status);
        }
    }

    for edge in graph.imports(file) {
        match edge {
            PythonImportEdge::Resolved { file, .. } => {
                if let Some(imported_model) = evaluated.completed.get(file) {
                    for imported_file in &imported_model.files_read {
                        push_file_once(&mut files_read, &mut seen_files, *imported_file);
                    }
                    source_paths.extend(imported_model.source_paths.clone());
                    for (file, path) in imported_model.read_failures() {
                        push_read_failure_once(&mut read_failures, *file, path.clone());
                    }
                    status = status.join(imported_model.status);
                } else if evaluated.cycles.contains(file)
                    && let Some(record) = graph.module(*file)
                {
                    push_file_once(&mut files_read, &mut seen_files, record.file());
                    source_paths.insert(record.file(), record.path().to_path_buf());
                    if let Some(record_status) = record.parse_status() {
                        status = status.join(record_status);
                    }
                }
            }
            PythonImportEdge::ReadFailed { file, path, .. } => {
                push_file_once(&mut files_read, &mut seen_files, *file);
                source_paths.insert(*file, path.clone());
                push_read_failure_once(&mut read_failures, *file, path.clone());
            }
            PythonImportEdge::Unresolved { .. } | PythonImportEdge::SkippedExternal { .. } => {}
        }
    }

    PythonSemanticModel {
        bindings: state.bindings,
        files_read,
        source_paths,
        read_failures,
        mutations: state.mutations,
        status,
    }
}

fn push_read_failure_once(
    read_failures: &mut Vec<(djls_source::File, camino::Utf8PathBuf)>,
    file: djls_source::File,
    path: camino::Utf8PathBuf,
) {
    if !read_failures
        .iter()
        .any(|(existing_file, existing_path)| *existing_file == file && *existing_path == path)
    {
        read_failures.push((file, path));
    }
}

fn push_file_once(
    files_read: &mut Vec<djls_source::File>,
    seen_files: &mut FxHashSet<djls_source::File>,
    file: djls_source::File,
) {
    if seen_files.insert(file) {
        files_read.push(file);
    }
}
