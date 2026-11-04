//! Diagnostic coverage in real application environments, not a diagnostic correctness score.
//!
//! Run after `just corpus sync`: `cargo test -p djls-ide --test corpus_coverage`.
//! `DJLS_COVERAGE_JSON=<path>` exports a versioned JSON document, including filtered-run
//! completeness. `DJLS_COVERAGE_SCAN=1` also scans first-party IDE template targets
//! from `Corpus::templates_in`, without a file limit, and exports per-file diagnostics.
//! Scan results are observations for differential review, not zero-false-positive assertions.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use djls_lsp::AnalysisHost;
use djls_lsp::Workspace;
use djls_project::Db as _;
use djls_project::testing;
use djls_source::PositionEncoding;
use djls_testing::Corpus;
use libtest_mimic::Arguments;
use libtest_mimic::Trial;
use serde_json::Value;
use serde_json::json;
use tower_lsp_server::ls_types;

// Application cohort from the diagnostic investigation. Package repositories exercise
// extraction elsewhere; they are not standalone applications with editor discovery.
const APPS: &[&str] = &[
    "archivebox",
    "babybuddy",
    "django-crm",
    "djangopackages.org",
    "djangoproject.com",
    "geonode",
    "healthchecks",
    "horizon",
    "hyperkitty",
    "inventree",
    "linkding",
    "misago",
    "netbox",
    "nomnom",
    "pretix",
    "pythonic-news",
    "readthedocs.org",
    "sentry",
    "simonwillison.net",
];

// An unknown load can legitimately suppress later name errors. Keep it after the
// unknown names, and keep the unclosed block last so the checks remain independent.
const PROBE: &str = "{% djls_coverage_unknown_tag %}\n{{ value|djls_coverage_unknown_filter }}\n{% block a %}{% endblock b %}\n{% load djls_coverage_unknown_library %}\n{% if x %}\n";
const CHECKS: &[(&str, u32)] = &[
    ("S108", 0),
    ("S111", 1),
    ("S103", 2),
    ("S120", 3),
    ("S100", 4),
];

fn location(
    db: &dyn djls_source::Db,
    origin: djls_source::Origin,
    checkout: &Utf8Path,
    environment: &Utf8Path,
) -> String {
    let path = origin.file.path(db);
    let relative = path.strip_prefix(checkout).map_or_else(
        |_| {
            path.strip_prefix(environment).map_or_else(
                |_| path.to_string(),
                |relative| {
                    relative.as_str().split_once("site-packages/").map_or_else(
                        || format!("$ENV/{relative}"),
                        |(_, module)| format!("$SITE_PACKAGES/{module}"),
                    )
                },
            )
        },
        ToString::to_string,
    );
    let (line, _): (u32, u32) = origin
        .file
        .line_index(db)
        .to_line_col(djls_source::Offset::new(origin.span.start()))
        .into();
    format!("{relative}:{}", line + 1)
}

