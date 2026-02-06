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

use std::path::Path;
use std::path::PathBuf;

use djls_extraction::extract_rules;
use djls_extraction::ExtractionResult;

/// Get corpus root from environment, or use default if it exists.
fn corpus_root() -> Option<PathBuf> {
    // Explicit env var takes precedence
    if let Ok(path) = std::env::var("DJLS_CORPUS_ROOT") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try default location relative to workspace root
    // CARGO_MANIFEST_DIR points to crates/djls-extraction
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().and_then(|p| p.parent())?;
    let default = workspace_root.join("crates/djls-corpus/.corpus");
    if default.exists() {
        return Some(default);
    }

    None
}

/// Derive a module path from a file path within the corpus.
///
/// E.g., `.corpus/packages/Django/6.0.2/django/template/defaulttags.py`
/// → `"django.template.defaulttags"`
fn module_path_from_file(file: &Path) -> String {
    let components: Vec<&str> = file
        .components()
        .filter_map(|c| {
            if let std::path::Component::Normal(s) = c {
                s.to_str()
            } else {
                None
            }
        })
        .collect();

    // Find the first Python-package-looking component after the version directory.
    let mut start_idx = None;
    for (i, component) in components.iter().enumerate() {
        if component.chars().next().is_some_and(|c| c.is_ascii_digit())
            && component.contains('.')
            && !Path::new(component)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
        {
            start_idx = Some(i + 1);
        }
    }

    let start = start_idx.unwrap_or(0);
    let parts: Vec<&str> = components[start..]
        .iter()
        .map(|s| s.strip_suffix(".py").unwrap_or(s))
        .collect();
    parts.join(".")
}

/// Run extraction on a single file, returning the result if successful.
fn extract_file(path: &Path) -> Option<ExtractionResult> {
    let source = std::fs::read_to_string(path).ok()?;
    let module_path = module_path_from_file(path);
    let result = extract_rules(&source, &module_path);
    Some(result)
}

#[test]
fn test_corpus_extraction_no_panics() {
    let Some(root) = corpus_root() else {
        eprintln!("Corpus not available (run `cargo run -p djls-corpus -- sync` first)");
        eprintln!("Or set DJLS_CORPUS_ROOT to corpus location");
        return;
    };

    let files = djls_corpus::enumerate::enumerate_extraction_files(&root);
    assert!(
        !files.is_empty(),
        "Corpus should contain extraction targets"
    );

    let mut success_count = 0;
    let mut empty_count = 0;

    for path in &files {
        if let Some(result) = extract_file(path) {
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
    let Some(root) = corpus_root() else {
        return;
    };

    let files = djls_corpus::enumerate::enumerate_extraction_files(&root);

    let mut tags_found = 0;
    let mut filters_found = 0;
    let mut blocks_found = 0;
    let mut files_with_registrations = 0;

    for path in &files {
        if let Some(result) = extract_file(path) {
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
    let Some(root) = corpus_root() else {
        return;
    };

    let django_packages = root.join("packages/Django");
    if !django_packages.exists() {
        eprintln!("Django packages not in corpus, skipping");
        return;
    }

    let mut django_dirs: Vec<_> = std::fs::read_dir(&django_packages)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.chars().filter(|c| *c == '.').count() >= 2
        })
        .collect();

    django_dirs.sort();

    if django_dirs.is_empty() {
        eprintln!("No Django version dirs found, skipping");
        return;
    }

    let mut version_results: std::collections::BTreeMap<String, VersionSummary> =
        std::collections::BTreeMap::new();

    for django_dir in &django_dirs {
        let version = django_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let defaulttags = django_dir.join("django/template/defaulttags.py");

        if defaulttags.exists() {
            let source = std::fs::read_to_string(&defaulttags).unwrap();
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

#[derive(Debug, serde::Serialize)]
struct VersionSummary {
    tag_rule_count: usize,
    filter_count: usize,
    block_spec_count: usize,
    tag_names: Vec<String>,
}

#[test]
fn test_corpus_unsupported_patterns_summary() {
    let Some(root) = corpus_root() else {
        return;
    };

    let files = djls_corpus::enumerate::enumerate_extraction_files(&root);

    let mut total_tags = 0;
    let mut total_with_rules = 0;
    let mut total_with_block_spec = 0;

    for path in &files {
        if let Some(result) = extract_file(path) {
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
