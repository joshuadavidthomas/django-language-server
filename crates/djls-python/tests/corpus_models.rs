//! Corpus model extraction snapshot tests.
//!
//! Runs `extract_model_graph` against every `models.py` in the corpus
//! and snapshots the result. Each file gets its own snapshot.
//!
//! # Running
//!
//! These tests require the corpus to be synced.
//!
//! ```bash
//! # Sync the corpus:
//! cargo run -p djls-corpus -- sync -U
//!
//! # Run model corpus tests:
//! cargo test -p djls-python --test corpus_models -- --nocapture
//!
//! # Update snapshots after intentional changes:
//! INSTA_UPDATE=1 cargo test -p djls-python --test corpus_models
//! ```

use djls_corpus::module_path_from_file;
use djls_corpus::Corpus;
use djls_python::models::extract_model_graph;

fn snapshot_dir() -> insta::internals::SettingsBindDropGuard {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/snapshots/models"
    ));
    settings.bind_to_scope()
}

#[test]
fn model_extraction_snapshots() {
    let corpus = Corpus::require();
    let targets = corpus.model_files();
    assert!(!targets.is_empty(), "No model files in corpus.");

    let _guard = snapshot_dir();

    for path in targets {
        let source = std::fs::read_to_string(path.as_std_path()).unwrap();
        let module_path = module_path_from_file(&path);
        let graph = extract_model_graph(&source, &module_path);

        // Skip files that produce no models — they're likely just
        // re-exports or empty __init__-style modules
        if graph.is_empty() {
            continue;
        }

        let relative = path.strip_prefix(corpus.root()).unwrap_or(&path);
        let snapshot_name = relative.as_str().replace('/', "__");

        insta::assert_yaml_snapshot!(snapshot_name, graph);
    }
}
