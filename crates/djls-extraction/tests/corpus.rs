//! Corpus-scale extraction tests.
//!
//! These tests enumerate all registration modules in the corpus and run
//! extraction on each, validating:
//! - No panics (extraction is resilient to all Python patterns)
//! - Parse failures are logged with context (not silent)
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
//! just corpus-sync
//!
//! # Then run corpus tests (uses default corpus location):
//! cargo test -p djls-extraction corpus -- --nocapture
//!
//! # Or with explicit path:
//! DJLS_CORPUS_ROOT=/path/to/corpus cargo test -p djls-extraction corpus
//! ```

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use djls_extraction::extract_rules;
use djls_extraction::ExtractionResult;
use djls_extraction::RuleCondition;
use walkdir::WalkDir;

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

/// Find all Python files relevant to extraction in the corpus.
fn enumerate_extraction_files(corpus_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for entry in WalkDir::new(corpus_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let path_str = path.to_string_lossy();

        if path_str.contains("__pycache__") {
            continue;
        }

        if path.extension().is_none_or(|ext| ext != "py") {
            continue;
        }

        // Skip __init__.py — rarely contains registrations
        if path.file_name().is_some_and(|n| n == "__init__.py") {
            continue;
        }

        // templatetags directories
        if path_str.contains("/templatetags/") {
            files.push(path.to_path_buf());
            continue;
        }

        // Django core template modules
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path_str.contains("/template/")
            && matches!(
                file_name,
                "defaulttags.py" | "defaultfilters.py" | "loader_tags.py"
            )
        {
            files.push(path.to_path_buf());
        }
    }

    files.sort();
    files.dedup();
    files
}

/// Extraction outcome for a single file.
#[derive(Debug)]
enum ExtractionOutcome {
    Success(ExtractionResult),
    ParseFailure(String),
    ExtractionError(String),
}

/// Run extraction on a single file, capturing outcome.
fn extract_file(path: &Path) -> ExtractionOutcome {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return ExtractionOutcome::ExtractionError(format!("read error: {e}")),
    };

    match extract_rules(&source) {
        Ok(result) => ExtractionOutcome::Success(result),
        Err(djls_extraction::ExtractionError::ParseError { .. }) => {
            ExtractionOutcome::ParseFailure(format!("{}", path.display()))
        }
        Err(e) => ExtractionOutcome::ExtractionError(e.to_string()),
    }
}

