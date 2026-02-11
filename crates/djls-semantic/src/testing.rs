use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_corpus::module_path_from_file;
use djls_corpus::Corpus;
use djls_project::InspectorLibrarySymbol;
use djls_project::TemplateLibraries;
use djls_project::TemplateLibrariesResponse;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::Severity;
use djls_source::Span;
use djls_templates::parse_template;
use djls_workspace::FileSystem;
use djls_workspace::InMemoryFileSystem;

use crate::specs::tags::test_tag_specs;
use crate::FilterAritySpecs;
use crate::TagIndex;
use crate::TagSpecs;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

pub(crate) fn builtin_tag_json(name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "tag",
        "name": name,
        "load_name": null,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

pub(crate) fn library_tag_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "tag",
        "name": name,
        "load_name": load_name,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

pub(crate) fn builtin_filter_json(name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "filter",
        "name": name,
        "load_name": null,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

pub(crate) fn library_filter_json(name: &str, load_name: &str, module: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "filter",
        "name": name,
        "load_name": load_name,
        "library_module": module,
        "module": module,
        "doc": null,
    })
}

pub(crate) fn make_template_libraries(
    tags: &[serde_json::Value],
    filters: &[serde_json::Value],
    libraries: &HashMap<String, String>,
    builtins: &[String],
) -> TemplateLibraries {
    let mut symbols: Vec<InspectorLibrarySymbol> = tags
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .unwrap();

    symbols.extend(
        filters
            .iter()
            .cloned()
            .map(serde_json::from_value)
            .collect::<Result<Vec<InspectorLibrarySymbol>, _>>()
            .unwrap(),
    );

    let response = TemplateLibrariesResponse {
        symbols,
        libraries: libraries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        builtins: builtins.to_vec(),
    };

    TemplateLibraries::default().apply_inspector(Some(response))
}

pub(crate) fn make_template_libraries_tags_only(
    tags: &[serde_json::Value],
    libraries: &HashMap<String, String>,
    builtins: &[String],
) -> TemplateLibraries {
    make_template_libraries(tags, &[], libraries, builtins)
}

#[salsa::db]
#[derive(Clone)]
pub(crate) struct TestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<Mutex<InMemoryFileSystem>>,
    tag_specs: TagSpecs,
    filter_arity_specs: FilterAritySpecs,
    template_libraries: TemplateLibraries,
}

impl TestDatabase {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            tag_specs: test_tag_specs(),
            filter_arity_specs: FilterAritySpecs::new(),
            template_libraries: TemplateLibraries::default(),
        }
    }

    #[must_use]
    pub(crate) fn with_specs(mut self, specs: TagSpecs) -> Self {
        self.tag_specs = specs;
        self
    }

    #[must_use]
    pub(crate) fn with_arity_specs(mut self, specs: FilterAritySpecs) -> Self {
        self.filter_arity_specs = specs;
        self
    }

    #[must_use]
    pub(crate) fn with_template_libraries(mut self, template_libraries: TemplateLibraries) -> Self {
        self.template_libraries = template_libraries;
        self
    }

    pub(crate) fn add_file(&self, path: &str, content: &str) {
        self.fs
            .lock()
            .unwrap()
            .add_file(path.into(), content.to_string());
    }

    #[must_use]
    pub(crate) fn create_file(&self, path: &Utf8Path) -> File {
        <Self as djls_source::Db>::create_file(self, path)
    }

    #[must_use]
    pub(crate) fn create_file_with_revision(&self, path: &Utf8Path, revision: u64) -> File {
        File::new(self, path.to_owned(), revision)
    }
}

#[salsa::db]
impl salsa::Database for TestDatabase {}

#[salsa::db]
impl djls_source::Db for TestDatabase {
    fn create_file(&self, path: &Utf8Path) -> File {
        File::new(self, path.to_owned(), 0)
    }

    fn get_file(&self, _path: &Utf8Path) -> Option<File> {
        None
    }

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
        self.fs.lock().unwrap().read_to_string(path)
    }
}

#[salsa::db]
impl djls_templates::Db for TestDatabase {}

#[salsa::db]
impl crate::Db for TestDatabase {
    fn tag_specs(&self) -> TagSpecs {
        self.tag_specs.clone()
    }

    fn tag_index(&self) -> TagIndex<'_> {
        TagIndex::from_specs(self)
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        None
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        djls_conf::DiagnosticsConfig::default()
    }

    fn template_libraries(&self) -> TemplateLibraries {
        self.template_libraries.clone()
    }

    fn filter_arity_specs(&self) -> FilterAritySpecs {
        self.filter_arity_specs.clone()
    }
}

pub(crate) fn collect_errors(db: &TestDatabase, path: &str, source: &str) -> Vec<ValidationError> {
    collect_errors_with_revision(db, path, 0, source)
}

pub(crate) fn collect_errors_with_revision(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
) -> Vec<ValidationError> {
    db.add_file(path, source);
    let file = db.create_file_with_revision(Utf8Path::new(path), revision);

    let Some(nodelist) = parse_template(db, file) else {
        return Vec::new();
    };

    crate::validate_nodelist(db, nodelist);

    crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .collect()
}

