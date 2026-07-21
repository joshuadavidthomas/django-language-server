use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::Db;
use djls_bench::Fixture;
use djls_bench::REPEATED_INNER_ITERS;
use djls_bench::python_fixtures;
use djls_bench::require;
use djls_project::InvalidModuleName;
use djls_project::PythonModuleName;
use djls_source::File;
use djls_source::FileError;
use djls_testing::extract_bundle;

struct ExtractionFile {
    file: File,
    module: PythonModuleName,
}

struct ExtractionInput {
    db: Db,
    files: Vec<ExtractionFile>,
}

#[derive(Debug, thiserror::Error)]
enum ExtractionSetupError {
    #[error("invalid extraction benchmark module name: {0}")]
    Module(#[from] InvalidModuleName),
    #[error("failed to register extraction fixture {path}: {source}")]
    Register {
        path: Utf8PathBuf,
        #[source]
        source: FileError,
    },
}

fn main() {
    divan::main();
}

fn extraction_input(fixtures: &[Fixture]) -> Result<ExtractionInput, ExtractionSetupError> {
    let mut db = Db::new();
    let module = PythonModuleName::parse("bench.module")?;
    let mut files = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        let file = db
            .file_with_contents(fixture.path.clone(), &fixture.source)
            .map_err(|source| ExtractionSetupError::Register {
                path: fixture.path.clone(),
                source,
            })?;
        files.push(ExtractionFile {
            file,
            module: module.clone(),
        });
    }

    Ok(ExtractionInput { db, files })
}

#[divan::bench]
fn tags(bencher: Bencher) {
    let fixtures = require("load Python extraction fixtures", python_fixtures());
    bencher
        .with_inputs(|| require("prepare tag extraction input", extraction_input(fixtures)))
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
    let fixtures = require("load Python extraction fixtures", python_fixtures());
    let input = require("prepare tag merge input", extraction_input(fixtures));
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
