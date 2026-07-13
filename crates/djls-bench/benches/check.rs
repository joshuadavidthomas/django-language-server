//! Benchmarks for the `djls check` pipeline.
//!
//! `djls check` is a one-shot CLI command: fresh database, process every
//! file, render diagnostics, exit. There is no warm Salsa cache to
//! benefit from — every benchmark here measures the cold path.
//!
//! Scales from a single file up to the full corpus (~6 000 real-world
//! templates) to stress-test the pipeline end to end.
//!
//! Corpus benchmarks require a synced corpus (`just corpus sync`) and
//! skip gracefully when unavailable.

use std::sync::OnceLock;

use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::Db;
use djls_bench::DiagnosticDigest;
use djls_bench::FileCheckResult;
use djls_bench::realistic_db;
use djls_bench::template_fixtures;
use djls_bench::template_path;
use djls_source::DiagnosticRenderer;

fn main() {
    divan::main();
}

/// Run `check_file` and capture the source for rendering (mirrors CLI pattern).
fn run_check(db: &Db, file: djls_source::File) -> FileCheckResult {
    let source = file
        .try_source(db)
        .expect("benchmark file should be readable");
    let path = file.path(db).clone();
    let check = djls_bench::check_file(db, file);

    FileCheckResult {
        source,
        path,
        check,
    }
}

#[derive(Debug)]
struct CheckWorkloadContract {
    discovered_file_count: usize,
    synchronized_file_count: usize,
    total_bytes: usize,
    diagnostics: DiagnosticDigest,
    rendered_count: usize,
}

fn assert_check_workload_contract(
    discovered_file_count: usize,
    files: &[(Utf8PathBuf, String)],
    expected: &CheckWorkloadContract,
) {
    assert_eq!(discovered_file_count, expected.discovered_file_count);
    assert_eq!(files.len(), expected.synchronized_file_count);
    assert_eq!(
        files.iter().map(|(_, source)| source.len()).sum::<usize>(),
        expected.total_bytes,
    );

    let mut db = realistic_db();
    let synchronized: Vec<_> = files
        .iter()
        .map(|(path, source)| db.file_with_contents(path.clone(), source))
        .collect();
    assert_eq!(synchronized.len(), expected.synchronized_file_count);

    let config = djls_conf::DiagnosticsConfig::default();
    let fmt = DiagnosticRenderer::plain();
    let mut diagnostics = DiagnosticDigest::default();
    let mut rendered_count = 0;
    for file in synchronized {
        let result = run_check(&db, file);
        diagnostics.merge(&result.diagnostic_digest());
        rendered_count += result.render(&config, &fmt).len();
    }

    assert_eq!(diagnostics, expected.diagnostics);
    assert_eq!(rendered_count, expected.rendered_count);
}

// Batch: all fixture templates through one fresh database.
// The first file pays for TagIndex construction; subsequent files
// reuse it — matching real `djls check` behaviour within a single run.

#[divan::bench]
fn fixtures(bencher: Bencher) {
    let fixtures = template_fixtures();
    let workload: Vec<_> = fixtures
        .iter()
        .map(|fixture| (fixture.path.clone(), fixture.source.clone()))
        .collect();
    assert_check_workload_contract(
        workload.len(),
        &workload,
        &CheckWorkloadContract {
            discovered_file_count: 6,
            synchronized_file_count: 6,
            total_bytes: 30_105,
            diagnostics: DiagnosticDigest::from_counts(
                0,
                87,
                [("S108", 22), ("S109", 45), ("S111", 20)],
            ),
            rendered_count: 87,
        },
    );

    bencher.bench_local(move || {
        let mut db = realistic_db();
        let config = djls_conf::DiagnosticsConfig::default();
        let fmt = DiagnosticRenderer::plain();

        let files: Vec<_> = fixtures
            .iter()
            .map(|f| db.file_with_contents(f.path.clone(), &f.source))
            .collect();

        let mut total_errors = 0;
        for &file in &files {
            let result = run_check(&db, file);
            total_errors += result.render(&config, &fmt).len();
        }
        divan::black_box(total_errors);
    });
}

