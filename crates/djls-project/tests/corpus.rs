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

use djls_project::PythonModuleName;
use djls_testing::Corpus;
use djls_testing::TestDatabase;
use djls_testing::extract_bundle;
use djls_testing::module_name_from_file;
use djls_testing::sorted_snapshot;

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
    let db = TestDatabase::new();

    for path in targets {
        let source = std::fs::read_to_string(path.as_std_path()).unwrap();
        let module_name = module_name_from_file(&path);
        db.add_file(path.as_str(), &source);
        let file = db.file(&path);
        let bundle = extract_bundle(&db, file, PythonModuleName::parse(&module_name).unwrap());

        let relative = path.strip_prefix(corpus.root()).unwrap_or(&path);
        let snapshot_name = relative.as_str().replace('/', "__");

        insta::assert_yaml_snapshot!(snapshot_name, sorted_snapshot(&bundle));
    }
}
