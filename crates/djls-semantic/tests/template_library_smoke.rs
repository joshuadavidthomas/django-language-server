use std::fmt::Write;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use datatest_stable::Utf8Path as DtUtf8Path;
use djls_corpus::module_path_from_file;
use djls_corpus::Corpus;
use djls_project::TemplateLibraries;
use djls_semantic::validate_nodelist;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_templates::parse_template;
use djls_workspace::FileSystem;
use djls_workspace::InMemoryFileSystem;

#[salsa::db]
#[derive(Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
    fs: Arc<Mutex<InMemoryFileSystem>>,
    specs: TagSpecs,
    arity_specs: FilterAritySpecs,
}

impl TestDb {
    fn new(specs: TagSpecs, arity_specs: FilterAritySpecs) -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            specs,
            arity_specs,
        }
    }

    fn add_file(&self, path: &str, content: &str) {
        self.fs
            .lock()
            .unwrap()
            .add_file(path.into(), content.to_string());
    }
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl djls_source::Db for TestDb {
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
impl djls_templates::Db for TestDb {}

#[salsa::db]
impl djls_semantic::Db for TestDb {
    fn tag_specs(&self) -> TagSpecs {
        self.specs.clone()
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
        TemplateLibraries::default()
    }

    fn filter_arity_specs(&self) -> FilterAritySpecs {
        self.arity_specs.clone()
    }
}

fn is_argument_validation_error(err: &ValidationError) -> bool {
    matches!(
        err,
        ValidationError::ExpressionSyntaxError { .. }
            | ValidationError::FilterMissingArgument { .. }
            | ValidationError::FilterUnexpectedArgument { .. }
            | ValidationError::ExtractedRuleViolation { .. }
    )
}

fn validate_template(
    content: &str,
    specs: &TagSpecs,
    arities: &FilterAritySpecs,
) -> Vec<ValidationError> {
    let db = TestDb::new(specs.clone(), arities.clone());

    let path = "corpus_test.html";
    db.add_file(path, content);
    let file = db.create_file(Utf8Path::new(path));

    let Some(nodelist) = parse_template(&db, file) else {
        return Vec::new();
    };

    validate_nodelist(&db, nodelist);

    validate_nodelist::accumulated::<ValidationErrorAccumulator>(&db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .filter(is_argument_validation_error)
        .collect()
}

struct FailureEntry {
    path: Utf8PathBuf,
    errors: Vec<String>,
}

fn format_failures(failures: &[FailureEntry]) -> String {
    let mut out = String::new();
    for f in failures.iter().take(20) {
        let _ = writeln!(out, "  {}:", f.path);
        for err in &f.errors {
            let _ = writeln!(out, "    - {err}");
        }
    }
    if failures.len() > 20 {
        let _ = writeln!(out, "  ... and {} more", failures.len() - 20);
    }
    out
}

/// Find the corpus entry root for a given library file.
///
/// Walks up from the file until we find the directory that is a direct child
/// of `packages/` or `repos/`.
fn corpus_entry_dir(library_path: &Utf8Path, corpus_root: &Utf8Path) -> Utf8PathBuf {
    let relative = library_path
        .strip_prefix(corpus_root)
        .expect("library path should be under corpus root");
    // relative is like "packages/django-unfold/src/unfold/templatetags/unfold.py"
    // or "repos/sentry/src/sentry/templatetags/sentry_helpers.py"
    // We want the first two components: "packages/django-unfold" or "repos/sentry"
    let mut components = relative.components();
    let category = components.next().expect("should have category");
    let entry = components.next().expect("should have entry name");
    corpus_root.join(category.as_str()).join(entry.as_str())
}

fn test_library(path: &DtUtf8Path) -> datatest_stable::Result<()> {
    let path = Utf8Path::new(path.as_str());
    let corpus = Corpus::require();
    let entry_dir = corpus_entry_dir(path, corpus.root());
    let entry_name = entry_dir.file_name().unwrap();

    // Build specs: Django builtins + this entry's extractions
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();

    // Add Django builtins for non-Django entries
    let is_django = entry_dir
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|cat| cat == "packages")
        && (entry_name == "django"
            || (entry_name.starts_with("django-")
                && entry_name["django-".len()..].starts_with(|c: char| c.is_ascii_digit())));

    if !is_django {
        if let Some(django_dir) = corpus.latest_package("django") {
            for file_path in &corpus.extraction_targets_in(&django_dir) {
                let Ok(source) = std::fs::read_to_string(file_path.as_std_path()) else {
                    continue;
                };
                let module_path = module_path_from_file(file_path);
                let result = djls_python::extract_rules(&source, &module_path);
                arities.merge_extraction_result(&result);
                specs.merge_extraction_results(&result);
            }
        }
    }

    // Extract from this entry
    for file_path in &corpus.extraction_targets_in(&entry_dir) {
        let Ok(source) = std::fs::read_to_string(file_path.as_std_path()) else {
            continue;
        };
        let module_path = module_path_from_file(file_path);
        let result = djls_python::extract_rules(&source, &module_path);
        arities.merge_extraction_result(&result);
        specs.merge_extraction_results(&result);
    }

    // Validate all templates in this entry
    let templates = corpus.templates_in(&entry_dir);
    if templates.is_empty() {
        return Ok(());
    }

    let mut failures = Vec::new();
    for template_path in &templates {
        let Ok(content) = std::fs::read_to_string(template_path.as_std_path()) else {
            continue;
        };
        let errors = validate_template(&content, &specs, &arities);
        if !errors.is_empty() {
            let arg_errors: Vec<String> = errors.iter().map(|e| format!("{e:?}")).collect();
            failures.push(FailureEntry {
                path: template_path.clone(),
                errors: arg_errors,
            });
        }
    }

    if !failures.is_empty() {
        return Err(format!(
            "{entry_name} templates have false positives:\n{}",
            format_failures(&failures),
        )
        .into());
    }

    Ok(())
}

fn corpus_root() -> String {
    Corpus::require().root().to_string()
}

datatest_stable::harness! {
    {
        test = test_library,
        root = corpus_root(),
        pattern = r"(templatetags/[^/]+\.py|template/(defaulttags|defaultfilters|loader_tags)\.py)$",
    },
}