fn app_names(value: &Value, result: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(name) = object.get("value").and_then(Value::as_str) {
                result.insert(name.to_string());
            }
            for child in object.values() {
                app_names(child, result);
            }
        }
        Value::Array(array) => {
            for child in array {
                app_names(child, result);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn case_counts(settings: &Value, setting: &str) -> Result<Value, io::Error> {
    let cases = settings[setting]["cases"]
        .as_array()
        .ok_or_else(|| io::Error::other("missing setting cases"))?;
    let mut counts = BTreeMap::from([("known", 0), ("dynamic", 0), ("malformed", 0), ("unset", 0)]);
    for case in cases {
        let kind = if case.as_str() == Some("unset") {
            "unset"
        } else {
            *counts
                .keys()
                .find(|&&key| case.get(key).is_some())
                .ok_or_else(|| io::Error::other("unrecognized setting case"))?
        };
        *counts
            .get_mut(kind)
            .ok_or_else(|| io::Error::other("unrecognized setting case"))? += 1;
    }
    Ok(json!(counts))
}

fn diagnostic_code(diagnostic: &ls_types::Diagnostic) -> Option<&str> {
    match &diagnostic.code {
        Some(ls_types::NumberOrString::String(code)) => Some(code),
        _ => None,
    }
}

fn project_observations(
    db: &dyn djls_project::Db,
    project: djls_project::Project,
    recipe: &djls_testing::CorpusEnvironmentProject,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let settings = serde_json::to_value(testing::django_settings(db, project))?;
    let module = project
        .django_settings_module(db)
        .as_ref()
        .map(|module| module.as_str().to_string());
    let source = if recipe.django_settings_module.is_some() {
        "config"
    } else if std::env::var("DJANGO_SETTINGS_MODULE").is_ok_and(|value| !value.is_empty()) {
        "env"
    } else if module.is_some() {
        "manage.py"
    } else {
        "none"
    };
    let mut uncertainty = BTreeSet::new();
    for (setting, kind, origins) in testing::django_settings_uncertainty(db, project) {
        let mut locations: Vec<_> = origins
            .into_iter()
            .map(|origin| location(db, origin, &recipe.checkout_root, &recipe.environment_root))
            .collect();
        locations.sort();
        locations.dedup();
        uncertainty.insert((setting, kind, locations));
    }
    let uncertainty: Vec<_> = uncertainty.into_iter().map(|(setting, kind, locations)| json!({"setting": setting, "kind": kind, "locations": locations})).collect();
    let mut apps = BTreeSet::new();
    app_names(&settings["installed_apps"], &mut apps);
    let unresolved: Vec<_> = apps
        .into_iter()
        .filter(|app| !testing::installed_app_resolves(db, project, app))
        .collect();
    let scoped = djls_project::ScopedTemplateLibraries::from_project_inventory(
        djls_project::template_library_catalog(db, project),
    );
    let mut unread = Vec::new();
    for library in scoped.resolved_libraries() {
        if !library.symbols_are_unobserved() {
            continue;
        }
        let facts = djls_project::template_library_definition_facts(db, library.id());
        let reasons: Vec<_> = facts
            .unread_registrations()
            .iter()
            .map(|registration| {
                let at = library.source_file().map(|file| {
                    location(
                        db,
                        djls_source::Origin::new(file, registration.span),
                        &recipe.checkout_root,
                        &recipe.environment_root,
                    )
                });
                json!({"reason": registration.shape.to_string(), "at": at})
            })
            .collect();
        unread.push(json!({"load_name": library.load_name().map(djls_project::LibraryName::as_str), "module": library.module_name_str(), "source_failed": facts.source_failed(), "reasons": reasons}));
    }
    unread.sort_by_key(Value::to_string);
    unread.dedup();
    Ok(json!({
        "settings_module": module, "settings_source": source,
        "settings_file": testing::settings_module_file(db, project).map(|file| location(db, djls_source::Origin::new(file, djls_source::Span::new(0, 0)), &recipe.checkout_root, &recipe.environment_root)),
        "settings": {"installed_apps": case_counts(&settings, "installed_apps")?, "templates": case_counts(&settings, "templates")?},
        "uncertainty": uncertainty, "unread_libraries": unread, "unresolved_apps": unresolved,
    }))
}

#[expect(
    clippy::too_many_lines,
    reason = "keep editor discovery and the complete unsaved-buffer trial together"
)]
fn probe(
    corpus: &Corpus,
    name: &str,
    scan: bool,
) -> Result<(Value, Option<Value>), Box<dyn std::error::Error + Send + Sync>> {
    let recipe = corpus.environment_project(name)?;
    let mut options =
        json!({"venv_path": recipe.environment_root, "pythonpath": recipe.source_roots});
    if let Some(module) = &recipe.django_settings_module {
        options["django_settings_module"] = json!(module);
    }
    let params = ls_types::InitializeParams {
        initialization_options: Some(options.clone()),
        ..Default::default()
    };
    let mut host = AnalysisHost::new(Workspace::new(), Some(&recipe.project_root), &params);
    host.db_mut()
        .apply_project_settings(serde_json::from_value(options)?);
    djls_project::run_django_discovery(host.db_mut())?
        .ok_or_else(|| io::Error::other("discovery produced no project"))?;
    let db = host.db();
    let project = db
        .project()
        .ok_or_else(|| io::Error::other("missing project"))?;
    let mut snapshot = project_observations(db, project, &recipe)?;
    snapshot["app"] = json!(name);
    let directories = djls_project::template_directories(db, project);
    let directory = directories
        .known_roots()
        .find(|root| root.starts_with(&recipe.checkout_root))
        .map(Utf8Path::to_path_buf)
        .or_else(|| {
            corpus
                .templates_in(&recipe.checkout_root)
                .first()
                .and_then(|path| {
                    path.ancestors()
                        .find(|path| path.file_name() == Some("templates"))
                })
                .map(Utf8Path::to_path_buf)
        });
    let directory_source = if directory.is_none() {
        "none"
    } else if directory
        .as_ref()
        .is_some_and(|path| directories.known_roots().any(|root| root == path))
    {
        "discovery"
    } else {
        "corpus_fallback"
    };
    snapshot["probe_directory"] = json!(
        directory
            .as_ref()
            .map(|path| path
                .strip_prefix(&recipe.checkout_root)
                .map(ToString::to_string))
            .transpose()?
    );
    snapshot["probe_directory_source"] = json!(directory_source);
    let mut diagnostics = Vec::new();
    if let Some(directory) = directory {
        let path = directory.join("__djls_coverage_probe__.html");
        if path.exists() {
            return Err(
                io::Error::other(format!("probe would shadow an existing file: {path}")).into(),
            );
        }
        let document = ls_types::TextDocumentItem {
            uri: ls_types::Uri::from_file_path(path.as_std_path())
                .ok_or_else(|| io::Error::other("invalid probe URI"))?,
            language_id: "django-html".into(),
            version: 1,
            text: PROBE.into(),
        };
        host.open_document(&path, &document, None);
        let file = host
            .analysis()
            .template(&path)
            .ok_or_else(|| io::Error::other("overlay probe is not a template"))?;
        diagnostics = djls_ide::collect_diagnostics(host.db(), file, PositionEncoding::Utf8)
            .ok_or_else(|| io::Error::other("probe diagnostics unavailable"))?;
        if path.exists() {
            return Err(io::Error::other("probe must remain unsaved").into());
        }
    }
    let fired: Vec<_> = CHECKS
        .iter()
        .filter(|(code, line)| {
            diagnostics.iter().any(|diagnostic| {
                diagnostic_code(diagnostic) == Some(*code) && diagnostic.range.start.line == *line
            })
        })
        .map(|(code, _)| *code)
        .collect();
    snapshot["probe_codes"] = json!(
        CHECKS
            .iter()
            .map(|(code, _)| (*code, fired.contains(code)))
            .collect::<BTreeMap<_, _>>()
    );
    snapshot["fired_codes"] = json!(fired);
    snapshot["coverage"] = json!(format!("{}/{}", fired.len(), CHECKS.len()));
    snapshot["diagnostics"] = json!(diagnostics.iter().map(|diagnostic| json!({"code": diagnostic_code(diagnostic), "line": diagnostic.range.start.line + 1, "message": diagnostic.message})).collect::<Vec<_>>());
    let scan = if scan {
        Some(scan_templates(corpus, &host, &recipe.checkout_root)?)
    } else {
        None
    };
    Ok((snapshot, scan))
}

