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
const FALSE_POSITIVES: &[&str] = &[
    // conditional argument pops are extracted as an unconditional count
    "lorem_no_arguments",
    "lorem_words",
];

/// Django rejects these templates; DJLS reports nothing. Every entry is a bug.
const MISSED_DIAGNOSTICS: &[&str] = &[
    // class constructors are not resolved as parser functions for rule extraction
    "class_missing",
    // split-sequence truthiness guards do not produce argument-count constraints
    "firstof_missing",
    // the conditional expression choosing the in-keyword index evaluates to Unknown
    "for_wrong_separator",
    // option extraction records names but not the assignments required after with
    "include_missing_assignment",
    // registration is curried through functools.partial; there is no standard-library search root and no model of partial, so the library is open
    "stdlib_partial_context_invalid",
    // callable is wrapped by a functools.wraps decorator; the wrapper is unresolvable and the library is open
    "stdlib_wrapped_invalid",
    // class-dictionary membership is not extracted as a set of allowed choices
    "templatetag_invalid_choice",
    // token_kwargs invalidates the remaining bits without modeling assignment parsing
    "with_invalid_assignment",
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

        match (compilation, errors.is_empty()) {
            (DjangoCompilation::Compiled, true) | (DjangoCompilation::Failed { .. }, false) => {}
            (DjangoCompilation::Compiled, false) => {
                false_positives.insert(case);
            }
            (DjangoCompilation::Failed { .. }, true) => {
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
