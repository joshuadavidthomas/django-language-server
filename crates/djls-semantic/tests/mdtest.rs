use std::path::Path;

#[test]
fn mdtest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mdtest");
    let mut actual = claimed_mdtest_suites(&root).expect("mdtest suites should be readable");
    actual.sort();

    let mut expected = vec![
        "diagnostics".to_string(),
        "inheritance".to_string(),
        "tags".to_string(),
        "unreadable-library".to_string(),
    ];
    expected.sort();

    assert_eq!(
        actual, expected,
        "resources/mdtest contains unregistered suites; register new suites in semantic mdtest tests"
    );
    djls_testing::run_suite(&root.join("diagnostics")).expect("diagnostic mdtest suite should run");
    djls_testing::run_suite(&root.join("tags")).expect("tag mdtest suite should run");
    djls_testing::run_suite_with(&root.join("unreadable-library"), render_unreadable_library)
        .expect("unreadable-library mdtest suite should run");
}

fn render_unreadable_library(scenario: &djls_testing::Scenario) -> anyhow::Result<String> {
    let db = djls_testing::unreadable_validation_db()?;
    djls_testing::render_validation_scenario(&db, scenario)
}

fn claimed_mdtest_suites(root: &Path) -> std::io::Result<Vec<String>> {
    let mut suites = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.path().is_dir() {
            suites.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(suites)
}