fn scan_templates(
    corpus: &Corpus,
    host: &AnalysisHost,
    checkout: &Utf8Path,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut counts = BTreeMap::<String, usize>::new();
    let mut records = Vec::new();
    let mut excluded = Vec::new();
    let mut templates = 0;
    for path in corpus.templates_in(checkout) {
        if !djls_source::FileKind::is_template(&path) {
            excluded.push(path.strip_prefix(checkout)?.to_string());
            continue;
        }
        let file = djls_source::path_to_file(host.db(), &path)?;
        let diagnostics = djls_ide::collect_diagnostics(host.db(), file, PositionEncoding::Utf8)
            .ok_or_else(|| io::Error::other(format!("scan diagnostics unavailable for {path}")))?;
        templates += 1;
        for diagnostic in diagnostics {
            let code = diagnostic_code(&diagnostic)
                .ok_or_else(|| io::Error::other("diagnostic without a string code"))?;
            *counts.entry(code.to_string()).or_default() += 1;
            records.push(json!({"path": path.strip_prefix(checkout)?, "code": code, "range": diagnostic.range, "message": diagnostic.message}));
        }
    }
    records.sort_by_key(Value::to_string);
    Ok(
        json!({"templates": templates, "excluded_non_targets": excluded, "code_counts": counts, "diagnostics": records}),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Arguments::from_args();
    let corpus = Arc::new(Corpus::require()?);
    let scan = std::env::var("DJLS_COVERAGE_SCAN").as_deref() == Ok("1");
    let export = std::env::var_os("DJLS_COVERAGE_JSON");
    if scan && export.is_none() {
        return Err(
            io::Error::other("DJLS_COVERAGE_SCAN=1 requires DJLS_COVERAGE_JSON=<path>").into(),
        );
    }
    let results = Arc::new(Mutex::new(BTreeMap::new()));
    let mut deferred = BTreeMap::new();
    let mut trials = Vec::new();
    for &name in APPS {
        let reason = corpus.environment_deferral(name)?;
        if let Some(reason) = &reason {
            deferred.insert(name, reason.clone());
        }
        let label = reason.as_ref().map_or_else(
            || format!("corpus_environment::coverage::{name}"),
            |reason| format!("corpus_environment::coverage::{name} [deferred: {reason}]"),
        );
        let corpus = Arc::clone(&corpus);
        let results = Arc::clone(&results);
        trials.push(
            Trial::test(label, move || {
                let (snapshot, scan) = probe(&corpus, name, scan)
                    .map_err(|error| libtest_mimic::Failed::from(error.to_string()))?;
                let mut settings = insta::Settings::clone_current();
                settings.set_snapshot_path(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/snapshots/coverage"
                ));
                let _guard = settings.bind_to_scope();
                let mut record = snapshot.clone();
                record["coverage"] = json!({"fired": snapshot["fired_codes"].as_array().map_or(0, Vec::len), "total": CHECKS.len()});
                if let Some(scan) = scan {
                    record["real_template_scan"] = scan;
                }
                results
                    .lock()
                    .map_err(|error| libtest_mimic::Failed::from(error.to_string()))?
                    .insert(name, record);
                insta::assert_yaml_snapshot!(name, snapshot);
                Ok(())
            })
            .with_ignored_flag(reason.is_some()),
        );
    }
    let conclusion = libtest_mimic::run(&args, trials);
    if let Some(path) = export {
        let results = results
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let fired: usize = results
            .values()
            .filter_map(|value| value["fired_codes"].as_array())
            .map(Vec::len)
            .sum();
        let full = results
            .values()
            .filter(|value| {
                value["fired_codes"]
                    .as_array()
                    .is_some_and(|codes| codes.len() == CHECKS.len())
            })
            .count();
        let document = json!({"schema_version": 1, "probe": PROBE, "checks": CHECKS.iter().map(|(code, line)| json!({"code": code, "line": line + 1})).collect::<Vec<_>>(), "cohort": APPS, "deferred_apps": deferred, "complete": results.len() + deferred.len() == APPS.len() && conclusion.num_filtered_out == 0 && !conclusion.has_failed(), "scan_enabled": scan, "apps": results.values().collect::<Vec<_>>(), "summary": {"apps": results.len(), "apps_at_full_coverage": full, "fired": fired, "total": results.len() * CHECKS.len()}});
        let path = std::path::Path::new(&path);
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(file.as_file_mut(), &document)?;
        file.persist(path)?;
    }
    conclusion.exit()
}
