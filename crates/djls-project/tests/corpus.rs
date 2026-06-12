//! Corpus extraction snapshot tests.
//!
//! Uses `insta::glob!` for per-file snapshot granularity — each extraction
//! target in the corpus gets its own snapshot file. When a snapshot changes,
//! `cargo insta review` shows exactly which file's extraction output differs.
//!
//! # Running
//!
//! These tests require the corpus to be synced.
//!
//! ```bash
//! # Sync the corpus:
//! cargo run -p djls-testing --bin corpus -- sync -U
//!
//! # Run all corpus tests:
//! cargo test -p djls-project --test corpus -- --nocapture
//!
//! # Update snapshots after intentional changes:
//! INSTA_UPDATE=1 cargo test -p djls-project --test corpus
//! ```

use std::collections::BTreeMap;

use djls_project::ExtractionResult;
use djls_project::FilterArity;
use djls_project::SymbolKey;
use djls_project::TagRule;
use djls_project::extract_rules;
use djls_testing::Corpus;
use djls_testing::module_path_from_file;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct SortedExtractionResult {
    tag_rules: BTreeMap<String, TagRule>,
    filter_arities: BTreeMap<String, FilterArity>,
    block_specs: BTreeMap<String, serde_json::Value>,
}

impl From<ExtractionResult> for SortedExtractionResult {
    fn from(result: ExtractionResult) -> Self {
        let key_str = |k: &SymbolKey| {
            let kind = match k.kind {
                djls_project::SymbolKind::Tag => "tag",
                djls_project::SymbolKind::Filter => "filter",
            };
            format!("{}::{kind}::{}", k.registration_module, k.name)
        };
        Self {
            tag_rules: result
                .tag_rules
                .iter()
                .map(|(k, v)| (key_str(k), v.as_ref().clone()))
                .collect(),
            filter_arities: result
                .filter_arities
                .iter()
                .map(|(k, v)| (key_str(k), v.clone()))
                .collect(),
            block_specs: serde_json::from_value(serde_json::to_value(&result.block_specs).unwrap())
                .unwrap(),
        }
    }
}

fn snapshot(result: ExtractionResult) -> SortedExtractionResult {
    result.into()
}

fn snapshot_dir() -> insta::internals::SettingsBindDropGuard {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots"));
    settings.bind_to_scope()
}

#[test]
fn extraction_snapshots() {
    let corpus = Corpus::require();
    let targets = corpus.extraction_targets();
    assert!(!targets.is_empty(), "No extraction targets in corpus.");

    let _guard = snapshot_dir();

    for path in targets {
        let source = std::fs::read_to_string(path.as_std_path()).unwrap();
        let module_path = module_path_from_file(&path);
        let result = extract_rules(&source, &module_path);

        let relative = path.strip_prefix(corpus.root()).unwrap_or(&path);
        let snapshot_name = relative.as_str().replace('/', "__");

        insta::assert_yaml_snapshot!(snapshot_name, snapshot(result));
    }
}
