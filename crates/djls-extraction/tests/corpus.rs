//! Corpus extraction tests.
//!
//! These tests enumerate all registration modules in the corpus and run
//! extraction on each, validating:
//! - No panics (extraction is resilient to all Python patterns)
//! - Stable serialization (golden snapshots for key entries)
//! - Key invariants hold across the corpus
//!
//! # Running
//!
//! These tests are gated — they skip gracefully when the corpus is not synced.
//! Default corpus location: `crates/djls-corpus/.corpus/`
//!
//! ```bash
//! # First, sync the corpus:
//! cargo run -p djls-corpus -- sync
//!
//! # Then run corpus tests (uses default corpus location):
//! cargo test -p djls-extraction --test corpus -- --nocapture
//!
//! # Or with explicit path:
//! DJLS_CORPUS_ROOT=/path/to/corpus cargo test -p djls-extraction --test corpus
//! ```

use std::collections::BTreeMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_corpus::enumerate::FileKind;
use djls_corpus::module_path_from_file;
use djls_corpus::Corpus;
use djls_extraction::extract_rules;
use djls_extraction::ArgumentCountConstraint;
use djls_extraction::BlockTagSpec;
use djls_extraction::ExtractionResult;
use djls_extraction::FilterArity;
use djls_extraction::SymbolKey;
use djls_extraction::TagRule;
use serde::Serialize;

/// A deterministically-ordered version of `ExtractionResult` for snapshot testing.
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

/// Extract from a real Django source file in the corpus.
fn extract_django_module(
    corpus: &Corpus,
    relative: &str,
    module_path: &str,
) -> Option<ExtractionResult> {
    let django_dir = corpus.latest_django()?;
    let path = django_dir.join(relative);
    if !path.as_std_path().exists() {
        return None;
    }
    let source = std::fs::read_to_string(path.as_std_path()).ok()?;
    Some(extract_rules(&source, module_path))
}

/// Sorted subdirectories of a path (for deterministic iteration).
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

// Corpus-wide tests

#[test]
fn test_corpus_extraction_no_panics() {
    let Some(corpus) = Corpus::discover() else {
        eprintln!("Corpus not available (run `cargo run -p djls-corpus -- sync` first)");
        eprintln!("Or set DJLS_CORPUS_ROOT to corpus location");
        return;
    };

    let files = corpus.extraction_targets();
    assert!(
        !files.is_empty(),
        "Corpus should contain extraction targets"
    );

    let mut success_count = 0;
    let mut empty_count = 0;

    for path in &files {
        if let Some(result) = corpus.extract_file(path) {
            if result.is_empty() {
                empty_count += 1;
            } else {
                success_count += 1;
            }
        }
    }

    eprintln!("\n=== Corpus Extraction Summary ===");
    eprintln!("Total files:        {}", files.len());
    eprintln!("With registrations: {success_count}");
    eprintln!("Empty results:      {empty_count}");
}

#[test]
fn test_corpus_extraction_yields_results() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let files = corpus.extraction_targets();

    let mut tags_found = 0;
    let mut filters_found = 0;
    let mut blocks_found = 0;
    let mut files_with_registrations = 0;

    for path in &files {
        if let Some(result) = corpus.extract_file(path) {
            if !result.is_empty() {
                files_with_registrations += 1;
                tags_found += result.tag_rules.len();
                filters_found += result.filter_arities.len();
                blocks_found += result.block_specs.len();
            }
        }
    }

    eprintln!("\n=== Extraction Yields ===");
    eprintln!("Files with registrations: {files_with_registrations}");
    eprintln!("Tag rules extracted:      {tags_found}");
    eprintln!("Filter arities extracted: {filters_found}");
    eprintln!("Block specs extracted:    {blocks_found}");

    assert!(
        tags_found > 20,
        "Expected >20 tag rules from corpus, got {tags_found}"
    );
    assert!(
        filters_found > 20,
        "Expected >20 filter arities from corpus, got {filters_found}"
    );
}

#[test]
fn test_corpus_unsupported_patterns_summary() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let files = corpus.extraction_targets();

    let mut total_tags = 0;
    let mut total_with_rules = 0;
    let mut total_with_block_spec = 0;

    for path in &files {
        if let Some(result) = corpus.extract_file(path) {
            total_tags += result.tag_rules.len() + result.block_specs.len();
            total_with_rules += result.tag_rules.len();
            total_with_block_spec += result.block_specs.len();
        }
    }

    eprintln!("\n=== Pattern Summary ===");
    eprintln!("Total tag-related extractions: {total_tags}");
    eprintln!("Tags with rules:              {total_with_rules}");
    eprintln!("Tags with block spec:         {total_with_block_spec}");
}

// Data-driven snapshot tests

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

    let files = corpus.enumerate_files(&django_dir, FileKind::ExtractionTarget);
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

// Specific assertion tests

#[test]
fn test_defaulttags_tag_count() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();

    let mut tag_names: Vec<&str> = result.tag_rules.keys().map(|k| k.name.as_str()).collect();
    tag_names.sort_unstable();

    eprintln!("defaulttags.py tags with rules: {tag_names:?}");
    assert!(
        result.tag_rules.len() >= 5,
        "Expected >= 5 tag rules from defaulttags.py, got {}",
        result.tag_rules.len()
    );
}

