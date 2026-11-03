//! What djls costs on a real project with its real Python environment.
//!
//! Each benchmark opens a corpus checkout with the environment `corpus environment sync`
//! builds from the project's own requirements, through the server's `AnalysisHost` and the
//! reload stages in `djls-server`: settings, Python Environment discovery into site-packages,
//! Project Facts (settings evaluation with real imports, the Template Library catalog, tag
//! candidates), Template Library priming, and IDE cache warm-up.
//!
//! The projects need synchronized environments:
//! `cargo run -p djls-testing --bin corpus -- environment sync djangoproject.com netbox`.
//! Optional local runs skip a missing environment; `DJLS_REQUIRE_BENCH_CORPUS` makes it an
//! error.

use divan::Bencher;
use divan::black_box;
use divan::counter::ItemsCount;
use djls_bench::Keystroke;
use djls_bench::REAL_PROJECTS;
use djls_bench::RealProject;
use djls_bench::corpus_or_skip;

fn main() {
    divan::main();
}

fn load(name: &str) -> Option<RealProject> {
    corpus_or_skip(
        format_args!("{name} real project benchmark"),
        RealProject::load(name),
    )
}

/// From a fresh database: open the project, then publish diagnostics for every template.
/// Building the empty host is setup; every stage after it is timed.
#[divan::bench(args = REAL_PROJECTS, sample_count = 3, sample_size = 1)]
fn cold_start_check_templates(bencher: Bencher, name: &str) {
    let Some(project) = load(name) else {
        return;
    };
    bencher
        .counter(ItemsCount::new(project.templates.len()))
        .with_inputs(|| project.host())
        .bench_local_values(|mut host| {
            project.open(&mut host);
            black_box(project.check_templates(&host));
            // Dropping the database is not part of a cold start.
            host
        });
}

/// On the warm project, type one character into an open template's HTML text, or delete it,
/// then recompute that buffer's diagnostics.
#[divan::bench(args = REAL_PROJECTS)]
fn keystroke_diagnostics(bencher: Bencher, name: &str) {
    let Some(project) = load(name) else {
        return;
    };
    let mut keystroke = Keystroke::open(&project);
    bencher.bench_local(|| black_box(keystroke.type_and_check()));
}
