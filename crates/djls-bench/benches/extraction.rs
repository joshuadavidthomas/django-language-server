use divan::Bencher;
use djls_bench::Db;
use djls_bench::Fixture;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::python_fixtures;
use djls_project::PythonModulePath;
use djls_source::File;
use djls_testing::extract_bundle;

struct ExtractionFile {
    file: File,
    module: PythonModulePath,
}

struct ExtractionInput {
    db: Db,
    files: Vec<ExtractionFile>,
}

fn main() {
    divan::main();
}

fn extraction_input(fixtures: &[Fixture]) -> ExtractionInput {
    let mut db = Db::new();
    let files = fixtures
        .iter()
        .map(|fixture| ExtractionFile {
            file: db.file_with_contents(fixture.path.clone(), &fixture.source),
            module: PythonModulePath::parse("bench.module").unwrap(),
        })
        .collect();

    ExtractionInput { db, files }
}

#[divan::bench]
fn tags(bencher: Bencher) {
    let fixtures = python_fixtures();
    bencher
        .with_inputs(|| extraction_input(fixtures))
        .bench_local_values(|input| {
            let mut extracted = 0;
            for extraction_file in input.files {
                let bundle =
                    extract_bundle(&input.db, extraction_file.file, extraction_file.module);
                extracted += bundle.tag_rules.len();
                extracted += bundle.filter_arities.len();
                extracted += bundle.block_specs.as_map().len();
                divan::black_box(bundle);
            }
            divan::black_box(extracted);
        });
}

#[divan::bench]
fn merge_tags(bencher: Bencher) {
    let input = extraction_input(python_fixtures());
    let bundles: Vec<_> = input
        .files
        .iter()
        .map(|extraction_file| {
            extract_bundle(
                &input.db,
                extraction_file.file,
                extraction_file.module.clone(),
            )
        })
        .collect();

    bencher.bench_local(move || {
        let mut merged_rules = 0;
        for _ in 0..REPEATED_INNER_ITERS {
            let mut specs = djls_semantic::TagSpecs::default();
            for bundle in &bundles {
                specs
                    .merge_block_specs(&bundle.block_specs)
                    .merge_tag_rules(&bundle.tag_rules);
            }
            merged_rules += specs.len();
        }
        divan::black_box(merged_rules);
    });
}
