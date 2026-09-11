use std::fmt::Write as _;
use std::fs;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_testing::Corpus;
use djls_testing::TestDatabase;
use djls_testing::build_entry_specs;
use djls_testing::collect_argument_validation_errors_with_revision;
use libtest_mimic::Arguments;
use libtest_mimic::Trial;

struct FailureEntry {
    path: Utf8PathBuf,
    errors: Vec<String>,
}

fn format_failures(failures: &[FailureEntry]) -> Result<String, std::fmt::Error> {
    let mut out = String::new();
    for failure in failures.iter().take(20) {
        writeln!(out, "  {}:", failure.path)?;
        for error in &failure.errors {
            writeln!(out, "    - {error}")?;
        }
    }
    if failures.len() > 20 {
        writeln!(out, "  ... and {} more", failures.len() - 20)?;
    }
    Ok(out)
}

#[expect(
    clippy::expect_used,
    reason = "corpus fixture failures should fail their named trial"
)]
fn validate_repo(corpus: &Corpus, entry_dir: &Utf8Path, templates: Vec<Utf8PathBuf>) {
    let (specs, arities) = build_entry_specs(corpus, entry_dir)
        .expect("corpus entry tag and filter specs should build");
    let db = TestDatabase::new()
        .with_projectless_tag_specs(specs)
        .with_projectless_filter_arity_specs(arities);
    let mut failures = Vec::new();

    for (i, template_path) in templates.into_iter().enumerate() {
        let Ok(content) = fs::read_to_string(template_path.as_std_path()) else {
            continue;
        };

        let errors = collect_argument_validation_errors_with_revision(
            &db,
            "corpus_test.html",
            i as u64,
            &content,
        )
        .expect("corpus template argument errors should be collected");
        if errors.is_empty() {
            continue;
        }

        failures.push(FailureEntry {
            path: template_path,
            errors: errors
                .into_iter()
                .take(5)
                .map(|error| format!("{error:?}"))
                .collect(),
        });
    }

    assert!(
        failures.is_empty(),
        "Corpus templates have false positives:\n{}",
        format_failures(&failures).expect("corpus failures should format")
    );
}

fn main() -> anyhow::Result<()> {
    let args = Arguments::from_args();
    let corpus = Arc::new(Corpus::require()?);
    let trials = corpus
        .locked_repos()
        .filter_map(|(repo_name, entry_dir)| {
            let templates = corpus.templates_in(&entry_dir);
            if templates.is_empty() {
                return None;
            }

            let corpus = Arc::clone(&corpus);
            Some(Trial::test(repo_name, move || {
                validate_repo(&corpus, &entry_dir, templates);
                Ok(())
            }))
        })
        .collect();

    libtest_mimic::run(&args, trials).exit()
}
