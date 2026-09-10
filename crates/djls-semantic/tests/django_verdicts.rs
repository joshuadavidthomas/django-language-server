use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::LibraryName;
use djls_project::ScopedTemplateLibraries;
use djls_project::TemplateSymbolKind;
use djls_project::template_library_catalog;
use djls_semantic::ValidationErrorAccumulator;
use djls_semantic::validate_template_file;
use djls_source::path_to_file;
use djls_testing::TemplateVerdict;
use djls_testing::django_facts_project;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disagreement {
    /// Django accepts the template; DJLS reports at least one diagnostic.
    FalsePositive,
    /// Django rejects the template; DJLS reports nothing.
    MissedDiagnostic,
}

/// Every row is a bug. A fix deletes its row in the same change.
const KNOWN_DISAGREEMENTS: &[(&str, Disagreement, &str)] = &[
    (
        "direct_context_valid",
        Disagreement::FalsePositive,
        "direct registration loses takes_context, so context counts as a template argument",
    ),
    (
        "lorem_no_arguments",
        Disagreement::FalsePositive,
        "conditional argument pops are extracted as an unconditional count",
    ),
    (
        "lorem_words",
        Disagreement::FalsePositive,
        "conditional argument pops are extracted as an unconditional count",
    ),
    (
        "authored_keyword_only_missing",
        Disagreement::MissedDiagnostic,
        "parameter syntax is extracted but validation does not bind required keyword-only arguments",
    ),
    (
        "authored_positional_after_keyword",
        Disagreement::MissedDiagnostic,
        "parameter syntax is extracted but validation does not check argument ordering",
    ),
    (
        "authored_repeated_keyword",
        Disagreement::MissedDiagnostic,
        "parameter syntax is extracted but validation does not reject repeated keywords",
    ),
    (
        "class_missing",
        Disagreement::MissedDiagnostic,
        "class constructors are not resolved as parser functions for rule extraction",
    ),
    (
        "curried_context_missing",
        Disagreement::MissedDiagnostic,
        "curried registration leaves the library inventory open without an argument rule",
    ),
    (
        "firstof_missing",
        Disagreement::MissedDiagnostic,
        "split-sequence truthiness guards do not produce argument-count constraints",
    ),
    (
        "for_wrong_separator",
        Disagreement::MissedDiagnostic,
        "the conditional expression choosing the in-keyword index evaluates to Unknown",
    ),
    (
        "include_missing_assignment",
        Disagreement::MissedDiagnostic,
        "option extraction records names but not the assignments required after with",
    ),
    (
        "simple_block_missing",
        Disagreement::MissedDiagnostic,
        "simple_block_tag bodies use manual-parser analysis instead of signature extraction",
    ),
    (
        "stdlib_partial_context_invalid",
        Disagreement::MissedDiagnostic,
        "registration is curried through functools.partial; there is no standard-library search root and no model of partial, so the library is open",
    ),
    (
        "stdlib_wrapped_invalid",
        Disagreement::MissedDiagnostic,
        "callable is wrapped by a functools.wraps decorator; the wrapper is unresolvable and the library is open",
    ),
    (
        "templatetag_invalid_choice",
        Disagreement::MissedDiagnostic,
        "class-dictionary membership is not extracted as a set of allowed choices",
    ),
    (
        "with_invalid_assignment",
        Disagreement::MissedDiagnostic,
        "token_kwargs invalidates the remaining bits without modeling assignment parsing",
    ),
];

/// Registrations Django reports that DJLS does not discover. Every row is a
/// bug. A fix deletes its row in the same change.
const KNOWN_MISSING_REGISTRATIONS: &[(&str, TemplateSymbolKind, &str, &str)] = &[
    (
        "verdict_tags",
        TemplateSymbolKind::Tag,
        "curried_context",
        "curried simple_tag registration is not discovered",
    ),
    (
        "verdict_tags",
        TemplateSymbolKind::Tag,
        "stdlib_partial_context",
        "functools.partial has no standard-library search root or static model",
    ),
    (
        "verdict_tags",
        TemplateSymbolKind::Tag,
        "stdlib_wrapped",
        "functools.wraps leaves the wrapper unresolvable and the library open",
    ),
];

