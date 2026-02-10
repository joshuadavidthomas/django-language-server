//! Corpus extraction snapshot tests.
//!
//! Uses `insta::glob!` for per-file snapshot granularity — each extraction
//! target in the corpus gets its own snapshot file. When a snapshot changes,
//! `cargo insta review` shows exactly which file's extraction output differs.
//!
//! # Running
//!
//! These tests skip gracefully when the corpus is not synced.
//!
//! ```bash
//! # Sync the corpus:
//! cargo run -p djls-corpus -- sync
//!
//! # Run all corpus tests:
//! cargo test -p djls-python --test corpus -- --nocapture
//!
//! # Update snapshots after intentional changes:
//! INSTA_UPDATE=1 cargo test -p djls-python --test corpus
//! ```

use std::collections::BTreeMap;

use camino::Utf8Path;
use djls_corpus::module_path_from_file;
use djls_corpus::Corpus;
use djls_python::extract_rules;
use djls_python::BlockTagSpec;
use djls_python::ExtractionResult;
use djls_python::FilterArity;
use djls_python::SymbolKey;
use djls_python::TagRule;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct SortedExtractionResult {
    tag_rules: BTreeMap<String, TagRule>,
    filter_arities: BTreeMap<String, FilterArity>,
    block_specs: BTreeMap<String, BlockTagSpec>,
}

impl From<ExtractionResult> for SortedExtractionResult {
    fn from(result: ExtractionResult) -> Self {
        let key_str = |k: &SymbolKey| {
            let kind = match k.kind {
                djls_python::SymbolKind::Tag => "tag",
                djls_python::SymbolKind::Filter => "filter",
            };
            format!("{}::{kind}::{}", k.registration_module, k.name)
        };
        Self {
            tag_rules: result
                .tag_rules
                .iter()
                .map(|(k, v)| (key_str(k), v.clone()))
                .collect(),
            filter_arities: result
                .filter_arities
                .iter()
                .map(|(k, v)| (key_str(k), v.clone()))
                .collect(),
            block_specs: result
                .block_specs
                .iter()
                .map(|(k, v)| (key_str(k), v.clone()))
                .collect(),
        }
    }
}

fn snapshot(result: ExtractionResult) -> SortedExtractionResult {
    result.into()
}

fn is_extraction_target(path: &Utf8Path) -> bool {
    let path_str = path.as_str();
    if path_str.contains("__pycache__") {
        return false;
    }
    let is_py = path.extension().is_some_and(|ext| ext == "py");
    if !is_py {
        return false;
    }
    if path.file_name() == Some("__init__.py") {
        return false;
    }
    path_str.contains("/templatetags/")
        || (path_str.contains("/template/")
            && matches!(
                path.file_name(),
                Some("defaulttags.py" | "defaultfilters.py" | "loader_tags.py")
            ))
}

fn snapshot_dir() -> insta::internals::SettingsBindDropGuard {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots"));
    settings.bind_to_scope()
}

#[test]
fn extraction_snapshots() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };
    if corpus.extraction_targets().is_empty() {
        eprintln!("No extraction targets in corpus.");
        return;
    }

    insta::glob!(corpus.root().as_str(), "**/*.py", |path| {
        let path = Utf8Path::from_path(path).unwrap();
        if !is_extraction_target(path) {
            return;
        }
        let _guard = snapshot_dir();
        let source = std::fs::read_to_string(path).unwrap();
        let module_path = module_path_from_file(path);
        let result = extract_rules(&source, &module_path);
        insta::assert_yaml_snapshot!(snapshot(result));
    });
}

#[test]
fn test_django_versions_extraction() {
    let _guard = snapshot_dir();
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let django_packages = corpus.root().join("packages/Django");
    if !django_packages.as_std_path().exists() {
        eprintln!("Django packages not in corpus, skipping");
        return;
    }

    let django_dirs = corpus.synced_dirs("packages/Django");
    if django_dirs.is_empty() {
        eprintln!("No Django version dirs found, skipping");
        return;
    }

    let mut version_results: BTreeMap<String, VersionSummary> = BTreeMap::new();

    for django_dir in &django_dirs {
        let version = django_dir.file_name().unwrap().to_string();
        let defaulttags = django_dir.join("django/template/defaulttags.py");

        if defaulttags.as_std_path().exists() {
            let source = std::fs::read_to_string(defaulttags.as_std_path()).unwrap();
            let result = extract_rules(&source, "django.template.defaulttags");

            let mut tag_names: Vec<String> =
                result.tag_rules.keys().map(|k| k.name.clone()).collect();
            tag_names.sort();

            version_results.insert(
                version,
                VersionSummary {
                    tag_rule_count: result.tag_rules.len(),
                    filter_count: result.filter_arities.len(),
                    block_spec_count: result.block_specs.len(),
                    tag_names,
                },
            );
        }
    }

    if !version_results.is_empty() {
        insta::assert_yaml_snapshot!("django_versions_extraction", version_results);
    }
}

#[derive(Debug, Serialize)]
struct VersionSummary {
    tag_rule_count: usize,
    filter_count: usize,
    block_spec_count: usize,
    tag_names: Vec<String>,
}
