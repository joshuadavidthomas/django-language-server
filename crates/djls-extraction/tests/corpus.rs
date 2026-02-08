//! Corpus extraction snapshot tests.
//!
//! Golden tests that extract rules from real Django and third-party Python
//! source and snapshot the results. Catch regressions in extraction output
//! across the entire corpus.
//!
//! # Running
//!
//! These tests skip gracefully when the corpus is not synced.
//!
//! ```bash
//! # Sync the corpus:
//! cargo run -p djls-corpus -- sync
//!
//! # Run corpus tests:
//! cargo test -p djls-extraction --test corpus -- --nocapture
//!
//! # Update snapshots after intentional changes:
//! cargo insta test --accept --unreferenced delete
//! ```

use std::collections::BTreeMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_corpus::module_path_from_file;
use djls_corpus::Corpus;
use djls_extraction::extract_rules;
use djls_extraction::BlockTagSpec;
use djls_extraction::ExtractionResult;
use djls_extraction::FilterArity;
use djls_extraction::SymbolKey;
use djls_extraction::TagRule;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct SortedExtractionResult {
    tag_rules: BTreeMap<String, TagRule>,
    filter_arities: BTreeMap<String, FilterArity>,
    block_specs: BTreeMap<String, BlockTagSpec>,
}

impl From<ExtractionResult> for SortedExtractionResult {
    fn from(result: ExtractionResult) -> Self {
        let key_str = |k: &SymbolKey| format!("{}::{}", k.registration_module, k.name);
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

fn sorted_subdirs(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir.as_std_path()) else {
        return Vec::new();
    };
    let mut dirs: Vec<Utf8PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_dir()))
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn test_django_core_modules_snapshots() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };

    let Some(django_dir) = corpus.latest_django() else {
        eprintln!("No Django in corpus");
        return;
    };

    let files = corpus.extraction_targets_in(&django_dir);
    assert!(!files.is_empty(), "Django should have extraction targets");

    for path in &files {
        let module_path = module_path_from_file(path);
        let result = corpus
            .extract_file(path)
            .expect("extraction should succeed");
        let snap_name = format!("django_core__{}", module_path.replace('.', "_"));
        insta::assert_yaml_snapshot!(snap_name, snapshot(result));
    }
}

#[test]
fn test_third_party_packages_snapshots() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available. Run `cargo run -p djls-corpus -- sync`.");
        return;
    };

    let mut entries: Vec<(String, Utf8PathBuf)> = Vec::new();

    let packages_dir = corpus.root().join("packages");
    if packages_dir.as_std_path().exists() {
        for name_dir in sorted_subdirs(&packages_dir) {
            let name = name_dir.file_name().unwrap().to_string();
            if name == "Django" {
                continue;
            }
            let synced = corpus.synced_dirs(&format!("packages/{name}"));
            if let Some(latest) = synced.last() {
                entries.push((name, latest.clone()));
            }
        }
    }

    let repos_dir = corpus.root().join("repos");
    if repos_dir.as_std_path().exists() {
        for name_dir in sorted_subdirs(&repos_dir) {
            let name = name_dir.file_name().unwrap().to_string();
            let synced = corpus.synced_dirs(&format!("repos/{name}"));
            if let Some(latest) = synced.last() {
                entries.push((name, latest.clone()));
            }
        }
    }

    assert!(!entries.is_empty(), "Should have non-Django corpus entries");

    for (name, dir) in &entries {
        let combined = corpus.extract_dir(dir);
        let snap_name = format!("thirdparty__{}", name.replace('-', "_").to_lowercase());
        insta::assert_yaml_snapshot!(snap_name, snapshot(combined));
    }
}

#[test]
fn test_django_versions_extraction() {
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
