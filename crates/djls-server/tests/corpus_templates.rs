//! Corpus-scale template validation tests.
//!
//! These tests validate actual templates from the corpus against
//! extracted rules, proving zero false positives end-to-end.
//!
//! # Running
//!
//! ```bash
//! # First, sync the corpus:
//! cargo run -p djls-corpus -- sync
//!
//! # Then run corpus template validation:
//! cargo test -p djls-server corpus_templates -- --nocapture
//! ```

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_corpus::enumerate::FileKind;
use djls_corpus::Corpus;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::File;
use djls_workspace::FileSystem;
use djls_workspace::InMemoryFileSystem;

#[salsa::db]
#[derive(Clone)]
struct CorpusTestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<Mutex<InMemoryFileSystem>>,
    specs: TagSpecs,
    arity_specs: FilterAritySpecs,
}

impl CorpusTestDatabase {
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
impl salsa::Database for CorpusTestDatabase {}

#[salsa::db]
impl djls_source::Db for CorpusTestDatabase {
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
impl djls_templates::Db for CorpusTestDatabase {}

#[salsa::db]
impl djls_semantic::Db for CorpusTestDatabase {
    fn tag_specs(&self) -> TagSpecs {
        self.specs.clone()
    }

    fn tag_index(&self) -> djls_semantic::TagIndex<'_> {
        djls_semantic::TagIndex::from_specs(self)
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        None
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        djls_conf::DiagnosticsConfig::default()
    }

    fn inspector_inventory(&self) -> Option<djls_project::TemplateTags> {
        None
    }

    fn filter_arity_specs(&self) -> FilterAritySpecs {
        self.arity_specs.clone()
    }

    fn environment_inventory(&self) -> Option<djls_extraction::EnvironmentInventory> {
        None
    }
}

/// Extract rules from all Python files in a directory and merge into specs.
fn extract_and_merge(
    corpus: &Corpus,
    dir: &Utf8Path,
    specs: &mut TagSpecs,
    arities: &mut FilterAritySpecs,
) {
    let extraction_files = corpus.enumerate_files(dir, FileKind::ExtractionTarget);
    for file_path in &extraction_files {
        if let Some(result) = corpus.extract_file(file_path) {
            arities.merge_extraction_result(&result);
            specs.merge_extraction_results(&result);
        }
    }
}

/// Build `TagSpecs` and `FilterAritySpecs` from extraction of all templatetag
/// modules in a directory.
fn build_specs_from_extraction(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();
    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);
    (specs, arities)
}

/// Build `TagSpecs` for a third-party package, including Django builtins
/// extracted from the matching Django version (or latest available).
fn build_specs_with_django_builtins(
    corpus: &Corpus,
    entry_dir: &Utf8Path,
    django_dir: Option<&Utf8Path>,
) -> (TagSpecs, FilterAritySpecs) {
    let mut specs = TagSpecs::default();
    let mut arities = FilterAritySpecs::new();

    if let Some(django) = django_dir {
        extract_and_merge(corpus, django, &mut specs, &mut arities);
    }

    extract_and_merge(corpus, entry_dir, &mut specs, &mut arities);
    (specs, arities)
}