pub(crate) fn is_argument_validation_error(err: &ValidationError) -> bool {
    matches!(
        err,
        ValidationError::ExpressionSyntaxError { .. }
            | ValidationError::FilterMissingArgument { .. }
            | ValidationError::FilterUnexpectedArgument { .. }
            | ValidationError::ExtractedRuleViolation { .. }
    )
}

pub(crate) fn collect_argument_validation_errors_with_revision(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
) -> Vec<ValidationError> {
    db.add_file(path, source);
    let file = db.create_file_with_revision(Utf8Path::new(path), revision);

    let Some(nodelist) = parse_template(db, file) else {
        return Vec::new();
    };

    crate::validate_nodelist(db, nodelist);

    crate::validate_nodelist::accumulated::<ValidationErrorAccumulator>(db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .filter(is_argument_validation_error)
        .collect()
}

pub(crate) fn extract_and_merge(
    corpus: &Corpus,
    dir: &Utf8Path,
    specs: &mut TagSpecs,
    arities: &mut FilterAritySpecs,
) {
    for file_path in &corpus.extraction_targets_in(dir) {
        let Ok(source) = std::fs::read_to_string(file_path.as_std_path()) else {
            continue;
        };

        let module_path = module_path_from_file(file_path);
        let result = djls_python::extract_rules(&source, &module_path);
        arities.merge_extraction_result(&result);
        specs.merge_extraction_results(&result);
    }
}

pub(crate) fn build_specs_from_extraction(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();
    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);
    (specs, arities)
}

pub(crate) fn build_entry_specs(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();

    if !corpus.is_django_entry(entry_dir) {
        if let Some(django_dir) = corpus.latest_package("django") {
            extract_and_merge(corpus, &django_dir, &mut specs, &mut arities);
        }
    }

    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);

    (specs, arities)
}

pub(crate) fn render_diagnostic_snapshot(
    path: &str,
    source: &str,
    errors: &[ValidationError],
) -> String {
    let renderer = DiagnosticRenderer::plain();
    let mut parts = Vec::new();

    for err in errors {
        let span = err
            .primary_span()
            .expect("all validation errors have a span");
        let message = err.to_string();
        let code = err.code();

        // Build notes for variants that carry extra context beyond the
        // primary message. When adding new ValidationError variants, consider
        // whether they have fields that would be useful as notes here.
        let mut notes: Vec<String> = Vec::new();
        match err {
            ValidationError::ExpressionSyntaxError { tag, .. }
            | ValidationError::ExtractedRuleViolation { tag, .. } => {
                notes.push(format!("in tag: {tag}"));
            }
            ValidationError::TagNotInInstalledApps { load_name, .. }
            | ValidationError::FilterNotInInstalledApps { load_name, .. } => {
                notes.push(format!("load_name: {load_name}"));
            }
            ValidationError::LibraryNotInInstalledApps { candidates, .. }
                if !candidates.is_empty() =>
            {
                notes.push(format!("candidates: {candidates:?}"));
            }
            ValidationError::UnclosedTag { .. }
            | ValidationError::OrphanedTag { .. }
            | ValidationError::UnbalancedStructure { .. }
            | ValidationError::UnmatchedBlockName { .. }
            | ValidationError::UnknownTag { .. }
            | ValidationError::UnloadedTag { .. }
            | ValidationError::AmbiguousUnloadedTag { .. }
            | ValidationError::UnknownFilter { .. }
            | ValidationError::UnloadedFilter { .. }
            | ValidationError::AmbiguousUnloadedFilter { .. }
            | ValidationError::FilterMissingArgument { .. }
            | ValidationError::FilterUnexpectedArgument { .. }
            | ValidationError::UnknownLibrary { .. }
            | ValidationError::LibraryNotInInstalledApps { .. }
            | ValidationError::ExtendsMustBeFirst { .. }
            | ValidationError::MultipleExtends { .. } => {}
        }

        let mut diag = Diagnostic::new(source, path, code, &message, Severity::Error, span, "");

        if let ValidationError::UnbalancedStructure {
            closing_span: Some(cs),
            ..
        } = err
        {
            diag = diag.annotation(*cs, "", false);
        }

        for note in &notes {
            diag = diag.note(note);
        }

        parts.push(renderer.render(&diag));
    }

    parts.join("\n")
}

pub(crate) fn render_validate_snapshot(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
) -> String {
    render_validate_snapshot_filtered(db, path, revision, source, |_| true)
}

pub(crate) fn render_validate_snapshot_filtered<F>(
    db: &TestDatabase,
    path: &str,
    revision: u64,
    source: &str,
    filter: F,
) -> String
where
    F: Fn(&ValidationError) -> bool,
{
    let mut errors: Vec<ValidationError> = collect_errors_with_revision(db, path, revision, source)
        .into_iter()
        .filter(|e| filter(e))
        .collect();

    errors.sort_by_key(|e| e.primary_span().map_or(0, Span::start));

    render_diagnostic_snapshot(path, source, &errors)
}