#[test]
#[allow(clippy::too_many_lines)]
fn verdicts_match_django() {
    let (db, project, project_root, _, golden) = django_facts_project(
        "tests/verdicts",
        "tests/fixtures/django-facts/verdicts-5.2.json",
        "settings",
    )
    .expect("Django facts project should build");
    let template_root = project_root.join("templates");
    let mut disk_names = BTreeSet::new();
    for entry in
        fs::read_dir(&template_root).expect("verdict template directory should be readable")
    {
        let path = Utf8PathBuf::from_path_buf(
            entry
                .expect("verdict directory entry should be readable")
                .path(),
        )
        .expect("verdict template path must be UTF-8");
        disk_names.insert(
            path.strip_prefix(&template_root)
                .expect("verdict should be under template root")
                .to_string(),
        );
    }
    let golden_names: BTreeSet<_> = golden.template_verdicts.keys().cloned().collect();
    assert_eq!(
        disk_names, golden_names,
        "verdict files changed; run nox -s fixtures"
    );

    let mut known: BTreeMap<_, _> = KNOWN_DISAGREEMENTS
        .iter()
        .map(|&(id, direction, reason)| (id, (direction, reason)))
        .collect();
    assert_eq!(
        known.len(),
        KNOWN_DISAGREEMENTS.len(),
        "duplicate register row"
    );
    let mut observations = Vec::new();
    let mut failures = Vec::new();
    for (name, verdict) in &golden.template_verdicts {
        let id = Utf8Path::new(name)
            .file_stem()
            .expect("template needs a case id");
        let file =
            path_to_file(&db, &template_root.join(name)).expect("verdict template should exist");
        file.try_source(&db)
            .expect("verdict template should be readable");
        validate_template_file(&db, file);
        let errors = validate_template_file::accumulated::<ValidationErrorAccumulator>(&db, file);
        let disagreement = match (verdict, errors.is_empty()) {
            (TemplateVerdict::Accepted, true) | (TemplateVerdict::Rejected { .. }, false) => None,
            (TemplateVerdict::Accepted, false) => Some(Disagreement::FalsePositive),
            (TemplateVerdict::Rejected { .. }, true) => Some(Disagreement::MissedDiagnostic),
        };
        let registered = known.remove(id);
        match (disagreement, registered) {
            (None, None) => {}
            (None, Some(_)) => failures.push(format!("{id}: stale row, delete it")),
            (Some(direction), Some((expected, reason))) if direction == expected => {
                observations.push((id, direction, Some(reason)));
            }
            (Some(direction), Some((expected, _))) => {
                observations.push((id, direction, None));
                failures.push(format!(
                    "{id}: gap changed from {expected:?} to {direction:?}"
                ));
            }
            (Some(direction), None) => {
                observations.push((id, direction, None));
                failures.push(format!("{id}: new {direction:?}"));
            }
        }
    }
    for id in known.keys() {
        failures.push(format!("{id}: register row has no template; delete it"));
    }

    for (direction, label) in [
        (
            Disagreement::FalsePositive,
            "false positives (DJLS rejects, Django accepts)",
        ),
        (
            Disagreement::MissedDiagnostic,
            "missed diagnostics (DJLS accepts, Django rejects)",
        ),
    ] {
        let rows: Vec<_> = observations
            .iter()
            .filter(|(_, observed, _)| *observed == direction)
            .collect();
        let known_count = rows
            .iter()
            .filter(|(_, _, reason)| reason.is_some())
            .count();
        println!(
            "{label}: {known_count} known, {} new",
            rows.len() - known_count
        );
        for (id, _, reason) in rows {
            println!("  {id:36} {}", reason.unwrap_or("NEW"));
        }
    }
    let catalog = template_library_catalog(&db, project);
    let scoped = ScopedTemplateLibraries::from_project_inventory(catalog);
    let library_name = LibraryName::parse("verdict_tags").expect("library name should parse");
    let library = scoped
        .loadable_library(&library_name)
        .found()
        .expect("verdict library should be discovered");
    assert_eq!(library.module_name_str(), "verdict_tags");
    let mut known_missing: BTreeMap<_, _> = KNOWN_MISSING_REGISTRATIONS
        .iter()
        .map(|&(module, kind, name, reason)| ((module, kind, name), reason))
        .collect();
    assert_eq!(
        known_missing.len(),
        KNOWN_MISSING_REGISTRATIONS.len(),
        "duplicate registration row"
    );
    let mut missing = Vec::new();
    for symbol in golden
        .template_library_catalog
        .symbols
        .iter()
        .filter(|symbol| symbol.library_module == "verdict_tags")
    {
        let key = (
            symbol.library_module.as_str(),
            symbol.kind,
            symbol.name.as_str(),
        );
        let present = library.symbol(symbol.kind, &symbol.name).is_some();
        let registered = known_missing.remove(&key);
        match (present, registered) {
            (true, None) => {}
            (true, Some(_)) => failures.push(format!(
                "{}: stale registration row, delete it",
                symbol.name
            )),
            (false, reason) => {
                missing.push((symbol.name.as_str(), reason));
                if reason.is_none() {
                    failures.push(format!("{}: new missing registration", symbol.name));
                }
            }
        }
    }
    for (module, kind, name) in known_missing.keys() {
        failures.push(format!("{module}.{name} ({kind:?}): registration row is absent from Django facts; check for a typo"));
    }
    let known_count = missing
        .iter()
        .filter(|(_, reason)| reason.is_some())
        .count();
    println!(
        "missing registrations: {known_count} known, {} new",
        missing.len() - known_count
    );
    for (name, reason) in missing {
        println!("  {name:36} {}", reason.unwrap_or("NEW"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
