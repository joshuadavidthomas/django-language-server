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
use djls_bench::realistic_db;
use djls_bench::template_fixtures;
use djls_bench::Db;
use djls_db::FileCheckResult;
use djls_source::DiagnosticRenderer;

fn main() {
    divan::main();
}

/// Run `check_file` and capture the source for rendering (mirrors CLI pattern).
fn run_check(db: &Db, file: djls_source::File) -> FileCheckResult {
    let source = file.source(db);
    let path = file.path(db).clone();
    let check = djls_db::check_file(db, file);

    FileCheckResult {
        source,
        path,
        check,
    }
}

// Batch: all fixture templates through one fresh database.
// The first file pays for TagIndex construction; subsequent files
// reuse it — matching real `djls check` behaviour within a single run.

#[divan::bench]
fn check_batch_fixtures(bencher: Bencher) {
    let fixtures = template_fixtures();

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
    files: Vec<(Utf8PathBuf, String)>,
}

fn load_corpus_inner(
    get_paths: impl FnOnce(&djls_corpus::Corpus) -> Option<Vec<Utf8PathBuf>>,
) -> Option<CorpusTemplates> {
    if !djls_corpus::Corpus::is_available() {
        return None;
    }

    let corpus = djls_corpus::Corpus::require();
    let mut template_paths = get_paths(&corpus)?;
    template_paths.sort();

    let files: Vec<(Utf8PathBuf, String)> = template_paths
        .into_iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(path.as_std_path()).ok()?;
            Some((path, source))
        })
        .collect();

    if files.is_empty() {
        return None;
    }

    Some(CorpusTemplates { files })
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

fn bench_corpus_check(bencher: Bencher, corpus: Option<&'static CorpusTemplates>) {
    let Some(corpus) = corpus else {
        assert!(
            std::env::var_os("CI").is_none(),
            "corpus not synced; run `just corpus sync` before benchmarks",
        );
        eprintln!("corpus not synced, skipping");
        return;
    };

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
fn check_corpus_django(bencher: Bencher) {
    bench_corpus_check(bencher, load_corpus_templates());
}

// Full corpus (~6 000 templates from 36 packages). Fresh db each iteration.

#[divan::bench]
fn check_corpus_all(bencher: Bencher) {
    bench_corpus_check(bencher, load_full_corpus_templates());
}
