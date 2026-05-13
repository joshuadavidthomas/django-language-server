use divan::Bencher;
use djls_bench::prime;
use djls_bench::realistic_db;
use djls_bench::template_fixtures;
use djls_bench::validation_error_fixtures;
use djls_bench::TemplateFixture;
use djls_bench::ValidationErrorFixture;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::Severity;
use djls_source::Span;

fn main() {
    divan::main();
}

const DIAGNOSTICS_INNER_ITERS: usize = 8;
const DIAGNOSTICS_WARMUP_ITERS: usize = 3;

// Rendering engine benchmarks

fn span_of(source: &str, needle: &str) -> Span {
    let start = source
        .find(needle)
        .expect("span_of: needle not found in source");
    Span::saturating_from_bounds_usize(start, start + needle.len())
}

static SINGLE_SPAN_SOURCE: &str = "{% block content %}\n<p>Hello</p>\n{% endblock %}\n";

static MULTI_SPAN_SOURCE: &str = "{% block sidebar %}\n<nav>Links</nav>\n{% endblock content %}\n";

static MANY_ERRORS_SOURCE: &str = concat!(
    "{% if and x %}oops{% endif %}\n",
    "{{ name|title:\"arg\" }}\n",
    "{{ text|truncatewords }}\n",
    "{% trans \"hello\" %}\n",
    "{% unknown_tag %}\n",
    "{% for %}empty{% endfor %}\n",
    "{{ value|bogus }}\n",
    "{% block a %}{% endblock b %}\n",
);

fn single_span_diagnostic() -> Diagnostic<'static> {
    Diagnostic::new(
        SINGLE_SPAN_SOURCE,
        "templates/page.html",
        "S100",
        "Unclosed tag: block",
        Severity::Error,
        span_of(SINGLE_SPAN_SOURCE, "{% block content %}"),
        "this block tag is never closed",
    )
}

fn multi_span_diagnostic() -> Diagnostic<'static> {
    Diagnostic::new(
        MULTI_SPAN_SOURCE,
        "templates/layout.html",
        "S103",
        "'content' does not match 'sidebar'",
        Severity::Error,
        span_of(MULTI_SPAN_SOURCE, "{% endblock content %}"),
        "closing tag says 'content'",
    )
    .annotation(
        span_of(MULTI_SPAN_SOURCE, "{% block sidebar %}"),
        "opening tag is 'sidebar'",
        false,
    )
}

#[divan::bench]
fn render_single_span(bencher: Bencher) {
    let renderer = DiagnosticRenderer::plain();
    let diag = single_span_diagnostic();

    bencher.bench_local(move || {
        divan::black_box(renderer.render(&diag));
    });
}

#[divan::bench]
fn render_multi_span(bencher: Bencher) {
    let renderer = DiagnosticRenderer::plain();
    let diag = multi_span_diagnostic();

    bencher.bench_local(move || {
        divan::black_box(renderer.render(&diag));
    });
}

#[divan::bench]
fn render_styled_single_span(bencher: Bencher) {
    let renderer = DiagnosticRenderer::styled();
    let diag = single_span_diagnostic();

    bencher.bench_local(move || {
        divan::black_box(renderer.render(&diag));
    });
}

#[divan::bench]
fn render_synthetic_diagnostics_repeated(bencher: Bencher) {
    let plain = DiagnosticRenderer::plain();
    let styled = DiagnosticRenderer::styled();
    let diagnostics = [single_span_diagnostic(), multi_span_diagnostic()];

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..100 {
            total += plain.render(&diagnostics[0]).len();
            total += plain.render(&diagnostics[1]).len();
            total += styled.render(&diagnostics[0]).len();
        }
        divan::black_box(total);
    });
}

// Collect diagnostics (realistic db — real Django specs)
// CodSpeed should treat aggregate realistic diagnostics as the primary
// regression signal. Per-fixture benches are amortized to reduce noise and are
// mainly useful for local diagnosis.

