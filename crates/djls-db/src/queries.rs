use djls_semantic::build_search_paths;
use djls_semantic::Db as SemanticDb;
use djls_semantic::ModelGraph;
use djls_semantic::ModulePath;
use djls_semantic::Project;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;

/// Compute `TagSpecs` from tag-rule and block-spec extraction results.
///
/// This tracked function reads only the extraction domains needed to build tag
/// specs. Filter-only extraction changes should not invalidate this query.
///
/// Does NOT read from `Arc<Mutex<Settings>>`.
#[salsa::tracked(returns(ref))]
pub fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
    let _libraries = project.template_libraries(db);
    let tagspecs = project.tagspecs(db);

    // Start from Django's builtin tag specs as a base
    let mut specs = djls_semantic::builtin_tag_specs();

    // Merge workspace extraction results (tracked, auto-invalidating on file change)
    let workspace_block_specs = collect_workspace_block_specs(db, project);
    for (_module_path, block_specs) in workspace_block_specs {
        specs.merge_block_specs(block_specs);
    }
    let workspace_tag_rules = collect_workspace_tag_rules(db, project);
    for (_module_path, tag_rules) in workspace_tag_rules {
        specs.merge_tag_rules(tag_rules);
    }

    // Merge external extraction results (from Project fields, updated by refresh_external_data)
    for block_specs in project.extracted_external_block_specs(db).values() {
        specs.merge_block_specs(block_specs);
    }
    for tag_rules in project.extracted_external_tag_rules(db).values() {
        specs.merge_tag_rules(tag_rules);
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
    TagIndex::from_tag_specs(db, specs)
}

/// Compute `FilterAritySpecs` from a project's extraction results.
///
/// Merges filter arity data from both workspace (tracked) and external
/// extraction results, with last-wins semantics for name collisions
/// (matching Django's builtin ordering).
#[salsa::tracked(returns(ref))]
pub fn compute_filter_arity_specs(
    db: &dyn SemanticDb,
    project: Project,
) -> djls_semantic::FilterAritySpecs {
    let mut specs = djls_semantic::FilterAritySpecs::new();

    // Merge workspace extraction results (tracked)
    let workspace_results = collect_workspace_filter_arities(db, project);
    for (_module_path, filter_arities) in workspace_results {
        specs.merge_filter_arities(filter_arities);
    }

    // Merge external extraction results (from Project field)
    for filter_arities in project.extracted_external_filter_arities(db).values() {
        specs.merge_filter_arities(filter_arities);
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
#[salsa::tracked(returns(ref))]
pub fn compute_model_graph(db: &dyn SemanticDb, project: Project) -> ModelGraph {
    let mut graph = ModelGraph::new();

    // Merge external models (from Project field, updated by refresh_external_data)
    for model_graph in project.extracted_external_models(db).values() {
        graph.merge(model_graph.clone());
    }

    // Merge workspace models (tracked, auto-invalidating on file change)
    for (_module_path, model_graph) in collect_workspace_models(db, project) {
        graph.merge(model_graph.clone());
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
#[salsa::tracked(returns(ref))]
fn collect_workspace_models(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(ModulePath, ModelGraph)> {
    let root = project.root(db);

    let model_files = djls_semantic::discover_workspace_model_files(root);
    if model_files.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();

    for (module_path, file_path) in model_files {
        let file = db.get_or_create_file(&file_path);
        let source = file.source(db);

        let graph = djls_semantic::extract_model_graph(source.as_ref(), module_path.as_str());
        if !graph.is_empty() {
            results.push((module_path, graph));
        }
    }

    results
}

fn resolve_workspace_registration_modules(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<djls_semantic::ResolvedModule> {
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

    let (workspace_modules, _external) = djls_semantic::resolve_modules(
        module_paths.iter().map(String::as_str),
        &search_paths,
        root,
    );

    workspace_modules
}

/// Collect extracted tag rules from all workspace registration modules.
#[salsa::tracked(returns(ref))]
fn collect_workspace_tag_rules(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(String, djls_semantic::TagRuleMap)> {
    let mut results = Vec::new();

    for resolved in resolve_workspace_registration_modules(db, project) {
        let file = db.get_or_create_file(&resolved.file_path);
        let tag_rules = djls_semantic::extract_tag_rules(
            db,
            file,
            ModulePath::new(resolved.module_path.clone()),
        );

        if !tag_rules.is_empty() {
            results.push((resolved.module_path, tag_rules.clone()));
        }
    }

    results
}

/// Collect extracted filter arities from all workspace registration modules.
#[salsa::tracked(returns(ref))]
fn collect_workspace_filter_arities(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(String, djls_semantic::FilterArityMap)> {
    let mut results = Vec::new();

    for resolved in resolve_workspace_registration_modules(db, project) {
        let file = db.get_or_create_file(&resolved.file_path);
        let filter_arities = djls_semantic::extract_filter_arities(
            db,
            file,
            ModulePath::new(resolved.module_path.clone()),
        );

        if !filter_arities.is_empty() {
            results.push((resolved.module_path, filter_arities.clone()));
        }
    }

    results
}

/// Collect extracted block specs from all workspace registration modules.
#[salsa::tracked(returns(ref))]
fn collect_workspace_block_specs(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(String, djls_semantic::BlockSpecMap)> {
    let mut results = Vec::new();

    for resolved in resolve_workspace_registration_modules(db, project) {
        let file = db.get_or_create_file(&resolved.file_path);
        let block_specs = djls_semantic::extract_block_specs(
            db,
            file,
            ModulePath::new(resolved.module_path.clone()),
        );

        if !block_specs.is_empty() {
            results.push((resolved.module_path, block_specs.clone()));
        }
    }

    results
}
