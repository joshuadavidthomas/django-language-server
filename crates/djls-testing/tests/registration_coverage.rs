use std::fmt::Write as _;

use djls_project::PythonModuleName;
use djls_project::TemplateLibraryId;
use djls_project::template_library_definition_facts;
use djls_testing::Corpus;
use djls_testing::TestDatabase;
use djls_testing::census::NameStatus;
use djls_testing::census::census_source;
use djls_testing::module_name_from_file;

#[test]
fn corpus_registration_census() {
    let corpus = Corpus::require().expect("synced corpus should be available for corpus tests");
    let targets = corpus
        .extraction_target_members()
        .expect("corpus extraction targets should have relative paths");
    assert!(!targets.is_empty(), "No extraction targets in corpus.");
    let db = TestDatabase::new();
    let mut report = String::from(
        "Counts: candidates / resolved / unresolved\nResolved means a known syntax name matches an extracted definition.\nUnresolved means unmatched, including unknown syntax names; it does not prove a missing registration.\n\n",
    );

    for target in targets {
        let source = std::fs::read_to_string(&target.path)
            .expect("corpus registration source should be readable");
        let candidates = census_source(&source).expect("corpus registration source should parse");
        db.add_file(target.path.as_str(), &source)
            .expect("corpus source should be added to the test database");
        let file = db.file(&target.path).expect("corpus source should exist");
        let module = PythonModuleName::parse(&module_name_from_file(&target.path))
            .expect("corpus module name should be valid");
        let library = TemplateLibraryId::new(&db, Some(file), module);
        let definitions = template_library_definition_facts(&db, library);
        assert!(
            !definitions.source_failed(),
            "definition extraction failed for {}/{}",
            target.member,
            target.relative_path,
        );
        let unresolved: Vec<_> = candidates
            .iter()
            .filter(|candidate| {
                candidate.name_status == NameStatus::Unresolved
                    || definitions
                        .symbol(candidate.kind, &candidate.name)
                        .is_none()
            })
            .collect();
        writeln!(
            report,
            "{}/{}: {} / {} / {}",
            target.member,
            target.relative_path,
            candidates.len(),
            candidates.len() - unresolved.len(),
            unresolved.len(),
        )
        .expect("writing to a string should succeed");
        for candidate in unresolved {
            writeln!(
                report,
                "  {:?} {:?} {}..{} {:?}: {}",
                candidate.helper,
                candidate.form,
                candidate.span.start,
                candidate.span.end,
                candidate.name_status,
                candidate.name,
            )
            .expect("writing to a string should succeed");
        }
    }

    insta::assert_snapshot!(report);
}
