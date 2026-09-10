use std::collections::BTreeSet;
use std::fs;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_semantic::ValidationErrorAccumulator;
use djls_semantic::validate_template_file;
use djls_source::path_to_file;
use djls_testing::CompilationGolden;
use djls_testing::DjangoCompilation;
use djls_testing::django_facts_project;

/// Django compiles these templates; DJLS reports a diagnostic. Every entry is a bug.
const FALSE_POSITIVES: &[&str] = &[];

/// Django rejects these templates; DJLS reports nothing. Every entry is a bug.
const MISSED_DIAGNOSTICS: &[&str] = &[
    // class constructors are not resolved as parser functions for rule extraction
    "class_missing",
    // flat constraint intersection drops the count-or-keyword alternatives of the raising and guard
    "conjunction_guard_invalid",
    // helper raises are not propagated, so caught-exception fallback forgets the split state after the first pop
    "exception_between_pops_missing",
    // option extraction records names but not the assignments required after with
    "include_missing_assignment",
    // the static choice list is mutated after assignment, so its value becomes Unknown
    "mutated_choice_invalid_value",
    // unpropagated helper raises admit the later split reassignment while caught-exception fallback widens bits to unknown
    "restored_exception_state_missing",
    // registration is curried through functools.partial; there is no standard-library search root and no model of partial, so the library is open
    "stdlib_partial_context_invalid",
    // callable is wrapped by a functools.wraps decorator; the wrapper is unresolvable and the library is open
    "stdlib_wrapped_invalid",
    // implicit-exception fallback enters finally with unknown bits and its return makes that path accepting
    "unhandled_exception_finally_missing",
];

#[test]
fn template_files_match_golden() {
    let (_, _, project_root, _) = django_facts_project("tests/compilation", "settings")
        .expect("Django compilation project should build");
    let template_root = project_root.join("templates");
    let golden: CompilationGolden = serde_json::from_str(include_str!(
        "../../../tests/fixtures/django-facts/compilation-5.2.json"
    ))
    .expect("compilation golden should parse");

    let mut disk_names = BTreeSet::new();
    for entry in
        fs::read_dir(&template_root).expect("compilation template directory should be readable")
    {
        let path = Utf8PathBuf::from_path_buf(
            entry
                .expect("compilation directory entry should be readable")
                .path(),
        )
        .expect("compilation template path must be UTF-8");
        disk_names.insert(
            path.strip_prefix(&template_root)
                .expect("template should be under compilation template root")
                .to_string(),
        );
    }
    let golden_names: BTreeSet<_> = golden.django_compilation.keys().cloned().collect();

    assert_eq!(
        disk_names, golden_names,
        "compilation template files changed; run nox -s fixtures"
    );
}

#[test]
fn templates_compile_like_django() {
    let (db, _, project_root, _) = django_facts_project("tests/compilation", "settings")
        .expect("Django compilation project should build");
    let golden: CompilationGolden = serde_json::from_str(include_str!(
        "../../../tests/fixtures/django-facts/compilation-5.2.json"
    ))
    .expect("compilation golden should parse");
    let template_root = project_root.join("templates");
    let mut false_positives = BTreeSet::new();
    let mut missed_diagnostics = BTreeSet::new();

    for (name, compilation) in &golden.django_compilation {
        let case = Utf8Path::new(name)
            .file_stem()
            .expect("template needs a case id");
        let file = path_to_file(&db, &template_root.join(name))
            .expect("compilation template should exist");
        file.try_source(&db)
            .expect("compilation template should be readable");
        validate_template_file(&db, file);
        let errors = validate_template_file::accumulated::<ValidationErrorAccumulator>(&db, file);
        // S124 says DJLS could not read part of a library. It is a hint, not a
        // rejection, so it does not count against Django compilation.
        let rejects = errors.iter().any(|error| {
            !matches!(
                &error.0,
                djls_semantic::ValidationError::UnreadableLibrary { .. }
            )
        });

        match (compilation, rejects) {
            (DjangoCompilation::Compiled, false) | (DjangoCompilation::Failed { .. }, true) => {}
            (DjangoCompilation::Compiled, true) => {
                false_positives.insert(case);
            }
            (DjangoCompilation::Failed { .. }, false) => {
                missed_diagnostics.insert(case);
            }
        }
    }

    assert_eq!(
        false_positives,
        FALSE_POSITIVES.iter().copied().collect(),
        "false positives (Django compiles, DJLS reports a diagnostic) changed; fix the list"
    );
    assert_eq!(
        missed_diagnostics,
        MISSED_DIAGNOSTICS.iter().copied().collect(),
        "missed diagnostics (Django rejects, DJLS reports nothing) changed; fix the list"
    );
}
