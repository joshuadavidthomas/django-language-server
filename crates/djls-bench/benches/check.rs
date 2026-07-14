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
    djls_ide::prepare_project_template_analysis(&db)
        .expect("check benchmark database should install a Project");

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
// Per-library semantic products are shared while each Template builds its own
// sparse occurrence projection, matching real `djls check` behaviour.

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
            // Canonical builtins intentionally correct the old synthetic unknown/unloaded output;
            // the six files and 30,105 input bytes are unchanged.
            diagnostics: DiagnosticDigest::from_counts(0, 20, [("S111", 20)]),
            rendered_count: 20,
        },
    );

    bencher
        .with_inputs(move || {
            let mut db = realistic_db();
            let files: Vec<_> = fixtures
                .iter()
                .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
                .collect();
            djls_ide::prepare_project_template_analysis(&db)
                .expect("check benchmark database should install a Project");
            (
                db,
                files,
                djls_conf::DiagnosticsConfig::default(),
                DiagnosticRenderer::plain(),
            )
        })
        .bench_local_refs(|(db, files, config, fmt)| {
            let mut total_errors = 0;
            for &file in files.iter() {
                let result = run_check(db, file);
                total_errors += result.render(config, fmt).len();
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
        .with_inputs(move || {
            let mut db = realistic_db();
            let files: Vec<_> = corpus
                .files
                .iter()
                .map(|(path, source)| db.file_with_contents(path.clone(), source))
                .collect();
            djls_ide::prepare_project_template_analysis(&db)
                .expect("check benchmark database should install a Project");
            (
                db,
                files,
                djls_conf::DiagnosticsConfig::default(),
                DiagnosticRenderer::plain(),
            )
        })
        .bench_local_refs(|(db, files, config, fmt)| {
            let mut total_errors = 0;
            for &file in files.iter() {
                let result = run_check(db, file);
                if result.has_diagnostics() {
                    total_errors += result.render(config, fmt).len();
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
            // Canonical builtins remove synthetic unloaded diagnostics while preserving all 123
            // Django templates and 134,038 input bytes.
            diagnostics: DiagnosticDigest::from_counts(
                0,
                104,
                [("S108", 19), ("S111", 67), ("S120", 18)],
            ),
            rendered_count: 104,
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
            // Canonical builtin roles remove synthetic unloaded output and classify unresolved
            // corpus libraries directly; corpus file and byte cardinality remain unchanged.
            diagnostics: DiagnosticDigest::from_counts(
                0,
                19_327,
                [
                    ("S101", 1),
                    ("S108", 10_363),
                    ("S111", 4_033),
                    ("S120", 4_930),
                ],
            ),
            rendered_count: 19_327,
        },
    );
}