/// Validate a template string and return only argument validation errors
/// (S114, S115, S116, S117) that could be false positives from extraction.
fn validate_template(
    content: &str,
    specs: &TagSpecs,
    arities: &FilterAritySpecs,
) -> Vec<ValidationError> {
    use djls_source::Db as SourceDb;

    let db = CorpusTestDatabase::new(specs.clone(), arities.clone());

    let path = "corpus_test.html";
    db.add_file(path, content);
    let file = db.create_file(Utf8Path::new(path));

    let Some(nodelist) = djls_templates::parse_template(&db, file) else {
        return Vec::new();
    };

    djls_semantic::validate_nodelist(&db, nodelist);

    djls_semantic::validate_nodelist::accumulated::<ValidationErrorAccumulator>(&db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .filter(is_argument_validation_error)
        .collect()
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

/// Run validation on all templates in a directory against given specs.
fn validate_templates_in_dir(
    corpus: &Corpus,
    dir: &Utf8Path,
    specs: &TagSpecs,
    arities: &FilterAritySpecs,
) -> Vec<FailureEntry> {
    let templates = corpus.enumerate_files(dir, FileKind::Template);
    let mut failures = Vec::new();

    for template_path in &templates {
        let Ok(content) = std::fs::read_to_string(template_path.as_std_path()) else {
            continue;
        };

        let errors = validate_template(&content, specs, arities);

        if !errors.is_empty() {
            let arg_errors: Vec<String> = errors.iter().map(|e| format!("{e:?}")).collect();
            failures.push(FailureEntry {
                path: template_path.clone(),
                errors: arg_errors,
            });
        }
    }

    failures
}

/// Django shipped templates (contrib/admin, forms, etc.) should produce
/// zero false positives when validated against Django's own extracted rules.
#[test]
fn test_django_shipped_templates_zero_false_positives() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };

    let django_packages = corpus.root().join("packages/Django");
    if !django_packages.as_std_path().exists() {
        eprintln!("No Django packages in corpus.");
        return;
    }

    for version_dir in &corpus.synced_dirs("packages/Django") {
        let version = version_dir.file_name().unwrap();

        let (specs, arities) = build_specs_from_extraction(&corpus, version_dir);
        let templates = corpus.enumerate_files(version_dir, FileKind::Template);

        if templates.is_empty() {
            eprintln!(
                "  Django {version} — no templates found \
                 (corpus may need re-sync with template support)"
            );
            continue;
        }

        let failures = validate_templates_in_dir(&corpus, version_dir, &specs, &arities);

        assert!(
            failures.is_empty(),
            "Django {version} shipped templates have argument \
             validation false positives:\n{}",
            format_failures(&failures),
        );

        eprintln!(
            "  ✓ Django {version} — {} templates validated, \
             zero argument validation false positives",
            templates.len()
        );
    }
}

/// Third-party package templates should produce zero argument validation
/// false positives when validated against the package's own extracted rules
/// plus Django builtins.
#[test]
fn test_third_party_templates_zero_arg_false_positives() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };

    let packages_dir = corpus.root().join("packages");
    if !packages_dir.as_std_path().exists() {
        eprintln!("No packages directory in corpus.");
        return;
    }

    let latest_django = corpus.latest_django();

    let mut entry_dirs: Vec<Utf8PathBuf> = std::fs::read_dir(packages_dir.as_std_path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter(|p| p.file_name().is_some_and(|n| n != "Django"))
        .collect();
    entry_dirs.sort();

    for pkg_dir in &entry_dirs {
        let pkg_name = pkg_dir.file_name().unwrap();
        let pkg_relative = format!("packages/{pkg_name}");

        for version_dir in &corpus.synced_dirs(&pkg_relative) {
            let version = version_dir.file_name().unwrap();

            let (specs, arities) =
                build_specs_with_django_builtins(&corpus, version_dir, latest_django.as_deref());
            let templates = corpus.enumerate_files(version_dir, FileKind::Template);

            if templates.is_empty() {
                continue;
            }

            let failures = validate_templates_in_dir(&corpus, version_dir, &specs, &arities);

            assert!(
                failures.is_empty(),
                "{pkg_name} {version} templates have argument \
                 validation false positives:\n{}",
                format_failures(&failures),
            );

            eprintln!(
                "  ✓ {pkg_name} {version} — {} templates validated",
                templates.len()
            );
        }
    }
}

/// Repo templates (Sentry, `NetBox`) should produce zero argument validation
/// false positives.
#[test]
fn test_repo_templates_zero_arg_false_positives() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };

    let repos_dir = corpus.root().join("repos");
    if !repos_dir.as_std_path().exists() {
        eprintln!("No repos directory in corpus.");
        return;
    }

    let latest_django = corpus.latest_django();

    let Ok(repo_entries) = std::fs::read_dir(repos_dir.as_std_path()) else {
        return;
    };

    let mut repo_dirs: Vec<Utf8PathBuf> = repo_entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .collect();
    repo_dirs.sort();

    for repo_dir in &repo_dirs {
        let repo_name = repo_dir.file_name().unwrap();
        let repo_relative = format!("repos/{repo_name}");

        for ref_dir in &corpus.synced_dirs(&repo_relative) {
            let (specs, arities) =
                build_specs_with_django_builtins(&corpus, ref_dir, latest_django.as_deref());
            let templates = corpus.enumerate_files(ref_dir, FileKind::Template);

            if templates.is_empty() {
                continue;
            }

            let failures = validate_templates_in_dir(&corpus, ref_dir, &specs, &arities);

            assert!(
                failures.is_empty(),
                "{repo_name} templates have argument \
                 validation false positives:\n{}",
                format_failures(&failures),
            );

            eprintln!("  ✓ {repo_name} — {} templates validated", templates.len());
        }
    }
}
