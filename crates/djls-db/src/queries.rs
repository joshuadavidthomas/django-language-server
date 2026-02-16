use djls_project::build_search_paths;
use djls_project::Project;
use djls_python::ModelGraph;
use djls_python::ModulePath;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;

/// Compute `TagSpecs` from extraction results.
///
/// This tracked function reads `project.template_libraries(db)` and
/// `project.extracted_external_rules(db)` to establish Salsa dependencies.
/// It starts from empty specs and populates purely from extraction results
/// (both workspace modules via tracked queries and external modules from
/// the Project field).
///
/// Does NOT read from `Arc<Mutex<Settings>>`.
#[salsa::tracked]
pub fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
    let _libraries = project.template_libraries(db);
    let tagspecs = project.tagspecs(db);

    // Start from Django's builtin tag specs as a base
    let mut specs = djls_semantic::builtin_tag_specs();

    // Merge workspace extraction results (tracked, auto-invalidating on file change)
    let workspace_results = collect_workspace_extraction_results(db, project);
    for (_module_path, extraction) in &workspace_results {
        specs.merge_extraction_results(extraction);
    }

    // Merge external extraction results (from Project field, updated by refresh_external_data)
    for extraction in project.extracted_external_rules(db).values() {
        specs.merge_extraction_results(extraction);
    }

    // Fill extraction gaps with manual TagSpecs configuration (fallback).
    // Extraction always wins.
    if !tagspecs.libraries.is_empty() {
        let fallback = TagSpecs::from_tagspec_def(tagspecs);
        specs.merge_fallback(fallback);
    }

    specs
}

/// Compute `TagIndex` from the project's `TagSpecs`.
///
/// Depends on `compute_tag_specs` — automatic invalidation cascade ensures
/// the index is rebuilt whenever specs change.
#[salsa::tracked]
pub fn compute_tag_index(db: &dyn SemanticDb, project: Project) -> TagIndex<'_> {
    let specs = compute_tag_specs(db, project);
    TagIndex::from_tag_specs(db, &specs)
}

/// Compute `FilterAritySpecs` from a project's extraction results.
///
/// Merges filter arity data from both workspace (tracked) and external
/// extraction results, with last-wins semantics for name collisions
/// (matching Django's builtin ordering).
#[salsa::tracked]
pub fn compute_filter_arity_specs(
    db: &dyn SemanticDb,
    project: Project,
) -> djls_semantic::FilterAritySpecs {
    let mut specs = djls_semantic::FilterAritySpecs::new();

    // Merge workspace extraction results (tracked)
    let workspace_results = collect_workspace_extraction_results(db, project);
    for (_module_path, extraction) in &workspace_results {
        specs.merge_extraction_result(extraction);
    }

    // Merge external extraction results (from Project field)
    for extraction in project.extracted_external_rules(db).values() {
        specs.merge_extraction_result(extraction);
    }

    specs
}

/// Compute a merged `ModelGraph` from workspace and external model sources.
///
/// This tracked function reads `project.extracted_external_models(db)` to
/// establish a Salsa dependency on external model data. It merges:
/// 1. External models from site-packages (cached on `Project` field)
/// 2. Workspace models from project `models.py` files (tracked, auto-invalidating)
///
/// Workspace models merge after external, so project models take precedence
/// over installed package models for same-named models.
#[salsa::tracked]
pub fn compute_model_graph(db: &dyn SemanticDb, project: Project) -> ModelGraph {
    let mut graph = ModelGraph::new();

    // Merge external models (from Project field, updated by refresh_external_data)
    for model_graph in project.extracted_external_models(db).values() {
        graph.merge(model_graph.clone());
    }

    // Merge workspace models (tracked, auto-invalidating on file change)
    for (_module_path, model_graph) in collect_workspace_models(db, project) {
        graph.merge(model_graph);
    }

    graph
}

/// Collect model graphs from all workspace `models.py` files.
///
/// This tracked query:
/// 1. Discovers `models.py` files in the project root
/// 2. Reads each via `db.get_or_create_file` for Salsa tracking
/// 3. Extracts model graphs from each file
///
/// External `models.py` files (in site-packages) are handled separately
/// via the `Project.extracted_external_models` field. This function only
/// processes workspace files, giving them automatic Salsa invalidation
/// when the user edits a `models.py`.
#[salsa::tracked]
fn collect_workspace_models(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(ModulePath, ModelGraph)> {
    let root = project.root(db);

    let model_files = djls_project::discover_workspace_model_files(root);
    if model_files.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();

    for (module_path, file_path) in model_files {
        let file = db.get_or_create_file(&file_path);
        let source = file.source(db);

        let graph = djls_python::extract_model_graph(source.as_ref(), module_path.as_str());
        if !graph.is_empty() {
            results.push((module_path, graph));
        }
    }

    results
}

/// Collect extracted rules from all workspace registration modules.
///
/// This tracked query:
/// 1. Gets registration modules from inspector inventory
/// 2. Resolves workspace modules to `File` inputs via `get_or_create_file`
/// 3. Extracts rules from each (via tracked `djls_python::extract_module`)
///
/// External modules are handled separately (cached on `Project` field,
/// updated by `refresh_external_data`). This function only processes workspace
/// modules, giving them automatic Salsa invalidation when files change.
#[salsa::tracked]
fn collect_workspace_extraction_results(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(String, djls_python::ExtractionResult)> {
    let template_libraries = project.template_libraries(db);
    let interpreter = project.interpreter(db);
    let root = project.root(db);
    let pythonpath = project.pythonpath(db);

    let module_paths = template_libraries.registration_modules();
    if module_paths.is_empty() {
        return Vec::new();
    }

    let module_paths: Vec<String> = module_paths
        .iter()
        .map(|m| m.as_str().to_string())
        .collect();

    let search_paths = build_search_paths(interpreter, root, pythonpath);

    let (workspace_modules, _external) =
        djls_project::resolve_modules(module_paths.iter().map(String::as_str), &search_paths, root);

    let mut results = Vec::new();

    for resolved in workspace_modules {
        let file = db.get_or_create_file(&resolved.file_path);
        let mut extraction = djls_python::extract_module(db, file);

        if !extraction.is_empty() {
            extraction.rekey_module(&resolved.module_path);
            results.push((resolved.module_path, extraction));
        }
    }

    results
}
