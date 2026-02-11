use divan::Bencher;
use djls_bench::python_fixtures;
use djls_bench::PythonFixture;

fn main() {
    divan::main();
}

#[divan::bench(args = python_fixtures())]
fn extract_rules(fixture: &PythonFixture) {
    divan::black_box(djls_python::extract_rules(&fixture.source, "bench.module"));
}

#[divan::bench]
fn extract_all_modules(bencher: Bencher) {
    let fixtures = python_fixtures();
    bencher.bench_local(move || {
        for fixture in fixtures {
            divan::black_box(djls_python::extract_rules(&fixture.source, "bench.module"));
        }
    });
}

#[divan::bench(args = python_fixtures())]
fn merge_extraction_into_specs(bencher: Bencher, fixture: &PythonFixture) {
    let mut result = djls_python::extract_rules(&fixture.source, "bench.module");
    result.rekey_module("bench.module");

    bencher.bench_local(move || {
        let mut specs = djls_semantic::TagSpecs::default();
        specs.merge_extraction_results(&result);
        divan::black_box(specs);
    });
}
