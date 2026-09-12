//! Corpus-backed snapshots for Django settings extraction.
//!
//! The corpus must be synced before running this suite:
//!
//! ```bash
//! just corpus sync
//! cargo test -p djls-project --test corpus_settings
//! ```

#[cfg(not(windows))]
use std::collections::BTreeSet;
#[cfg(not(windows))]
use std::io;

#[cfg(not(windows))]
use djls_project::Interpreter;
#[cfg(not(windows))]
use djls_project::testing::django_settings;
#[cfg(not(windows))]
use djls_project::testing::settings_module_file;
use djls_testing::Corpus;
#[cfg(not(windows))]
use djls_testing::OsTestDatabase;
#[cfg(not(windows))]
use djls_testing::ProjectFixture;
use libtest_mimic::Arguments;
#[cfg(not(windows))]
use libtest_mimic::Trial;
#[cfg(not(windows))]
use serde_json::Value;

#[cfg(not(windows))]
#[path = "support/corpus_settings.rs"]
mod corpus_settings_support;

#[cfg(not(windows))]
use corpus_settings_support::redact_repo_root;

#[cfg(not(windows))]
fn snapshot_dir() -> insta::internals::SettingsBindDropGuard {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/snapshots/settings"
    ));
    settings.bind_to_scope()
}

#[cfg(not(windows))]
fn installed_app_cases(settings: &Value) -> Result<(Vec<BTreeSet<&str>>, usize), io::Error> {
    let cases = settings
        .pointer("/installed_apps/cases")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("settings should contain installed-app cases"))?;
    let mut known = Vec::new();
    let mut dynamic_count = 0;

    for case in cases {
        if let Some(apps) = case.pointer("/known/apps").and_then(Value::as_array) {
            known.push(
                apps.iter()
                    .map(|app| {
                        app.get("value").and_then(Value::as_str).ok_or_else(|| {
                            io::Error::other("known installed app should be a string")
                        })
                    })
                    .collect::<Result<_, _>>()?,
            );
        } else if case.get("dynamic").is_some() {
            dynamic_count += 1;
        } else {
            return Err(io::Error::other(
                "installed-app case should be known or dynamic",
            ));
        }
    }

    Ok((known, dynamic_count))
}

#[cfg(not(windows))]
fn check_predicate_correlations(repo_name: &str, settings: &Value) -> Result<(), io::Error> {
    match repo_name {
        "archivebox" => {
            let (cases, dynamic_count) = installed_app_cases(settings)?;
            if cases.len() != 2 || dynamic_count != 0 {
                return Err(io::Error::other(format!(
                    "ArchiveBox should have two exact app cases and no dynamic case, found {} exact and {dynamic_count} dynamic",
                    cases.len()
                )));
            }
            if cases
                .iter()
                .any(|apps| apps.contains("django_autotyping") != apps.contains("requests_tracker"))
            {
                return Err(io::Error::other(
                    "ArchiveBox debug-controlled apps must occur together",
                ));
            }
            if !cases.iter().any(|apps| apps.contains("django_autotyping"))
                || !cases.iter().any(|apps| !apps.contains("django_autotyping"))
            {
                return Err(io::Error::other(
                    "ArchiveBox should retain both debug predicate outcomes",
                ));
            }
        }
        "inventree" => {
            let (cases, dynamic_count) = installed_app_cases(settings)?;
            if cases.len() != 12 || dynamic_count != 1 {
                return Err(io::Error::other(format!(
                    "InvenTree should have twelve exact app cases and one dynamic case, found {} exact and {dynamic_count} dynamic",
                    cases.len()
                )));
            }
            if cases
                .iter()
                .any(|apps| apps.contains("silk") && !apps.contains("sslserver"))
            {
                return Err(io::Error::other(
                    "InvenTree silk cases must also contain sslserver",
                ));
            }
            if !cases.iter().any(|apps| apps.contains("silk"))
                || !cases.iter().any(|apps| !apps.contains("silk"))
            {
                return Err(io::Error::other(
                    "InvenTree should retain both silk predicate outcomes",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

// The production evaluator deliberately leaves `os.path` calls unknown on
// Windows. These snapshots encode POSIX settings semantics.
#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Arguments::from_args();
    let corpus = Corpus::require()?;
    let declarations = corpus.repo_settings_projects()?;
    if declarations.is_empty() {
        return Err(io::Error::other(
            "corpus manifest should declare at least one Django settings module",
        )
        .into());
    }

    let interpreter = corpus.root().join("hermetic-no-venv");
    let mut snapshot_names = BTreeSet::new();
    let mut trials = Vec::new();

    for corpus_project in declarations {
        let repo_name = corpus_project.repo_name;
        let checkout_root = corpus_project.checkout_root;
        let project_root = corpus_project.project_root;

        for settings_module in corpus_project.django_settings_modules {
            let snapshot_name = format!("{repo_name}__{}", settings_module.replace('.', "__"));
            if !snapshot_names.insert(snapshot_name.clone()) {
                return Err(io::Error::other(format!(
                    "corpus settings modules produce duplicate snapshot name `{snapshot_name}`"
                ))
                .into());
            }

            let repo_name = repo_name.clone();
            let checkout_root = checkout_root.clone();
            let project_root = project_root.clone();
            let interpreter = interpreter.clone();
            trials.push(Trial::test(snapshot_name.clone(), move || {
                let _guard = snapshot_dir();
                let mut db = OsTestDatabase::with_disk_roots([checkout_root.clone()]);
                let interpreter = Interpreter::VenvPath(interpreter);
                let project = ProjectFixture::new(project_root.clone())
                    .django_settings_module(&settings_module)
                    .interpreter(interpreter)
                    .install(&mut db)?;

                settings_module_file(&db, project).ok_or_else(|| {
                    io::Error::other(format!(
                        "settings module `{settings_module}` for corpus repo `{repo_name}` did not resolve"
                    ))
                })?;
                let mut settings = serde_json::to_value(django_settings(&db, project))?;
                check_predicate_correlations(&repo_name, &settings)?;
                redact_repo_root(&mut settings, &checkout_root);

                insta::assert_yaml_snapshot!(snapshot_name, settings);
                Ok(())
            }));
        }
    }

    libtest_mimic::run(&args, trials).exit()
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Arguments::from_args();
    Corpus::require()?;
    libtest_mimic::run(&args, Vec::new()).exit()
}