#[divan::bench(args = template_fixtures())]
fn collect_diagnostics_realistic_repeated(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    prime(DIAGNOSTICS_WARMUP_ITERS, || {
        let _ = djls_ide::collect_diagnostics(&db, file);
    });

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            total += djls_ide::collect_diagnostics(&db, file).len();
        }
        divan::black_box(total);
    });
}

#[divan::bench]
fn collect_diagnostics_all_realistic(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = realistic_db();

    let files: Vec<_> = fixtures
        .iter()
        .map(|fixture| {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            prime(DIAGNOSTICS_WARMUP_ITERS, || {
                let _ = djls_ide::collect_diagnostics(&db, file);
            });
            file
        })
        .collect();

    bencher.bench_local(move || {
        for file in &files {
            divan::black_box(djls_ide::collect_diagnostics(&db, *file));
        }
    });
}

// Incremental diagnostics (realistic db — simulates editor edits)

#[divan::bench(args = template_fixtures())]
fn collect_diagnostics_incremental_repeated(bencher: Bencher, fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);

    prime(DIAGNOSTICS_WARMUP_ITERS, || {
        let _ = djls_ide::collect_diagnostics(&db, file);
    });

    let original = fixture.source.clone();
    let modified = {
        let mut text = original.clone();
        text.push(' ');
        text
    };

    let mut revision = 1_u64;
    let mut use_modified = true;

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            let contents = if use_modified { &modified } else { &original };
            use_modified = !use_modified;

            db.set_file_contents(file, contents, revision);
            revision = revision.wrapping_add(1);

            total += djls_ide::collect_diagnostics(&db, file).len();
        }
        divan::black_box(total);
    });
}

// Render validation errors from real templates (realistic db)

struct ValidationRenderFixture<'a> {
    source: &'a str,
    path: &'a str,
    check: djls_db::CheckResult,
}

fn validation_render_fixture(fixture: &ValidationErrorFixture) -> ValidationRenderFixture<'_> {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
    let check = djls_db::check_file(&db, file);
    assert!(
        !check.validation_errors.is_empty(),
        "validation error rendering fixture '{}' produced no validation errors",
        fixture.label,
    );

    ValidationRenderFixture {
        source: &fixture.source,
        path: fixture.path.as_str(),
        check,
    }
}

#[divan::bench(args = validation_error_fixtures())]
fn render_validation_errors_repeated(bencher: Bencher, fixture: &ValidationErrorFixture) {
    let fixture = validation_render_fixture(fixture);
    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        let mut rendered_count = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            for error in &fixture.check.validation_errors {
                if djls_db::render_validation_error(
                    fixture.source,
                    fixture.path,
                    error,
                    &config,
                    &renderer,
                )
                .is_some()
                {
                    rendered_count += 1;
                }
            }
        }
        divan::black_box(rendered_count);
    });
}

#[divan::bench]
fn render_validation_errors_batch(bencher: Bencher) {
    let fixtures: Vec<_> = validation_error_fixtures()
        .iter()
        .map(validation_render_fixture)
        .collect();
    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        let mut rendered_count = 0;
        for fixture in &fixtures {
            for error in &fixture.check.validation_errors {
                if djls_db::render_validation_error(
                    fixture.source,
                    fixture.path,
                    error,
                    &config,
                    &renderer,
                )
                .is_some()
                {
                    rendered_count += 1;
                }
            }
        }
        divan::black_box(rendered_count);
    });
}

// Render many synthetic errors (stress test with realistic specs)

#[divan::bench]
fn render_many_synthetic_errors(bencher: Bencher) {
    let mut db = realistic_db();
    let file = db.file_with_contents("bench.html".into(), MANY_ERRORS_SOURCE);

    let check = djls_db::check_file(&db, file);
    assert!(
        !check.validation_errors.is_empty(),
        "synthetic validation error benchmark produced no validation errors",
    );

    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        for error in &check.validation_errors {
            divan::black_box(djls_db::render_validation_error(
                MANY_ERRORS_SOURCE,
                "bench.html",
                error,
                &config,
                &renderer,
            ));
        }
    });
}
