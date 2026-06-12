use std::path::Path;

#[test]
fn mdtest() {
    djls_testing::run_suite(&Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mdtest"));
}
