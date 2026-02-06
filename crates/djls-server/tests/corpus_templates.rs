//! Corpus-scale template validation tests.
//!
//! These tests validate actual templates from the corpus against
//! extracted rules, proving zero false positives end-to-end.
//!
//! This is the Rust port of the prototype's `test_corpus_templates.py`
//! and `test_real_templates.py` — the ultimate proof the system works.
//!
//! # Running
//!
//! ```bash
//! # First, sync the corpus:
//! just corpus-sync
//!
//! # Then run corpus template validation:
//! cargo test -p djls-server corpus_templates -- --nocapture
//! ```

use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_corpus::enumerate::enumerate_extraction_files;
use djls_corpus::enumerate::enumerate_template_files;
use djls_extraction::extract_rules;
use djls_extraction::ExtractionResult;
use djls_semantic::django_builtin_specs;
use djls_semantic::TagSpecs;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::File;
use djls_workspace::FileSystem;
use djls_workspace::InMemoryFileSystem;

// ---------------------------------------------------------------------------
// Test database (minimal `Db` impl for validation)
// ---------------------------------------------------------------------------

#[salsa::db]
#[derive(Clone)]
struct CorpusTestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<Mutex<InMemoryFileSystem>>,
    specs: djls_semantic::TagSpecs,
}

