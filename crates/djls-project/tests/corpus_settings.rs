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

use camino::Utf8Path;
#[cfg(not(windows))]
use djls_project::Interpreter;
#[cfg(not(windows))]
use djls_project::Project;
#[cfg(not(windows))]
use djls_project::PythonModuleName;
#[cfg(not(windows))]
use djls_project::SearchPaths;
#[cfg(not(windows))]
use djls_project::testing::django_settings;
#[cfg(not(windows))]
use djls_project::testing::settings_module_file;
#[cfg(not(windows))]
use djls_source::Db as _;
use djls_testing::Corpus;
#[cfg(not(windows))]
use djls_testing::OsTestDatabase;
use serde_json::Value;

#[cfg(not(windows))]
fn snapshot_dir() -> insta::internals::SettingsBindDropGuard {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/snapshots/settings"
    ));
    settings.bind_to_scope()
}

fn redact_repo_root(value: &mut Value, repo_root: &Utf8Path) {
    match value {
        Value::String(text) => {
            let normalized_text = text.replace('\\', "/");
            let normalized_repo_root = repo_root.as_str().replace('\\', "/");
            if let Ok(relative) =
                Utf8Path::new(&normalized_text).strip_prefix(Utf8Path::new(&normalized_repo_root))
            {
                *text = if relative.as_str().is_empty() {
                    "${REPO}".to_string()
                } else {
                    format!("${{REPO}}/{relative}")
                };
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_repo_root(value, repo_root);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                redact_repo_root(value, repo_root);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
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

#[test]
fn corpus_is_synced() -> Result<(), Box<dyn std::error::Error>> {
    Corpus::require()?;
    Ok(())
}

// The production evaluator deliberately leaves `os.path` calls unknown on
// Windows. These snapshots encode POSIX settings semantics; path redaction is
// tested on every platform below.
#[cfg(not(windows))]
#[test]
fn settings_extraction_snapshots() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = Corpus::require()?;
    let declarations = corpus.repo_settings_projects()?;
    if declarations.is_empty() {
        return Err(io::Error::other(
            "corpus manifest should declare at least one Django settings module",
        )
        .into());
    }
    let _guard = snapshot_dir();
    let mut snapshot_names = BTreeSet::new();

    for corpus_project in declarations {
        let repo_name = &corpus_project.repo_name;
        let checkout_root = &corpus_project.checkout_root;
        let project_root = &corpus_project.project_root;

        for settings_module in corpus_project.django_settings_modules {
            let mut db = OsTestDatabase::new();
            let interpreter = Interpreter::VenvPath(corpus.root().join("hermetic-no-venv"));
            let pythonpath = Vec::new();
            let search_paths = SearchPaths::from_project_settings(
                db.file_system(),
                project_root.as_path(),
                &interpreter,
                &pythonpath,
            );
            search_paths.register_roots(&db);
            let project = Project::new(
                &db,
                project_root.clone(),
                search_paths,
                interpreter,
                Some(PythonModuleName::parse(&settings_module)?),
                pythonpath,
                Vec::new(),
                djls_conf::Settings::default().tagspecs().clone(),
            );
            db.set_project(project);

            settings_module_file(&db, project).ok_or_else(|| {
                io::Error::other(format!(
                    "settings module `{settings_module}` for corpus repo `{repo_name}` did not resolve"
                ))
            })?;
            let mut settings = serde_json::to_value(django_settings(&db, project))?;
            check_predicate_correlations(repo_name, &settings)?;
            redact_repo_root(&mut settings, checkout_root);

            let snapshot_name = format!("{repo_name}__{}", settings_module.replace('.', "__"));
            if !snapshot_names.insert(snapshot_name.clone()) {
                return Err(io::Error::other(format!(
                    "corpus settings modules produce duplicate snapshot name `{snapshot_name}`"
                ))
                .into());
            }
            insta::assert_yaml_snapshot!(snapshot_name, settings);
        }
    }

    Ok(())
}

#[test]
fn repo_root_redaction_rewrites_nested_string_values() {
    let mut value = serde_json::json!({
        "path": "/corpus/repo/templates",
        "nested": ["/corpus/repo", "unchanged"],
        "prefix_collision": "/corpus/repository/templates",
    });

    redact_repo_root(&mut value, Utf8Path::new("/corpus/repo"));

    assert_eq!(
        value,
        serde_json::json!({
            "path": "${REPO}/templates",
            "nested": ["${REPO}", "unchanged"],
            "prefix_collision": "/corpus/repository/templates",
        })
    );

    let mut windows_value = serde_json::json!({
        "path": r"C:\corpus\repo\templates",
        "prefix_collision": r"C:\corpus\repository\templates",
    });
    redact_repo_root(&mut windows_value, Utf8Path::new(r"C:\corpus\repo"));
    assert_eq!(
        windows_value,
        serde_json::json!({
            "path": "${REPO}/templates",
            "prefix_collision": r"C:\corpus\repository\templates",
        })
    );
}
