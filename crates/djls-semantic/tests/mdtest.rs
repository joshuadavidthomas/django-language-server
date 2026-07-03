use std::path::Path;

#[test]
fn mdtest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mdtest");
    djls_testing::run_suite(&root.join("diagnostics"));
    djls_testing::run_suite(&root.join("tags"));
}
