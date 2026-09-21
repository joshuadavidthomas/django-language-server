//! Corpus extraction snapshot tests.
//!
//! Uses path-derived snapshot names for per-file snapshot granularity — each
//! extraction target in the corpus gets its own snapshot file. When a snapshot
//! changes, `cargo insta review` shows exactly which file's extraction output differs.
//!
//! # Running
//!
//! These tests require corpus source and per-repository environments.
//!
//! ```bash
//! # Sync the corpus:
//! just corpus sync
//!
//! # Run all corpus tests:
//! cargo test -p djls-project --test corpus -- --nocapture
//!
//! # Update snapshots after intentional changes:
//! INSTA_UPDATE=1 cargo test -p djls-project --test corpus
//! ```

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

use djls_project::Db as _;
use djls_project::file_to_module;
use djls_testing::Corpus;
use djls_testing::extract_bundle;
use djls_testing::sorted_snapshot;
use libtest_mimic::Arguments;
use libtest_mimic::Trial;

fn snapshot_dir() -> insta::internals::SettingsBindDropGuard {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots"));
    settings.bind_to_scope()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = Arc::new(Corpus::require()?);
    let mut targets_by_member: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for target in corpus.extraction_target_members()? {
        targets_by_member
            .entry(target.member.clone())
            .or_default()
            .push(target);
    }
    if targets_by_member.is_empty() {
        return Err(io::Error::other("No extraction targets in corpus.").into());
    }
    let mut trials = Vec::new();
    for (name, _) in corpus.locked_repos() {
        let name = name.to_string();
        let deferred = corpus.environment_deferral(&name)?;
        let label = deferred.as_ref().map_or_else(
            || format!("corpus_environment::{name}"),
            |reason| format!("corpus_environment::{name} [deferred: {reason}]"),
        );
        let corpus = Arc::clone(&corpus);
        let targets = targets_by_member.remove(&name).unwrap_or_default();
        trials.push(
            Trial::test(label, move || {
                let db = corpus.environment_database(&name)?;
                let project = db
                    .project()
                    .ok_or_else(|| io::Error::other("missing corpus project"))?;
                let _guard = snapshot_dir();
                for target in targets {
                    let module =
                        file_to_module(&db, project, target.path.clone()).ok_or_else(|| {
                            io::Error::other(format!(
                                "target {} does not resolve in {name}",
                                target.relative_path
                            ))
                        })?;
                    let bundle = extract_bundle(&db, module.file(), module.name().clone());
                    let relative = target.path.strip_prefix(corpus.root())?;
                    let snapshot_name = relative.as_str().replace('/', "__");
                    insta::assert_yaml_snapshot!(snapshot_name, sorted_snapshot(&bundle)?);
                }
                Ok(())
            })
            .with_ignored_flag(deferred.is_some()),
        );
    }
    libtest_mimic::run(&Arguments::from_args(), trials).exit()
}
