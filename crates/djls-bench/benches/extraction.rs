use divan::Bencher;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::python_fixtures;

fn main() {
    divan::main();
}

#[divan::bench]
fn tags(bencher: Bencher) {
    let fixtures = python_fixtures();
    bencher.bench_local(move || {
        for fixture in fixtures {
            divan::black_box(djls_semantic::extract_rules(
                &fixture.source,
                "bench.module",
            ));
        }
    });
}

#[divan::bench]
fn merge_tags(bencher: Bencher) {
    let fixtures = python_fixtures();
    let results: Vec<_> = fixtures
        .iter()
        .map(|fixture| djls_semantic::extract_rules(&fixture.source, "bench.module"))
        .collect();

    bencher.bench_local(move || {
        let mut merged_rules = 0;
        for _ in 0..REPEATED_INNER_ITERS {
            let mut specs = djls_semantic::TagSpecs::default();
            for result in &results {
                specs.merge_extraction_results(result);
            }
            merged_rules += specs.len();
        }
        divan::black_box(merged_rules);
    });
}