impl CorpusTestDatabase {
    fn new(specs: djls_semantic::TagSpecs) -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            specs,
        }
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
    fn tag_specs(&self) -> djls_semantic::TagSpecs {
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

    fn inspector_inventory(&self) -> Option<djls_project::InspectorInventory> {
        None
    }

    fn filter_arity_specs(&self) -> djls_semantic::FilterAritySpecs {
        djls_semantic::FilterAritySpecs::default()
    }

    fn opaque_tag_map(&self) -> djls_semantic::OpaqueTagMap {
        let mut map = djls_semantic::OpaqueTagMap::default();
        for (name, spec) in &self.specs {
            if spec.opaque {
                if let Some(ref end_tag) = spec.end_tag {
                    map.insert(name.clone(), end_tag.name.to_string());
                }
            }
        }
        map
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get corpus root from environment, or use default if it exists.
fn corpus_root() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("DJLS_CORPUS_ROOT") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().and_then(|p| p.parent())?;
    let default = workspace_root.join("crates/djls-corpus/.corpus");
    if default.exists() {
        return Some(default);
    }

    None
}

/// Build `TagSpecs` from extraction of all templatetag modules in a directory.
///
/// Starts from Django builtin specs, then merges extraction results.
fn build_specs_from_extraction(entry_dir: &Path) -> TagSpecs {
    let mut specs = django_builtin_specs();
    extract_and_merge(entry_dir, &mut specs);
    specs
}

/// Extract rules from all Python files in a directory and merge into specs.
fn extract_and_merge(dir: &Path, specs: &mut TagSpecs) {
    let extraction_files = enumerate_extraction_files(dir);
    for file_path in &extraction_files {
        let Ok(source) = std::fs::read_to_string(file_path) else {
            continue;
        };
        let Ok(result) = extract_rules(&source) else {
            continue;
        };
        merge_extraction_into_specs(specs, &result);
    }
}

/// Build `TagSpecs` for a third-party package, including Django builtins
/// extracted from the matching Django version (or latest available).
fn build_specs_with_django_builtins(
    entry_dir: &Path,
    django_dir: Option<&Path>,
) -> TagSpecs {
    let mut specs = django_builtin_specs();

    if let Some(django) = django_dir {
        extract_and_merge(django, &mut specs);
    }

    extract_and_merge(entry_dir, &mut specs);
    specs
}

/// Merge extraction results into tag specs (same logic as server's
/// `merge_extraction_into_specs`).
fn merge_extraction_into_specs(specs: &mut TagSpecs, extraction: &ExtractionResult) {
    use djls_semantic::TagSpec;

    for tag in &extraction.tags {
        if let Some(spec) = specs.get_mut(&tag.name) {
            spec.merge_extracted_rules(&tag.rules);
            if let Some(ref block_spec) = tag.block_spec {
                spec.merge_block_spec(block_spec);
            }

        } else {
            let new_spec = TagSpec::from_extraction("corpus", tag);
            specs.insert(tag.name.clone(), new_spec);
        }
    }
}

/// Validate a template string against given specs.
///
/// Returns all validation errors.
fn validate_template(content: &str, specs: &TagSpecs) -> Vec<ValidationError> {
    use djls_source::Db as SourceDb;

    let db = CorpusTestDatabase::new(specs.clone());
    let path = Utf8Path::new("/test.html");
    db.fs
        .lock()
        .unwrap()
        .add_file(path.to_owned(), content.to_string());

    let file = db.create_file(path);
    let Some(nodelist) = djls_templates::parse_template(&db, file) else {
        return Vec::new();
    };

    djls_semantic::validate_nodelist(&db, nodelist);

    djls_semantic::validate_nodelist::accumulated::<ValidationErrorAccumulator>(&db, nodelist)
        .into_iter()
        .map(|acc| acc.0.clone())
        .collect()
}

/// Check if a validation error is an argument validation error (the kind
/// that could be a false positive from extracted rules).
fn is_argument_validation_error(err: &ValidationError) -> bool {
    matches!(err, ValidationError::ExtractedRuleViolation { .. })
}

/// Find the latest Django version directory in the corpus.
fn find_latest_django_dir(corpus_root: &Path) -> Option<PathBuf> {
    let django_dir = corpus_root.join("packages/Django");
    if !django_dir.exists() {
        return None;
    }

    let mut versions: Vec<PathBuf> = std::fs::read_dir(&django_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .map(|e| e.path())
        .filter(|p| p.join(".complete").exists())
        .collect();

    versions.sort();
    versions.last().cloned()
}

struct FailureEntry {
    path: PathBuf,
    errors: Vec<String>,
}

fn format_failures(failures: &[FailureEntry]) -> String {
    let mut out = String::new();
    for f in failures.iter().take(20) {
        let _ = writeln!(out, "  {}:", f.path.display());
        for err in &f.errors {
            let _ = writeln!(out, "    - {err}");
        }
    }
    if failures.len() > 20 {
        let _ = writeln!(out, "  ... and {} more", failures.len() - 20);
    }
    out
}

/// Collect version directories that have been fully synced.
fn synced_version_dirs(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };

    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .map(|e| e.path())
        .filter(|p| p.join(".complete").exists())
        .collect();

    dirs.sort();
    dirs
}

/// Run validation on all templates in a directory against given specs.
///
/// Returns failures (only argument validation errors).
fn validate_templates_in_dir(
    dir: &Path,
    specs: &TagSpecs,
) -> Vec<FailureEntry> {
    let templates = enumerate_template_files(dir);
    let mut failures = Vec::new();

    for template_path in &templates {
        let Ok(content) = std::fs::read_to_string(template_path) else {
            continue;
        };

        let errors = validate_template(&content, specs);

        let arg_errors: Vec<String> = errors
            .iter()
            .filter(|e| is_argument_validation_error(e))
            .map(|e| format!("{e:?}"))
            .collect();

        if !arg_errors.is_empty() {
            failures.push(FailureEntry {
                path: template_path.clone(),
                errors: arg_errors,
            });
        }
    }

    failures
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Django shipped templates (contrib/admin, forms, etc.) should produce
/// zero false positives when validated against Django's own extracted rules.
#[test]
fn test_django_shipped_templates_zero_false_positives() {
    let Some(root) = corpus_root() else {
        eprintln!("Corpus not available. Run `just corpus-sync`.");
        return;
    };

    let django_packages = root.join("packages/Django");
    if !django_packages.exists() {
        eprintln!("No Django packages in corpus.");
        return;
    }

    for version_dir in &synced_version_dirs(&django_packages) {
        let version = version_dir
            .file_name()
            .unwrap()
            .to_string_lossy();

        let specs = build_specs_from_extraction(version_dir);
        let templates = enumerate_template_files(version_dir);

        if templates.is_empty() {
            eprintln!(
                "  Django {version} — no templates found \
                 (corpus may need re-sync with template support)"
            );
            continue;
        }

        let failures = validate_templates_in_dir(version_dir, &specs);

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
    let Some(root) = corpus_root() else {
        eprintln!("Corpus not available. Run `just corpus-sync`.");
        return;
    };

    let packages_dir = root.join("packages");
    if !packages_dir.exists() {
        eprintln!("No packages directory in corpus.");
        return;
    }

    let latest_django = find_latest_django_dir(&root);

    let mut entry_dirs: Vec<PathBuf> = std::fs::read_dir(&packages_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n != "Django")
        })
        .collect();
    entry_dirs.sort();

    for pkg_dir in &entry_dirs {
        let pkg_name = pkg_dir.file_name().unwrap().to_string_lossy();

        for version_dir in &synced_version_dirs(pkg_dir) {
            let version = version_dir
                .file_name()
                .unwrap()
                .to_string_lossy();

            let specs = build_specs_with_django_builtins(
                version_dir,
                latest_django.as_deref(),
            );
            let templates = enumerate_template_files(version_dir);

            if templates.is_empty() {
                continue;
            }

            let failures = validate_templates_in_dir(version_dir, &specs);

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
    let Some(root) = corpus_root() else {
        eprintln!("Corpus not available. Run `just corpus-sync`.");
        return;
    };

    let repos_dir = root.join("repos");
    if !repos_dir.exists() {
        eprintln!("No repos directory in corpus.");
        return;
    }

    let latest_django = find_latest_django_dir(&root);

    let Ok(repo_entries) = std::fs::read_dir(&repos_dir) else {
        return;
    };

    let mut repo_dirs: Vec<PathBuf> = repo_entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .map(|e| e.path())
        .collect();
    repo_dirs.sort();

    for repo_dir in &repo_dirs {
        let repo_name = repo_dir.file_name().unwrap().to_string_lossy();

        for ref_dir in &synced_version_dirs(repo_dir) {
            let specs = build_specs_with_django_builtins(
                ref_dir,
                latest_django.as_deref(),
            );
            let templates = enumerate_template_files(ref_dir);

            if templates.is_empty() {
                continue;
            }

            let failures = validate_templates_in_dir(ref_dir, &specs);

            assert!(
                failures.is_empty(),
                "{repo_name} templates have argument \
                 validation false positives:\n{}",
                format_failures(&failures),
            );

            eprintln!(
                "  ✓ {repo_name} — {} templates validated",
                templates.len()
            );
        }
    }
}
