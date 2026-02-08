//! Corpus-scale extraction tests.
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

use djls_corpus::Corpus;

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
fn test_corpus_django_versions_golden() {
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

    let mut version_results: std::collections::BTreeMap<String, VersionSummary> =
        std::collections::BTreeMap::new();

    for django_dir in &django_dirs {
        let version = django_dir.file_name().unwrap().to_string();
        let defaulttags = django_dir.join("django/template/defaulttags.py");

        if defaulttags.as_std_path().exists() {
            let source = std::fs::read_to_string(defaulttags.as_std_path()).unwrap();
            let result = djls_extraction::extract_rules(&source, "django.template.defaulttags");

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

#[derive(Debug, serde::Serialize)]
struct VersionSummary {
    tag_rule_count: usize,
    filter_count: usize,
    block_spec_count: usize,
    tag_names: Vec<String>,
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