#[test]
fn test_defaulttags_for_tag_rules() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();
    let for_key = SymbolKey::tag("django.template.defaulttags", "for");
    let for_rule = result
        .tag_rules
        .get(&for_key)
        .expect("for tag should have extracted rules");

    eprintln!("for tag constraints ({}):", for_rule.arg_constraints.len());
    for constraint in &for_rule.arg_constraints {
        eprintln!("  {constraint:?}");
    }

    assert!(
        for_rule
            .arg_constraints
            .iter()
            .any(|c| matches!(c, ArgumentCountConstraint::Min(4))),
        "for tag should have Min(4) constraint from `len(bits) < 4`"
    );

    let for_block = result
        .block_specs
        .get(&for_key)
        .expect("for tag should have block spec");
    assert_eq!(for_block.end_tag.as_deref(), Some("endfor"));
    assert!(
        for_block.intermediates.contains(&"empty".to_string()),
        "for tag should have 'empty' intermediate"
    );
}

#[test]
fn test_defaulttags_if_tag() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();

    let if_key = SymbolKey::tag("django.template.defaulttags", "if");
    let block = result
        .block_specs
        .get(&if_key)
        .expect("if tag should have block spec");
    assert_eq!(block.end_tag.as_deref(), Some("endif"));
    assert!(
        block.intermediates.contains(&"elif".to_string()),
        "should have elif"
    );
    assert!(
        block.intermediates.contains(&"else".to_string()),
        "should have else"
    );
}

#[test]
fn test_defaulttags_url_tag_rules() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();
    let url_key = SymbolKey::tag("django.template.defaulttags", "url");
    let url_rule = result
        .tag_rules
        .get(&url_key)
        .expect("url tag should have extracted rules");

    eprintln!("url tag constraints ({}):", url_rule.arg_constraints.len());
    for constraint in &url_rule.arg_constraints {
        eprintln!("  {constraint:?}");
    }

    assert!(
        url_rule
            .arg_constraints
            .iter()
            .any(|c| matches!(c, ArgumentCountConstraint::Min(2))),
        "url tag should have Min(2) constraint from `len(bits) < 2`"
    );
}

#[test]
fn test_defaulttags_with_tag() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaulttags.py",
        "django.template.defaulttags",
    )
    .unwrap();
    let with_key = SymbolKey::tag("django.template.defaulttags", "with");
    let block = result
        .block_specs
        .get(&with_key)
        .expect("with tag should have block spec");
    assert_eq!(block.end_tag.as_deref(), Some("endwith"));
}

#[test]
fn test_defaultfilters_filter_count() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/defaultfilters.py",
        "django.template.defaultfilters",
    )
    .unwrap();

    let mut filter_names: Vec<&str> = result
        .filter_arities
        .keys()
        .map(|k| k.name.as_str())
        .collect();
    filter_names.sort_unstable();

    eprintln!("defaultfilters.py filters ({}):", filter_names.len());
    for name in &filter_names {
        let key = result
            .filter_arities
            .keys()
            .find(|k| k.name == *name)
            .unwrap();
        let arity = &result.filter_arities[key];
        eprintln!("  {name}: {arity:?}");
    }

    assert!(
        result.filter_arities.len() >= 50,
        "Expected >= 50 filters from defaultfilters.py, got {}",
        result.filter_arities.len()
    );
}

#[test]
fn test_loader_tags_block_tag() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let result = extract_django_module(
        &corpus,
        "django/template/loader_tags.py",
        "django.template.loader_tags",
    )
    .unwrap();
    let block_key = SymbolKey::tag("django.template.loader_tags", "block");
    let block = result
        .block_specs
        .get(&block_key)
        .expect("block tag should have block spec");
    assert_eq!(block.end_tag.as_deref(), Some("endblock"));
}

#[test]
fn test_for_tag_rules_across_django_versions() {
    let Some(corpus) = Corpus::discover() else {
        return;
    };

    let django_packages = corpus.root().join("packages/Django");
    if !django_packages.as_std_path().exists() {
        return;
    }

    let version_dirs = corpus.synced_dirs("packages/Django");

    for version_dir in &version_dirs {
        let version = version_dir.file_name().unwrap();
        let defaulttags = version_dir.join("django/template/defaulttags.py");
        if !defaulttags.as_std_path().exists() {
            continue;
        }

        let source = std::fs::read_to_string(defaulttags.as_std_path()).unwrap();
        let result = extract_rules(&source, "django.template.defaulttags");
        let for_key = SymbolKey::tag("django.template.defaulttags", "for");

        if let Some(for_rule) = result.tag_rules.get(&for_key) {
            eprintln!(
                "Django {version} for tag: {} constraints",
                for_rule.arg_constraints.len()
            );
            for constraint in &for_rule.arg_constraints {
                eprintln!("  {constraint:?}");
            }

            assert!(
                for_rule
                    .arg_constraints
                    .iter()
                    .any(|c| matches!(c, ArgumentCountConstraint::Min(4))),
                "Django {version}: for tag missing Min(4) constraint"
            );
        } else {
            eprintln!("Django {version}: for tag has no extracted rules");
        }
    }
}