// Corpus-scale: real Django templates and the full corpus.

struct CorpusTemplates {
    discovered_file_count: usize,
    files: Vec<(Utf8PathBuf, String)>,
}

fn load_corpus_inner(
    get_paths: impl FnOnce(&djls_testing::Corpus) -> Option<Vec<Utf8PathBuf>>,
) -> Option<CorpusTemplates> {
    if !djls_testing::Corpus::is_available() {
        return None;
    }

    let corpus = djls_testing::Corpus::require();
    let mut template_paths = get_paths(&corpus)?;
    template_paths.sort();

    let discovered_file_count = template_paths.len();
    let files: Vec<(Utf8PathBuf, String)> = template_paths
        .into_iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(path.as_std_path()).ok()?;
            let relative = path.strip_prefix(corpus.root()).ok()?;
            Some((template_path(relative), source))
        })
        .collect();

    if files.is_empty() {
        return None;
    }

    Some(CorpusTemplates {
        discovered_file_count,
        files,
    })
}

fn load_corpus_templates() -> Option<&'static CorpusTemplates> {
    static CORPUS: OnceLock<Option<CorpusTemplates>> = OnceLock::new();
    CORPUS
        .get_or_init(|| {
            load_corpus_inner(|corpus| {
                let django_dir = corpus.latest_package("django")?;
                Some(corpus.templates_in(&django_dir))
            })
        })
        .as_ref()
}

fn load_full_corpus_templates() -> Option<&'static CorpusTemplates> {
    static CORPUS: OnceLock<Option<CorpusTemplates>> = OnceLock::new();
    CORPUS
        .get_or_init(|| load_corpus_inner(|corpus| Some(corpus.templates_in(corpus.root()))))
        .as_ref()
}

fn bench_corpus_check(
    bencher: Bencher,
    corpus: Option<&'static CorpusTemplates>,
    expected: &CheckWorkloadContract,
) {
    let Some(corpus) = corpus else {
        assert!(
            std::env::var_os("CI").is_none(),
            "corpus not synced; run `just corpus sync` before benchmarks",
        );
        eprintln!("corpus not synced, skipping");
        return;
    };

    assert_check_workload_contract(corpus.discovered_file_count, &corpus.files, expected);
    let file_count = corpus.files.len();

    bencher
        .counter(divan::counter::ItemsCount::new(file_count))
        .bench_local(move || {
            let mut db = realistic_db();
            let config = djls_conf::DiagnosticsConfig::default();
            let fmt = DiagnosticRenderer::plain();

            let files: Vec<_> = corpus
                .files
                .iter()
                .map(|(path, source)| db.file_with_contents(path.clone(), source))
                .collect();

            let mut total_errors = 0;
            for &file in &files {
                let result = run_check(&db, file);
                if result.has_diagnostics() {
                    total_errors += result.render(&config, &fmt).len();
                }
            }
            divan::black_box(total_errors);
        });
}

// Django's own templates (~123 files). Fresh db each iteration.

#[divan::bench]
fn corpus_django(bencher: Bencher) {
    bench_corpus_check(
        bencher,
        load_corpus_templates(),
        &CheckWorkloadContract {
            discovered_file_count: 123,
            synchronized_file_count: 123,
            total_bytes: 134_038,
            diagnostics: DiagnosticDigest::from_counts(
                0,
                633,
                [("S108", 257), ("S109", 309), ("S111", 67)],
            ),
            rendered_count: 633,
        },
    );
}

// Full corpus (~6 000 templates from 36 packages). Fresh db each iteration.

#[divan::bench]
fn corpus_all(bencher: Bencher) {
    bench_corpus_check(
        bencher,
        load_full_corpus_templates(),
        &CheckWorkloadContract {
            discovered_file_count: 7_145,
            synchronized_file_count: 7_145,
            total_bytes: 11_783_514,
            diagnostics: DiagnosticDigest::from_counts(
                0,
                49_444,
                [
                    ("S101", 1),
                    ("S108", 23_402),
                    ("S109", 22_008),
                    ("S111", 4_033),
                ],
            ),
            rendered_count: 49_444,
        },
    );
}
