use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

static DATABASES_CREATED: AtomicUsize = AtomicUsize::new(0);

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
    DATABASES_CREATED.store(0, Ordering::Relaxed);
    djls_testing::run_validation_suite_with(&root.join("diagnostics"), counted_standard_database)
        .expect("diagnostic mdtest suite should run");
    djls_testing::run_validation_suite_with(&root.join("tags"), counted_standard_database)
        .expect("tag mdtest suite should run");
    djls_testing::run_validation_suite_with(
        &root.join("unreadable-library"),
        counted_unreadable_database,
    )
    .expect("unreadable-library mdtest suite should run");
    assert_eq!(
        DATABASES_CREATED.load(Ordering::Relaxed),
        3,
        "each mdtest suite should create one validation database"
    );
}

fn counted_standard_database() -> anyhow::Result<djls_testing::OsTestDatabase> {
    DATABASES_CREATED.fetch_add(1, Ordering::Relaxed);
    djls_testing::standard_validation_db()
}

fn counted_unreadable_database() -> anyhow::Result<djls_testing::OsTestDatabase> {
    DATABASES_CREATED.fetch_add(1, Ordering::Relaxed);
    djls_testing::unreadable_validation_db()
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
