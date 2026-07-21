//! Benchmarks for the cold validation-and-render kernel used by `djls check`.
//!
//! Divan input setup creates the fixture-backed Project, synchronizes the selected Template
//! sources, then runs production intrinsic priming and Template indexing outside the timed region.
//! The measured kernel validates and renders the synchronized files serially. It intentionally
//! excludes CLI argument/config loading, Django Environment and Project Facts discovery,
//! filesystem Template discovery, Salsa input synchronization, intrinsic priming, Template
//! indexing, Rayon scheduling, sorting, and terminal I/O.
//!
//! The semantic benchmark suite separately measures both a cold Project and a primed
//! Project/cold-Template workload, so excluding setup here does not hide required Project work.
//!
//! Scales from the fixture batch to the full corpus to stress the kernel while
//! keeping that narrower boundary explicit.
//!
//! Corpus benchmarks require a synced corpus (`just corpus sync`) when
//! `DJLS_REQUIRE_BENCH_CORPUS` is set. Optional local runs skip an absent corpus.

use camino::Utf8PathBuf;
use divan::Bencher;
use divan::black_box;
use divan::counter::ItemsCount;
use djls::check_template;
use djls_bench::BenchmarkSetupError;
use djls_bench::CorpusLoadError;
use djls_bench::CorpusTemplates;
use djls_bench::Db;
use djls_bench::corpus_or_skip;
use djls_bench::django_corpus_templates;
use djls_bench::full_corpus_templates;
use djls_bench::realistic_db;
use djls_bench::require;
use djls_bench::template_fixtures;
use djls_conf::DiagnosticsConfig;
use djls_ide::prepare_project_template_analysis;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::FileError;
use djls_source::FileReadError;

fn main() {
    divan::main();
}

#[derive(Debug, thiserror::Error)]
enum CheckSetupError {
    #[error(transparent)]
    Database(#[from] BenchmarkSetupError),
    #[error("failed to register check fixture {path}: {source}")]
    Register {
        path: Utf8PathBuf,
        #[source]
        source: FileError,
    },
    #[error("check benchmark database has no Project to prime and index")]
    MissingProject,
    #[error("failed to preflight check benchmark template {path}: {source}")]
    Check {
        path: Utf8PathBuf,
        #[source]
        source: FileReadError,
    },
}

struct CheckInput {
    db: Db,
    files: Vec<File>,
    config: DiagnosticsConfig,
    renderer: DiagnosticRenderer,
}

fn build_check_input(fixtures: &[(&Utf8PathBuf, &String)]) -> Result<CheckInput, CheckSetupError> {
    let mut db = realistic_db()?;
    let mut files = Vec::with_capacity(fixtures.len());
    for &(path, source) in fixtures {
        let file = db
            .file_with_contents(path.clone(), source)
            .map_err(|source| CheckSetupError::Register {
                path: path.clone(),
                source,
            })?;
        files.push(file);
    }
    prepare_project_template_analysis(&db).ok_or(CheckSetupError::MissingProject)?;
    Ok(CheckInput {
        db,
        files,
        config: DiagnosticsConfig::default(),
        renderer: DiagnosticRenderer::plain(),
    })
}

fn check_input<'a>(
    fixtures: impl IntoIterator<Item = (&'a Utf8PathBuf, &'a String)>,
) -> Result<CheckInput, CheckSetupError> {
    let fixtures: Vec<_> = fixtures.into_iter().collect();
    let preflight = build_check_input(&fixtures)?;
    for &file in &preflight.files {
        check_template(&preflight.db, file).map_err(|source| CheckSetupError::Check {
            path: file.path(&preflight.db).clone(),
            source,
        })?;
    }

    // Keep preflight query results out of the database used for timings.
    build_check_input(&fixtures)
}

fn run_check_kernel(input: &CheckInput) -> usize {
    let mut total_errors = 0;
    for &file in &input.files {
        let result = require(
            format_args!("check benchmark template {}", file.path(&input.db)),
            check_template(&input.db, file),
        );
        if result.has_diagnostics() {
            total_errors += result.render(&input.config, &input.renderer).len();
        }
    }
    total_errors
}

// Batch: all fixture templates through one fresh database.
// Per-library semantic products are shared while each Template builds its own
// sparse occurrence projection, matching real `djls check` behaviour.

#[divan::bench]
fn fixtures(bencher: Bencher) {
    let fixtures = require("load check benchmark fixtures", template_fixtures());
    bencher
        .with_inputs(move || {
            require(
                "prepare fixture check benchmark input",
                check_input(
                    fixtures
                        .iter()
                        .map(|fixture| (&fixture.path, &fixture.source)),
                ),
            )
        })
        .bench_local_refs(|input| black_box(run_check_kernel(input)));
}

// Corpus-scale: real Django templates and the full corpus.

fn bench_corpus_check(
    bencher: Bencher,
    corpus: Result<Option<&'static CorpusTemplates>, CorpusLoadError>,
    selection: &'static str,
) {
    let Some(corpus) = corpus_or_skip(format_args!("{selection} check benchmark"), corpus) else {
        return;
    };

    let file_count = corpus.files.len();

    bencher
        .counter(ItemsCount::new(file_count))
        .with_inputs(move || {
            require(
                format_args!("prepare {selection} corpus check benchmark input"),
                check_input(corpus.files.iter().map(|(path, source)| (path, source))),
            )
        })
        .bench_local_refs(|input| black_box(run_check_kernel(input)));
}

// Django's own templates (~123 files). Fresh db each iteration.

#[divan::bench]
fn corpus_django(bencher: Bencher) {
    bench_corpus_check(bencher, django_corpus_templates(), "Django");
}

// Full corpus (~6 000 templates from 36 packages). Fresh db each iteration.

#[divan::bench]
fn corpus_all(bencher: Bencher) {
    bench_corpus_check(bencher, full_corpus_templates(), "full");
}
