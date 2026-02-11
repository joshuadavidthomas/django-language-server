use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_corpus::module_path_from_file;
use djls_corpus::Corpus;
use djls_project::InspectorLibrarySymbol;
use djls_project::TemplateLibraries;
use djls_project::TemplateLibrariesResponse;
use djls_project::TemplateLibrary;
use djls_source::File;
use djls_source::LineIndex;
use djls_source::Span;
use djls_templates::parse_template;
use djls_workspace::FileSystem;
use djls_workspace::InMemoryFileSystem;

use crate::templatetags::test_tag_specs;
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

    #[must_use]
    pub(crate) fn with_inventory(template_libraries: TemplateLibraries) -> Self {
        Self::new().with_template_libraries(template_libraries)
    }

    #[must_use]
    pub(crate) fn with_inventories(
        template_libraries: TemplateLibraries,
        discovered: Vec<TemplateLibrary>,
    ) -> Self {
        Self::new().with_template_libraries(template_libraries.apply_discovery(discovered))
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

pub(crate) fn primary_span(err: &ValidationError) -> Span {
    match err {
        ValidationError::UnclosedTag { span, .. }
        | ValidationError::OrphanedTag { span, .. }
        | ValidationError::UnmatchedBlockName { span, .. }
        | ValidationError::UnknownTag { span, .. }
        | ValidationError::UnloadedTag { span, .. }
        | ValidationError::AmbiguousUnloadedTag { span, .. }
        | ValidationError::UnknownFilter { span, .. }
        | ValidationError::UnloadedFilter { span, .. }
        | ValidationError::AmbiguousUnloadedFilter { span, .. }
        | ValidationError::ExpressionSyntaxError { span, .. }
        | ValidationError::FilterMissingArgument { span, .. }
        | ValidationError::FilterUnexpectedArgument { span, .. }
        | ValidationError::ExtractedRuleViolation { span, .. }
        | ValidationError::TagNotInInstalledApps { span, .. }
        | ValidationError::FilterNotInInstalledApps { span, .. }
        | ValidationError::UnknownLibrary { span, .. }
        | ValidationError::LibraryNotInInstalledApps { span, .. }
        | ValidationError::ExtendsMustBeFirst { span }
        | ValidationError::MultipleExtends { span } => *span,
        ValidationError::UnbalancedStructure { opening_span, .. } => *opening_span,
    }
}

pub(crate) fn error_kind(err: &ValidationError) -> &'static str {
    match err {
        ValidationError::UnclosedTag { .. } => "UnclosedTag",
        ValidationError::OrphanedTag { .. } => "OrphanedTag",
        ValidationError::UnbalancedStructure { .. } => "UnbalancedStructure",
        ValidationError::UnmatchedBlockName { .. } => "UnmatchedBlockName",
        ValidationError::UnknownTag { .. } => "UnknownTag",
        ValidationError::UnloadedTag { .. } => "UnloadedTag",
        ValidationError::AmbiguousUnloadedTag { .. } => "AmbiguousUnloadedTag",
        ValidationError::UnknownFilter { .. } => "UnknownFilter",
        ValidationError::UnloadedFilter { .. } => "UnloadedFilter",
        ValidationError::AmbiguousUnloadedFilter { .. } => "AmbiguousUnloadedFilter",
        ValidationError::ExpressionSyntaxError { .. } => "ExpressionSyntaxError",
        ValidationError::FilterMissingArgument { .. } => "FilterMissingArgument",
        ValidationError::FilterUnexpectedArgument { .. } => "FilterUnexpectedArgument",
        ValidationError::ExtractedRuleViolation { .. } => "ExtractedRuleViolation",
        ValidationError::TagNotInInstalledApps { .. } => "TagNotInInstalledApps",
        ValidationError::FilterNotInInstalledApps { .. } => "FilterNotInInstalledApps",
        ValidationError::UnknownLibrary { .. } => "UnknownLibrary",
        ValidationError::LibraryNotInInstalledApps { .. } => "LibraryNotInInstalledApps",
        ValidationError::ExtendsMustBeFirst { .. } => "ExtendsMustBeFirst",
        ValidationError::MultipleExtends { .. } => "MultipleExtends",
    }
}

fn error_detail_lines(err: &ValidationError) -> Vec<String> {
    match err {
        ValidationError::ExpressionSyntaxError { tag, .. }
        | ValidationError::ExtractedRuleViolation { tag, .. } => {
            vec![format!("tag: {tag}")]
        }
        ValidationError::TagNotInInstalledApps { load_name, .. }
        | ValidationError::FilterNotInInstalledApps { load_name, .. } => {
            vec![format!("load_name: {load_name}")]
        }
        ValidationError::LibraryNotInInstalledApps { candidates, .. } => {
            if candidates.is_empty() {
                Vec::new()
            } else {
                vec![format!("candidates: {candidates:?}")]
            }
        }
        _ => Vec::new(),
    }
}

pub(crate) fn render_diagnostic_snapshot(
    path: &str,
    source: &str,
    errors: &[ValidationError],
) -> String {
    let index = LineIndex::from(source);

    let mut out = String::new();

    out.push_str("Source\n");
    out.push_str(path);
    out.push('\n');

    for (i, line) in source.lines().enumerate() {
        let line_no = i + 1;
        let _ = writeln!(&mut out, "{line_no:>4} | {line}");
    }

    if source.ends_with('\n') {
        let _ = writeln!(&mut out, "{:>4} |", source.lines().count() + 1);
    }

    out.push('\n');
    let _ = writeln!(&mut out, "Diagnostics ({})", errors.len());

    for (i, err) in errors.iter().enumerate() {
        let span = primary_span(err);
        let start = span.start_offset();
        let end = span.end_offset();

        let start_lc = index.to_line_col(start);
        let _end_lc = index.to_line_col(end);

        let _ = writeln!(&mut out, "{}. {}: {}", i + 1, error_kind(err), err);
        let _ = writeln!(
            &mut out,
            "    --> {}:{}:{}",
            path,
            start_lc.line() + 1,
            start_lc.column() + 1
        );

        for detail in error_detail_lines(err) {
            let _ = writeln!(&mut out, "    {detail}");
        }

        let line = start_lc.line();
        let Some(line_start_u32) = index.line_start(line) else {
            out.push('\n');
            continue;
        };

        let line_start = line_start_u32 as usize;
        let next_start_u32 = index
            .line_start(line + 1)
            .unwrap_or_else(|| u32::try_from(source.len()).unwrap_or(u32::MAX));
        let mut line_end = next_start_u32 as usize;
        line_end = line_end.min(source.len());

        let mut line_text = source[line_start..line_end].to_string();
        while line_text.ends_with('\n') || line_text.ends_with('\r') {
            line_text.pop();
        }

        let start_in_line = (start.get() as usize).saturating_sub(line_start);
        let end_in_line = (end.get() as usize)
            .saturating_sub(line_start)
            .min(line_text.len());

        let caret_len = end_in_line.saturating_sub(start_in_line).max(1);
        let caret = "^".repeat(caret_len);
        let padding = " ".repeat(start_in_line.min(line_text.len()));

        let line_no = line as usize + 1;
        let _ = writeln!(&mut out, "{line_no:>4} | {line_text}");
        let _ = writeln!(&mut out, "     | {padding}{caret}");
        out.push('\n');
    }

    out
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

    errors.sort_by_key(|e| primary_span(e).start());

    render_diagnostic_snapshot(path, source, &errors)
}