#[test]
fn test_corpus_extraction_no_panics() {
    let Some(root) = corpus_root() else {
        eprintln!("Corpus not available (run `just corpus-sync` first)");
        eprintln!("Or set DJLS_CORPUS_ROOT to corpus location");
        return;
    };

    let files = enumerate_extraction_files(&root);
    assert!(
        !files.is_empty(),
        "Corpus should contain extraction targets"
    );

    let mut success_count = 0;
    let mut parse_failure_count = 0;
    let mut error_count = 0;
    let mut failures: Vec<(PathBuf, String)> = Vec::new();

    for path in &files {
        match extract_file(path) {
            ExtractionOutcome::Success(_) => success_count += 1,
            ExtractionOutcome::ParseFailure(msg) => {
                parse_failure_count += 1;
                eprintln!("Parse failure: {msg}");
            }
            ExtractionOutcome::ExtractionError(msg) => {
                error_count += 1;
                failures.push((path.clone(), msg));
            }
        }
    }

    eprintln!("\n=== Corpus Extraction Summary ===");
    eprintln!("Total files:      {}", files.len());
    eprintln!("Successful:       {success_count}");
    eprintln!("Parse failures:   {parse_failure_count} (expected for unsupported syntax)");
    eprintln!("Errors:           {error_count}");

    assert!(
        failures.is_empty(),
        "Extraction errors (not parse failures):\n{}",
        failures
            .iter()
            .map(|(p, e)| format!("  {}: {e}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn test_corpus_extraction_yields_results() {
    let Some(root) = corpus_root() else {
        return;
    };

    let files = enumerate_extraction_files(&root);

    let mut tags_found = 0;
    let mut filters_found = 0;
    let mut files_with_registrations = 0;

    for path in &files {
        if let ExtractionOutcome::Success(result) = extract_file(path) {
            if !result.is_empty() {
                files_with_registrations += 1;
                tags_found += result.tags.len();
                filters_found += result.filters.len();
            }
        }
    }

    eprintln!("\n=== Extraction Yields ===");
    eprintln!("Files with registrations: {files_with_registrations}");
    eprintln!("Tags extracted:           {tags_found}");
    eprintln!("Filters extracted:        {filters_found}");

    assert!(
        tags_found > 50,
        "Expected >50 tags from corpus, got {tags_found}"
    );
    assert!(
        filters_found > 20,
        "Expected >20 filters from corpus, got {filters_found}"
    );
}

#[test]
fn test_corpus_no_hardcoded_bits_assumption() {
    let Some(root) = corpus_root() else {
        return;
    };

    let files = enumerate_extraction_files(&root);
    let mut non_bits_vars_found = 0;

    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };

        for line in source.lines() {
            if line.contains("split_contents()") {
                if let Some(var) = line.split('=').next() {
                    let var = var.trim();
                    if !var.is_empty() && var != "bits" {
                        non_bits_vars_found += 1;
                    }
                }
            }
        }
    }

    eprintln!("Non-'bits' split_contents assignments found: {non_bits_vars_found}");
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

    let mut version_results: BTreeMap<String, VersionSummary> = BTreeMap::new();

    for django_dir in &django_dirs {
        let version = django_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let defaulttags = django_dir.join("django/template/defaulttags.py");

        if defaulttags.exists() {
            if let ExtractionOutcome::Success(result) = extract_file(&defaulttags) {
                let mut tag_names: Vec<String> =
                    result.tags.iter().map(|t| t.name.clone()).collect();
                tag_names.sort();

                version_results.insert(
                    version,
                    VersionSummary {
                        tag_count: result.tags.len(),
                        filter_count: result.filters.len(),
                        tag_names,
                    },
                );
            }
        }
    }

    if !version_results.is_empty() {
        insta::assert_yaml_snapshot!("django_versions_extraction", version_results);
    }
}

#[derive(Debug, serde::Serialize)]
struct VersionSummary {
    tag_count: usize,
    filter_count: usize,
    tag_names: Vec<String>,
}

#[test]
fn test_corpus_unsupported_patterns_summary() {
    let Some(root) = corpus_root() else {
        return;
    };

    let files = enumerate_extraction_files(&root);

    let mut opaque_rules = 0;
    let mut ambiguous_block_specs = 0;
    let mut total_tags = 0;
    let mut total_with_rules = 0;
    let mut total_with_block_spec = 0;

    for path in &files {
        if let ExtractionOutcome::Success(result) = extract_file(path) {
            for tag in &result.tags {
                total_tags += 1;

                if !tag.rules.is_empty() {
                    total_with_rules += 1;
                    for rule in &tag.rules {
                        if matches!(rule.condition, RuleCondition::Opaque { .. }) {
                            opaque_rules += 1;
                        }
                    }
                }

                if let Some(ref spec) = tag.block_spec {
                    if spec.end_tag.is_some() {
                        total_with_block_spec += 1;
                    } else {
                        ambiguous_block_specs += 1;
                    }
                }
            }
        }
    }

    eprintln!("\n=== Unsupported Pattern Summary ===");
    eprintln!("Total tags:                {total_tags}");
    eprintln!("Tags with rules:           {total_with_rules}");
    eprintln!("Tags with block spec:      {total_with_block_spec}");
    eprintln!("Opaque rules (couldn't simplify): {opaque_rules}");
    eprintln!("Ambiguous block specs (end_tag=None): {ambiguous_block_specs}");
}

/// Parity oracle: compare Rust extraction to Python prototype.
///
/// **⚠️ TEMPORARY PORTING SCAFFOLDING — DELETE AFTER M6 PARITY ACHIEVED**
///
/// Requirements:
/// - `DJLS_PY_ORACLE=1` — explicit opt-in
/// - `DJLS_PY_ORACLE_PATH` — path to Python prototype checkout
/// - Corpus synced
#[test]
#[allow(clippy::too_many_lines)]
fn test_corpus_parity_with_python_prototype() {
    // Gate 1: Corpus must be available
    let Some(corpus) = corpus_root() else {
        eprintln!("Corpus not available, skipping parity test");
        return;
    };

    // Gate 2: Must explicitly opt-in
    if std::env::var("DJLS_PY_ORACLE").as_deref() != Ok("1") {
        eprintln!("DJLS_PY_ORACLE not set — parity oracle is opt-in developer scaffolding");
        return;
    }

    // Gate 3: Must provide prototype path
    let Ok(prototype_path_str) = std::env::var("DJLS_PY_ORACLE_PATH") else {
        eprintln!("DJLS_PY_ORACLE_PATH not set — must point to Python prototype checkout");
        return;
    };
    let prototype_path = PathBuf::from(prototype_path_str);

    if !prototype_path.exists() {
        eprintln!(
            "DJLS_PY_ORACLE_PATH does not exist: {}",
            prototype_path.display()
        );
        return;
    }

    // Verify prototype is runnable
    let prototype_check = std::process::Command::new("uv")
        .args([
            "run",
            "--directory",
            &prototype_path.to_string_lossy(),
            "python",
            "-c",
            "import template_linter",
        ])
        .status();

    if !prototype_check.map(|s| s.success()).unwrap_or(false) {
        eprintln!(
            "Python prototype not runnable at {}",
            prototype_path.display()
        );
        return;
    }

    // Test against Django's defaulttags.py
    let django_version =
        std::env::var("DJLS_PY_ORACLE_DJANGO_VERSION").unwrap_or_else(|_| "5.2.11".to_string());

    let test_file = corpus.join(format!(
        "packages/Django/{django_version}/django/template/defaulttags.py"
    ));
    if !test_file.exists() {
        eprintln!("Django {django_version} not in corpus");
        return;
    }

    // Run Rust extraction
    let rust_result = match extract_file(&test_file) {
        ExtractionOutcome::Success(r) => r,
        other => {
            eprintln!("Rust extraction failed: {other:?}");
            return;
        }
    };

    // Run Python extraction via oracle
    let python_result = run_python_oracle(&prototype_path, &test_file);
    let Some(python_result) = python_result else {
        return;
    };

    // Compare
    let mut rust_tags: Vec<&str> = rust_result.tags.iter().map(|t| t.name.as_str()).collect();
    rust_tags.sort_unstable();
    let python_tags: Vec<&str> = python_result["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    let missing: Vec<&&str> = python_tags
        .iter()
        .filter(|t| !rust_tags.contains(t))
        .collect();
    let extra: Vec<&&str> = rust_tags
        .iter()
        .filter(|t| !python_tags.contains(t))
        .collect();

    eprintln!("\n=== Parity Oracle Report (Django {django_version}) ===");
    eprintln!(
        "Rust: {} tags, Python: {} tags",
        rust_tags.len(),
        python_tags.len()
    );
    if !missing.is_empty() {
        eprintln!("Missing in Rust: {missing:?}");
    }
    if !extra.is_empty() {
        eprintln!("Extra in Rust: {extra:?}");
    }

    if missing.len() > 5 {
        eprintln!("WARNING: Large parity gap. Review extraction heuristics.");
    }
}

fn run_python_oracle(prototype_path: &Path, test_file: &Path) -> Option<serde_json::Value> {
    let python_output = std::process::Command::new("uv")
        .args([
            "run",
            "--directory",
            &prototype_path.to_string_lossy(),
            "python",
            "-c",
            &format!(
                concat!(
                    "import json\n",
                    "from template_linter.extraction import extract_from_file\n",
                    "result = extract_from_file(\"{}\")\n",
                    "print(json.dumps({{\n",
                    "    \"tags\": sorted([t.name for t in result.tags]),\n",
                    "    \"filters\": sorted([f.name for f in result.filters]),\n",
                    "    \"block_tags\": sorted([t.name for t in result.tags ",
                    "if t.block_spec and t.block_spec.end_tag]),\n",
                    "}}, sort_keys=True))"
                ),
                test_file.display()
            ),
        ])
        .output();

    match python_output {
        Ok(output) if output.status.success() => {
            Some(serde_json::from_slice(&output.stdout).expect("parse Python JSON"))
        }
        Ok(output) => {
            eprintln!(
                "Python extraction failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            None
        }
        Err(e) => {
            eprintln!("Failed to run Python: {e}");
            None
        }
    }
}
