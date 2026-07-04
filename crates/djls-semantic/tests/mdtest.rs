use std::path::Path;

#[test]
fn mdtest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mdtest");
    assert_claimed_mdtest_suites(&root);
    djls_testing::run_suite(&root.join("diagnostics"));
    djls_testing::run_suite(&root.join("tags"));
}

fn assert_claimed_mdtest_suites(root: &Path) {
    let mut actual = std::fs::read_dir(root)
        .expect("failed to read mdtest root")
        .map(|entry| entry.expect("failed to read mdtest root entry").path())
        .filter(|path| path.is_dir())
        .map(|path| {
            path.file_name()
                .expect("mdtest suite path should have a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    actual.sort();

    let mut expected = vec![
        "diagnostics".to_string(),
        "inheritance".to_string(),
        "tags".to_string(),
    ];
    expected.sort();

    assert_eq!(
        actual, expected,
        "resources/mdtest contains unregistered suites; register new suites in semantic mdtest tests"
    );
}
